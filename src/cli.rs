//! CLI surface: describe / ocr / compare / config / serve.

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::adapter::{DescribeOpts, http_client};
use crate::config::{Overrides, config_path, load_config_file, resolve};
use crate::core::{self, DescribeResult};
use crate::image::{self, ImageInput};
#[derive(Parser, Debug)]
#[command(
    name = "vision-mcp",
    version,
    about = "Multimodal vision proxy for text-only LLM agents (MCP server + CLI)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Run as an MCP server.
    Serve {
        /// Transport: stdio (default) or http.
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Bind address for http transport (default 127.0.0.1:8000).
        #[arg(long, default_value = "127.0.0.1:8000")]
        bind: String,
    },
    /// Describe one or more images (CLI).
    Describe {
        /// Image source(s): path / URL / "-"(stdin). Repeatable.
        #[arg(required = true)]
        images: Vec<String>,
        /// Instruction / question.
        #[arg(short, long)]
        prompt: Option<String>,
        /// Override model.
        #[arg(long)]
        model: Option<String>,
        /// OpenAI detail.
        #[arg(long)]
        detail: Option<String>,
        /// Force-download URL images to base64.
        #[arg(long)]
        fetch_url: bool,
        /// Override max output tokens.
        #[arg(long)]
        max_tokens: Option<u32>,
        /// Use a specific profile.
        #[arg(long)]
        profile: Option<String>,
        /// Output structured JSON to stdout.
        #[arg(long)]
        json: bool,
    },
    /// Extract text from an image (OCR).
    Ocr {
        /// Image source: path / URL / "-".
        image: String,
        /// Extra OCR instruction.
        #[arg(short, long)]
        prompt: Option<String>,
        /// Output format: text / markdown / json.
        #[arg(long)]
        format: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        detail: Option<String>,
        #[arg(long)]
        fetch_url: bool,
        #[arg(long)]
        max_tokens: Option<u32>,
        #[arg(long)]
        profile: Option<String>,
        /// Output structured JSON to stdout (wraps the result).
        #[arg(long)]
        json: bool,
    },
    /// Compare two or more images.
    Compare {
        /// Image sources (≥2): path / URL / "-".
        #[arg(required = true, num_args = 2..)]
        images: Vec<String>,
        #[arg(short, long)]
        prompt: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        detail: Option<String>,
        #[arg(long)]
        fetch_url: bool,
        #[arg(long)]
        max_tokens: Option<u32>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show or inspect configuration.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Print the resolved active profile.
    Show,
    /// Print the config file path.
    Path,
}

/// Dispatch a CLI command.
pub async fn run(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::Serve { transport, bind } => {
            let cfg = load_config_file(&config_path()?)?;
            let t = transport.to_ascii_lowercase();
            match t.as_str() {
                "stdio" => crate::mcp::serve_stdio(cfg).await,
                "http" => crate::mcp::serve_http(cfg, bind).await,
                other => Err(anyhow::anyhow!("unknown transport '{other}', use stdio|http")),
            }
        }
        Cmd::Describe {
            images,
            prompt,
            model,
            detail,
            fetch_url,
            max_tokens,
            profile,
            json,
        } => {
            let cfg = load_config_file(&config_path()?)?;
            let resolved = resolve(
                &cfg,
                &Overrides {
                    profile,
                    model,
                    detail: detail.clone(),
                    max_tokens,
                    ..Default::default()
                },
            )?;
            let result = run_describe(
                &resolved,
                images,
                prompt,
                core::default_view_instruction(),
                false,
                detail,
                fetch_url,
                max_tokens,
            )
            .await?;
            print_result(&result, json);
            Ok(())
        }
        Cmd::Ocr {
            image,
            prompt,
            format,
            model,
            detail,
            fetch_url,
            max_tokens,
            profile,
            json,
        } => {
            let cfg = load_config_file(&config_path()?)?;
            let resolved = resolve(
                &cfg,
                &Overrides {
                    profile,
                    model,
                    detail: detail.clone(),
                    max_tokens,
                    ..Default::default()
                },
            )?;
            let json_strict = format
                .as_deref()
                .map(|f| f.eq_ignore_ascii_case("json"))
                .unwrap_or(false);
            let instruction = match format.as_deref() {
                Some(f) if f.eq_ignore_ascii_case("json") => {
                    let base = prompt.clone().unwrap_or_else(core::default_ocr_instruction);
                    format!(
                        "{base}\n\n【输出格式】只输出一个合法 JSON 对象：{{\"text\":\"提取到的文字\",\"type\":\"图片类型\"}}，不要输出其他内容。"
                    )
                }
                _ => prompt.clone().unwrap_or_else(core::default_ocr_instruction),
            };
            let result = run_describe(
                &resolved,
                vec![image],
                Some(instruction),
                core::default_ocr_instruction(),
                json_strict,
                detail,
                fetch_url,
                max_tokens,
            )
            .await?;
            print_result(&result, json);
            Ok(())
        }
        Cmd::Compare {
            images,
            prompt,
            model,
            detail,
            fetch_url,
            max_tokens,
            profile,
            json,
        } => {
            if images.len() < 2 {
                anyhow::bail!("compare requires at least 2 images");
            }
            let cfg = load_config_file(&config_path()?)?;
            let resolved = resolve(
                &cfg,
                &Overrides {
                    profile,
                    model,
                    detail: detail.clone(),
                    max_tokens,
                    ..Default::default()
                },
            )?;
            let result = run_describe(
                &resolved,
                images,
                prompt,
                core::default_compare_instruction(),
                false,
                detail,
                fetch_url,
                max_tokens,
            )
            .await?;
            print_result(&result, json);
            Ok(())
        }
        Cmd::Config { action } => match action {
            ConfigCmd::Path => {
                println!("{}", config_path()?.display());
                Ok(())
            }
            ConfigCmd::Show => {
                let path = config_path()?;
                let cfg = load_config_file(&path)?;
                let resolved = resolve(&cfg, &Overrides::default())?;
                println!("# config file: {}", path.display());
                println!("provider      = {:?}", resolved.provider);
                println!("base_url      = {}", resolved.base_url);
                println!("model         = {}", resolved.model);
                println!("max_tokens    = {}", resolved.max_tokens);
                println!("timeout_secs  = {}", resolved.timeout_secs);
                println!(
                    "api_key       = {}",
                    if resolved.api_key.is_empty() {
                        "(unset)"
                    } else {
                        "(set)"
                    }
                );
                Ok(())
            }
        },
    }
}

/// Shared describe runner used by all CLI subcommands.
async fn run_describe(
    cfg: &crate::config::ResolvedConfig,
    images: Vec<String>,
    prompt: Option<String>,
    default_instruction: String,
    json_strict: bool,
    detail: Option<String>,
    fetch_url: bool,
    max_tokens: Option<u32>,
) -> Result<DescribeResult> {
    let client = http_client(cfg.timeout_secs)?;
    let mut imgs: Vec<ImageInput> = Vec::with_capacity(images.len());
    for src in images {
        imgs.push(image::resolve_image(&src, fetch_url, &client).await?);
    }
    let instruction = prompt.unwrap_or(default_instruction);
    let opts = DescribeOpts {
        json_mode: json_strict,
        detail,
        max_tokens,
    };
    core::describe(cfg, &imgs, &instruction, json_strict, &opts).await
}

/// Print the result: plain text by default, JSON object with `--json`.
fn print_result(r: &DescribeResult, json: bool) {
    if json {
        let v = serde_json::json!({
            "description": r.text,
            "provider": r.provider,
            "model": r.model,
            "usage": {
                "input_tokens": r.input_tokens,
                "output_tokens": r.output_tokens,
            },
            "elapsed_ms": r.elapsed_ms,
            "images": r.images,
        });
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
    } else {
        println!("{}", r.text);
    }
}

// Re-export for tests that want to build a CLI programmatically.
pub use clap;
