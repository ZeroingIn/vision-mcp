//! Provider adapters: build a `genai::Client` and turn (instruction, images)
//! into a vision chat request, then parse the reply.
//!
//! The hand-rolled OpenAI/Anthropic HTTP code has been replaced by the `genai`
//! crate (v0.6+), which natively supports OpenAI Vision / Anthropic Vision /
//! Gemini Vision with one-line model switching. The active provider is chosen
//! at config-resolution time (see `config::Provider`) and bound to the client
//! via a `ServiceTargetResolver` closure that injects the configured
//! `base_url` + `api_key` + `AdapterKind`.

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatResponseFormat, ContentPart};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};

use crate::config::ResolvedConfig;
use crate::image::{ImageInput, is_url_sentinel};

/// What the core wants back from an adapter call.
#[derive(Debug, Clone)]
pub struct AdapterResult {
    /// The model's text reply.
    pub text: String,
    /// Reported token usage (best-effort; 0 if unknown).
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Options for a single describe call.
#[derive(Debug, Clone, Default)]
pub struct DescribeOpts {
    /// Force the model to return a JSON object (strict). When true, adapters must
    /// enforce JSON output and the core will validate it.
    pub json_mode: bool,
    /// OpenAI detail override (low|high|auto); falls back to config.
    /// Passed to OpenAI-compatible providers via `extra_body.detail`.
    pub detail: Option<String>,
    /// Override max_tokens for this call.
    pub max_tokens: Option<u32>,
}

/// Map our `Provider` enum to genai's `AdapterKind`.
fn adapter_kind_for(provider: crate::config::Provider) -> AdapterKind {
    match provider {
        crate::config::Provider::OpenAI => AdapterKind::OpenAI,
        crate::config::Provider::Anthropic => AdapterKind::Anthropic,
    }
}

/// Build a `genai::Client` that always targets the configured endpoint/auth.
///
/// A `ServiceTargetResolver` closure overrides every outgoing request's
/// endpoint + auth + adapter kind, so the model name passed to `exec_chat`
/// does not need to follow genai's built-in inference rules (e.g. it can be a
/// gateway model id like `my-custom-model` while still routing through the OpenAI
/// Chat Completions wire format).
pub fn build_client(cfg: &ResolvedConfig) -> Result<Client> {
    // genai joins "chat/completions" (or "messages") onto the endpoint via
    // `reqwest::Url::join`, which follows RFC 3986: a base without a trailing
    // slash has its last segment replaced. So `…/v1` + `chat/completions` →
    // `…/chat/completions` (losing `/v1`). Ensure the base ends with '/'.
    let mut base = cfg.base_url.trim_end_matches('/').to_string();
    base.push('/');
    let endpoint = Endpoint::from_owned(base);
    let auth = if cfg.api_key.is_empty() {
        AuthData::None
    } else {
        AuthData::from_single(cfg.api_key.clone())
    };
    let adapter_kind = adapter_kind_for(cfg.provider);
    let model_name = cfg.model.clone();

    let resolver = ServiceTargetResolver::from_resolver_fn(move |_target: ServiceTarget| {
        // Re-bind every outgoing request to the configured endpoint/auth/kind. The
        // model name is preserved from config so the wire format matches the
        // configured adapter (e.g. a gateway model id like `my-custom-model` still
        // routes through the OpenAI Chat Completions format).
        Ok(ServiceTarget {
            endpoint: endpoint.clone(),
            auth: auth.clone(),
            model: ModelIden::new(adapter_kind, model_name.as_str()),
        })
    });

    let reqwest_client = http_client(cfg.timeout_secs)?;
    Ok(Client::builder()
        .with_reqwest(reqwest_client)
        .with_service_target_resolver(resolver)
        .build())
}

/// The single unified adapter (genai-backed). Replaces the old OpenAI/Anthropic
/// enum dispatch — genai handles provider wire-format differences internally.
pub struct Adapter {
    client: Client,
}

impl Adapter {
    pub async fn describe(
        &self,
        cfg: &ResolvedConfig,
        instruction: &str,
        images: &[ImageInput],
        opts: &DescribeOpts,
    ) -> Result<AdapterResult> {
        if images.is_empty() {
            return Err(anyhow!("at least one image is required"));
        }

        // Build multipart content: instruction first, then each image.
        let mut parts: Vec<ContentPart> = Vec::with_capacity(1 + images.len());
        parts.push(ContentPart::from_text(instruction));

        for img in images {
            if let Some(url) = is_url_sentinel(img) {
                // Pass the remote URL directly (supported by OpenAI/Gemini;
                // Anthropic lacks URL images, but genai will surface that error).
                parts.push(ContentPart::from_binary_url(
                    img.mime_if_real().unwrap_or("image/png"),
                    url,
                    None,
                ));
            } else {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&img.bytes);
                parts.push(ContentPart::from_binary_base64(&img.mime, b64, None));
            }
        }

        let chat_req = ChatRequest::default().append_message(ChatMessage::user(parts));

        let mut chat_opts = ChatOptions::default();
        let max_tokens = opts.max_tokens.unwrap_or(cfg.max_tokens);
        chat_opts = chat_opts.with_max_tokens(max_tokens);

        if opts.json_mode {
            chat_opts = chat_opts.with_response_format(ChatResponseFormat::JsonMode);
        }

        // OpenAI `detail` hint (low|high|auto): inject via extra_body so it is
        // forwarded to OpenAI-compatible providers and silently ignored by
        // Anthropic (which does not understand it).
        let detail = opts
            .detail
            .clone()
            .or_else(|| cfg.detail.clone())
            .unwrap_or_else(|| "auto".to_string());
        if cfg.provider == crate::config::Provider::OpenAI {
            chat_opts = chat_opts.with_extra_body(serde_json::json!({ "detail": detail }));
        }

        let model = ModelIden::new(adapter_kind_for(cfg.provider), cfg.model.as_str());
        let resp = self
            .client
            .exec_chat(model, chat_req, Some(&chat_opts))
            .await
            .map_err(|e| anyhow!(format!("{e:#}")))
            .with_context(|| format!("exec_chat to {} {}", cfg.provider_label(), cfg.model))?;

        let text = resp
            .first_text()
            .map(|s| s.to_string())
            .or_else(|| {
                // Some OpenAI-compatible reasoning models return text in
                // `reasoning_content` when `content` is null.
                resp.reasoning_content.clone()
            })
            .ok_or_else(|| {
                anyhow!("no text content in response (model: {})", cfg.model)
            })?;

        let input_tokens = resp.usage.prompt_tokens.unwrap_or(0).max(0) as u32;
        let output_tokens = resp.usage.completion_tokens.unwrap_or(0).max(0) as u32;

        Ok(AdapterResult {
            text,
            input_tokens,
            output_tokens,
        })
    }
}

/// Build an adapter (genai Client wrapper) for a resolved config.
pub fn make_adapter(cfg: &ResolvedConfig) -> Result<Adapter> {
    Ok(Adapter {
        client: build_client(cfg)?,
    })
}

/// Install the `ring` CryptoProvider as rustls's process-wide default.
///
/// We build reqwest with the `rustls-no-provider` feature (no `aws-lc-rs`,
/// which needs CMake/NASM and breaks cross-compilation). In that mode reqwest
/// PANICS at `Client` construction unless a CryptoProvider is installed first.
/// `install_default` is idempotent — it returns Err (which we ignore) if a
/// provider is already installed. Safe to call from every `http_client()`.
fn install_tls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Helper: build a reqwest client with rustls(ring) and a timeout.
///
/// `.no_proxy()` disables system/ENV proxy interception: vision-mcp always
/// talks to a user-configured `base_url` (often a LAN gateway), and a
/// misconfigured system proxy (e.g. a local Clash/V2Ray on :7897) returning
/// 502 for LAN hosts would otherwise break every call. Kept for image URL
/// downloads (genai manages its own HTTP for chat — but we inject this same
/// client into genai via `with_reqwest`, so it applies to chat calls too).
pub fn http_client(timeout_secs: u64) -> Result<reqwest::Client> {
    install_tls_provider();
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .no_proxy()
        .build()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Provider;

    #[test]
    fn adapter_kind_mapping() {
        assert!(matches!(
            adapter_kind_for(Provider::OpenAI),
            AdapterKind::OpenAI
        ));
        assert!(matches!(
            adapter_kind_for(Provider::Anthropic),
            AdapterKind::Anthropic
        ));
    }
}
