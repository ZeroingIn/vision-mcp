//! vision-mcp: a multimodal vision proxy for text-only LLM agents.
//!
//! Provides both an MCP server (stdio / streamable-http) and a CLI that share
//! the same core. Supports any OpenAI-compatible or Anthropic multimodal endpoint
//! via user-configured `base_url` / `api_key` / `model`.

pub mod adapter;
pub mod cli;
pub mod config;
pub mod core;
pub mod image;
pub mod mcp;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing to stderr (so stdout stays clean for both MCP and CLI).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .try_init();

    let cli = cli::Cli::parse();
    cli::run(cli).await
}
