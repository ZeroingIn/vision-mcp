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
