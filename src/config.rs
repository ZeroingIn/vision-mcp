//! Configuration loading for vision-mcp.
//!
//! Precedence (low -> high): config file < environment variables < CLI flags.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

/// Top-level config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Name of the active profile.
    #[serde(default = "default_profile_name")]
    pub active: String,
    /// Named provider profiles.
    #[serde(default)]
    pub profiles: std::collections::BTreeMap<String, Profile>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            active: default_profile_name(),
            profiles: std::collections::BTreeMap::new(),
        }
    }
}

fn default_profile_name() -> String {
    "default".to_string()
}

/// A provider profile: how to reach one multimodal model endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// Adapter family: "openai" | "anthropic" | "auto".
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Base URL of the API (no trailing path like /chat/completions).
    pub base_url: String,
    /// API key. May be empty when sourced from env.
    #[serde(default)]
    pub api_key: String,
    /// Model name to send to the provider.
    pub model: String,
    /// Max output tokens.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// OpenAI `detail` hint: low|high|auto. Ignored by Anthropic.
    #[serde(default)]
    pub detail: Option<String>,
    /// Request timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Default instruction used when a tool is called without a prompt.
    #[serde(default)]
    pub default_instruction: Option<String>,
}

fn default_provider() -> String {
    "auto".to_string()
}

fn default_max_tokens() -> u32 {
    1024
}

fn default_timeout() -> u64 {
    60
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            max_tokens: default_max_tokens(),
            detail: None,
            timeout_secs: default_timeout(),
            default_instruction: None,
        }
    }
}

/// Resolved, fully-populated settings used by the core layer.
/// Built from (config file profile) ∪ (env) ∪ (flag overrides).
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub provider: Provider,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub detail: Option<String>,
    pub timeout_secs: u64,
    pub default_instruction: String,
}

impl ResolvedConfig {
    /// Human-readable provider label (lowercase), used in logging/output.
    pub fn provider_label(&self) -> &'static str {
        match self.provider {
            Provider::OpenAI => "openai",
            Provider::Anthropic => "anthropic",
        }
    }
}

/// Concrete adapter family to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAI,
    Anthropic,
}

impl Provider {
    /// Resolve "auto" by inspecting the base_url.
    pub fn resolve(kind: &str, base_url: &str) -> Result<Self> {
        match kind {
            "openai" => Ok(Provider::OpenAI),
            "anthropic" => Ok(Provider::Anthropic),
            "auto" => {
                let u = base_url.to_ascii_lowercase();
                if u.contains("anthropic.com") || u.contains("/v1/messages") {
                    Ok(Provider::Anthropic)
                } else {
                    Ok(Provider::OpenAI)
                }
            }
            other => Err(anyhow!("unknown provider '{other}', expected openai|anthropic|auto")),
        }
    }
}

/// Flag overrides passed from the CLI / MCP tool call.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub profile: Option<String>,
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub detail: Option<String>,
    pub timeout_secs: Option<u64>,
}

/// Return the canonical config file path: `config.toml` next to the running
/// executable (portable layout — config travels with the binary).
///
/// If the executable path cannot be determined (very unusual), this falls back
/// to `./config.toml` in the current working directory.
///
/// Override with the `VMCP_CONFIG` environment variable for explicit control.
pub fn config_path() -> Result<PathBuf> {
    if let Some(p) = env("VMCP_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(dir.join("config.toml"))
}

/// Load the config file if it exists; otherwise return an empty Config.
pub fn load_config_file(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    let cfg: Config = toml::from_str(&raw)
        .with_context(|| format!("parsing config file {}", path.display()))?;
    Ok(cfg)
}

/// Read a single env var, returning None if unset or empty.
fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Fully resolve settings for the active profile + env + overrides.
pub fn resolve(cfg: &Config, overrides: &Overrides) -> Result<ResolvedConfig> {
    // 1. Pick profile name: override > config.active > "default"
    let name = overrides
        .profile
        .clone()
        .or_else(|| env("VMCP_ACTIVE"))
        .unwrap_or_else(|| cfg.active.clone());

    // 2. Base profile (from file, or a default empty one).
    let base = cfg
        .profiles
        .get(&name)
        .cloned()
        .unwrap_or_else(|| Profile {
            provider: default_provider(),
            ..Profile::default()
        });

    // 3. Env overrides (VMCP_*).
    // 4. Flag overrides (highest).
    let provider_kind = overrides
        .provider
        .clone()
        .or_else(|| env("VMCP_PROVIDER"))
        .unwrap_or_else(|| base.provider.clone());
    let base_url = overrides
        .base_url
        .clone()
        .or_else(|| env("VMCP_BASE_URL"))
        .unwrap_or_else(|| base.base_url.clone());
    let api_key = overrides
        .api_key
        .clone()
        .or_else(|| env("VMCP_API_KEY"))
        .unwrap_or_else(|| base.api_key.clone());
    let model = overrides
        .model
        .clone()
        .or_else(|| env("VMCP_MODEL"))
        .unwrap_or_else(|| base.model.clone());
    let max_tokens = overrides
        .max_tokens
        .or_else(|| env("VMCP_MAX_TOKENS").and_then(|s| s.parse().ok()))
        .unwrap_or(base.max_tokens);
    let detail = overrides
        .detail
        .clone()
        .or_else(|| env("VMCP_DETAIL"))
        .or(base.detail.clone());
    let timeout_secs = overrides
        .timeout_secs
        .or_else(|| env("VMCP_TIMEOUT").and_then(|s| s.parse().ok()))
        .unwrap_or(base.timeout_secs);
    let default_instruction = base
        .default_instruction
        .clone()
        .or_else(|| env("VMCP_INSTRUCTION"))
        .unwrap_or_else(crate::core::default_view_instruction);

    if base_url.trim().is_empty() {
        return Err(anyhow!(
            "base_url is not set. Configure it in {} or set VMCP_BASE_URL.",
            config_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "the config file".into())
        ));
    }
    if model.trim().is_empty() {
        return Err(anyhow!("model is not set. Set it in the active profile or VMCP_MODEL."));
    }

    let provider = Provider::resolve(&provider_kind, &base_url)?;

    Ok(ResolvedConfig {
        provider,
        base_url,
        api_key,
        model,
        max_tokens,
        detail,
        timeout_secs,
        default_instruction,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VMCP_VARS: &[&str] = &[
        "VMCP_ACTIVE",
        "VMCP_PROVIDER",
        "VMCP_BASE_URL",
        "VMCP_API_KEY",
        "VMCP_MODEL",
        "VMCP_MAX_TOKENS",
        "VMCP_DETAIL",
        "VMCP_TIMEOUT",
        "VMCP_INSTRUCTION",
    ];

    fn save_vmcp_env() -> Vec<(String, Option<String>)> {
        VMCP_VARS
            .iter()
            .map(|k| ((*k).to_string(), std::env::var(k).ok()))
            .collect()
    }

    fn clear_vmcp_env() {
        for k in VMCP_VARS {
            std::env::remove_var(k);
        }
    }

    fn restore_vmcp_env(saved: Vec<(String, Option<String>)>) {
        for (k, v) in saved {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }

    #[test]
    fn resolve_auto_by_url() {
        assert_eq!(
            Provider::resolve("auto", "https://api.anthropic.com").unwrap(),
            Provider::Anthropic
        );
        assert_eq!(
            Provider::resolve("auto", "https://api.openai.com/v1").unwrap(),
            Provider::OpenAI
        );
        assert_eq!(
            Provider::resolve("auto", "https://dashscope.aliyuncs.com/compatible-mode/v1").unwrap(),
            Provider::OpenAI
        );
    }

    #[test]
    fn resolve_explicit() {
        assert_eq!(Provider::resolve("anthropic", "https://x").unwrap(), Provider::Anthropic);
        assert_eq!(Provider::resolve("openai", "https://x").unwrap(), Provider::OpenAI);
        assert!(Provider::resolve("weird", "https://x").is_err());
    }

    #[test]
    fn overrides_win_over_profile() {
        // Snapshot VMCP_* env so the precedence test is deterministic regardless of
        // what the host environment happens to set.
        let saved = save_vmcp_env();
        clear_vmcp_env();

        let mut cfg = Config::default();
        cfg.profiles.insert(
            "default".into(),
            Profile {
                provider: "openai".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key: "file-key".into(),
                model: "gpt-4o".into(),
                ..Profile::default()
            },
        );
        let r = resolve(
            &cfg,
            &Overrides {
                model: Some("gpt-4o-mini".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.model, "gpt-4o-mini");
        assert_eq!(r.api_key, "file-key");

        restore_vmcp_env(saved);
    }

    #[test]
    fn missing_base_url_errors() {
        let saved = save_vmcp_env();
        clear_vmcp_env();
        let cfg = Config::default();
        let result = resolve(&cfg, &Overrides::default());
        assert!(result.is_err());
        restore_vmcp_env(saved);
    }
}
