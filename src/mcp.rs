//! MCP server: exposes view_image / extract_text / compare_images tools via rmcp.

use anyhow::Result;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::ServerInfo,
    schemars,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;
use tracing::warn;

use crate::adapter::DescribeOpts;
use crate::config::{Config, Overrides, ResolvedConfig, resolve};
use crate::core::{self, DescribeResult};
use crate::image::{self, ImageInput};

/// The MCP service state.
#[derive(Clone)]
pub struct VisionMcp {
    cfg: Config,
}

impl VisionMcp {
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }

    /// Resolve a config snapshot for one tool call.
    fn resolved(&self, o: &Overrides) -> Result<ResolvedConfig> {
        resolve(&self.cfg, o)
    }
}

/// Shared helper: resolve images, run describe, return text.
async fn run(
    state: &VisionMcp,
    images: Vec<String>,
    prompt: Option<String>,
    instruction_default: String,
    overrides: Overrides,
    detail: Option<String>,
    fetch_url: Option<bool>,
    max_tokens: Option<u32>,
    json_strict: bool,
) -> Result<String> {
    let cfg = state.resolved(&overrides)?;
    let client = crate::adapter::http_client(cfg.timeout_secs)?;
    let fetch = fetch_url.unwrap_or(false);

    let mut imgs: Vec<ImageInput> = Vec::with_capacity(images.len());
    for src in images {
        imgs.push(image::resolve_image(&src, fetch, &client).await?);
    }

    let instruction = prompt.unwrap_or(instruction_default);
    let opts = DescribeOpts {
        json_mode: json_strict,
        detail,
        max_tokens,
    };
    let res: DescribeResult =
        core::describe(&cfg, &imgs, &instruction, json_strict, &opts).await?;
    Ok(res.text)
}

// ---------- view_image ----------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ViewImageParams {
    /// 图片来源，可多个：本地路径 / http(s) URL / "-"(stdin)。至少 1 个。
    pub images: Vec<String>,
    /// 对图片的描述/提问指令。缺省使用内置默认指令。
    #[serde(default)]
    pub prompt: Option<String>,
    /// 临时覆盖模型名。
    #[serde(default)]
    pub model: Option<String>,
    /// OpenAI detail：low/high/auto。仅 OpenAI 适配器生效。
    #[serde(default)]
    pub detail: Option<String>,
    /// 为 true 时，URL 图片会先下载转 base64 再发送（兼容不支持 URL 直传的 provider）。
    #[serde(default)]
    pub fetch_url: Option<bool>,
    /// 覆盖最大输出 token。
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// 覆盖使用的 profile 名。
    #[serde(default)]
    pub profile: Option<String>,
}

// ---------- extract_text ----------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExtractTextParams {
    /// 图片：本地路径 / URL / "-"(stdin)。
    pub image: String,
    /// 额外 OCR 指令，例如"只提取表格部分"。
    #[serde(default)]
    pub prompt: Option<String>,
    /// 输出格式：text / markdown / json。json 时严格返回 JSON 对象。
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub fetch_url: Option<bool>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub profile: Option<String>,
}

// ---------- compare_images ----------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompareImagesParams {
    /// 2 张及以上图片：路径 / URL / "-"。
    pub images: Vec<String>,
    /// 对比指令，缺省"对比这些图片的差异"。
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub fetch_url: Option<bool>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub profile: Option<String>,
}

#[tool_router]
impl VisionMcp {
    #[tool(description = "按指令查看并描述图片。先判断图片类型（截图/照片/图表/文档/插画），再分类详细描述，原样转写图中文字。适用于纯文本模型查看图片。")]
    async fn view_image(
        &self,
        Parameters(p): Parameters<ViewImageParams>,
    ) -> String {
        match run(
            self,
            p.images,
            p.prompt,
            core::default_view_instruction(),
            Overrides {
                profile: p.profile,
                model: p.model,
                ..Default::default()
            },
            p.detail,
            p.fetch_url,
            p.max_tokens,
            false,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                warn!("view_image error: {e:#}");
                format!("Error: {e:#}")
            }
        }
    }

    #[tool(description = "从图片提取文字（OCR）。原样转录可见文字，保留结构。format=json 时严格返回 JSON 对象 {\"text\":\"...\",\"type\":\"...\"}。")]
    async fn extract_text(
        &self,
        Parameters(p): Parameters<ExtractTextParams>,
    ) -> String {
        let json_strict = p
            .format
            .as_deref()
            .map(|f| f.eq_ignore_ascii_case("json"))
            .unwrap_or(false);

        let instruction = match p.format.as_deref() {
            Some(f) if f.eq_ignore_ascii_case("json") => {
                let base = p
                    .prompt
                    .clone()
                    .unwrap_or_else(core::default_ocr_instruction);
                format!(
                    "{base}\n\n【输出格式】只输出一个合法 JSON 对象：{{\"text\":\"提取到的文字\",\"type\":\"图片类型\"}}，不要输出其他内容。"
                )
            }
            _ => p.prompt.clone().unwrap_or_else(core::default_ocr_instruction),
        };

        match run(
            self,
            vec![p.image],
            Some(instruction),
            core::default_ocr_instruction(),
            Overrides {
                profile: p.profile,
                model: p.model,
                ..Default::default()
            },
            p.detail,
            p.fetch_url,
            p.max_tokens,
            json_strict,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                warn!("extract_text error: {e:#}");
                format!("Error: {e:#}")
            }
        }
    }

    #[tool(description = "对比多张图片（≥2）。逐项对比主体、布局、文字、颜色，明确指出差异点。")]
    async fn compare_images(
        &self,
        Parameters(p): Parameters<CompareImagesParams>,
    ) -> String {
        if p.images.len() < 2 {
            return "Error: compare_images requires at least 2 images".to_string();
        }
        match run(
            self,
            p.images,
            p.prompt,
            core::default_compare_instruction(),
            Overrides {
                profile: p.profile,
                model: p.model,
                ..Default::default()
            },
            p.detail,
            p.fetch_url,
            p.max_tokens,
            false,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                warn!("compare_images error: {e:#}");
                format!("Error: {e:#}")
            }
        }
    }
}

#[tool_handler(name = "vision-mcp", version = "0.1.0")]
impl ServerHandler for VisionMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default().with_instructions(
            "vision-mcp: 让纯文本模型通过多模态模型查看图片。\
             工具：view_image（描述）、extract_text（OCR）、compare_images（对比）。",
        )
    }
}

/// Run the MCP server over stdio.
pub async fn serve_stdio(cfg: Config) -> Result<()> {
    let service = VisionMcp::new(cfg);
    let server = service.serve(stdio()).await?;
    server.waiting().await?;
    Ok(())
}

/// Run the MCP server over streamable HTTP (axum, plain HTTP, no auth).
pub async fn serve_http(cfg: Config, bind: String) -> Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };

    let ct = tokio_util::sync::CancellationToken::new();
    let service = StreamableHttpService::new(
        move || Ok(VisionMcp::new(cfg.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token()),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("vision-mcp HTTP listening on http://{bind}/mcp (no auth, plain HTTP)");
    let _ = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            ct.cancel();
        })
        .await;
    Ok(())
}
