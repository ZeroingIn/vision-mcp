//! Integration tests for the genai-backed adapter and core orchestration.
//!
//! The old tests poked at hand-rolled OpenAI/Anthropic adapter internals
//! (exact headers, paths, body shapes). Now that `genai` owns the wire format,
//! these tests verify behavior through `core::describe` against a mock HTTP
//! server that speaks the OpenAI Chat Completions shape — which is what an
//! OpenAI-compatible gateway (our default profile) expects.
//!
//! For each call we:
//!   - mount a mock that accepts any POST and returns a canned response, and
//!   - assert the high-level outcome (text, usage, JSON strictness, errors).
//!
//! This keeps the tests robust to genai's internal request construction while
//! still exercising the full describe path (image → base64 → request → parse).

use std::sync::Arc;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use vision_mcp::adapter::DescribeOpts;
use vision_mcp::config::{Provider, ResolvedConfig};
use vision_mcp::core;

const PNG_BYTES: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
    0x00, 0x00, 0x00, 0x0D, // IHDR length
    0x49, 0x48, 0x44, 0x52, // "IHDR"
];

/// Build a ResolvedConfig pointing at the mock server (OpenAI-compatible).
/// The base_url ends with `/v1/` so genai's `Url::join("chat/completions")`
/// resolves to `<server>/v1/chat/completions`.
fn resolved_openai(base_url: String) -> ResolvedConfig {
    ResolvedConfig {
        provider: Provider::OpenAI,
        base_url,
        api_key: "test-key".into(),
        model: "gpt-4o".into(),
        max_tokens: 512,
        detail: None,
        timeout_secs: 10,
        default_instruction: core::default_view_instruction(),
    }
}

fn png_image() -> vision_mcp::image::ImageInput {
    vision_mcp::image::ImageInput {
        mime: "image/png".into(),
        bytes: PNG_BYTES.to_vec(),
    }
}

/// Mount a mock that returns a plain OpenAI chat completion with `text`.
async fn mount_openai_ok(server: &MockServer, text: &str, input: u32, output: u32) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": { "role": "assistant", "content": text }
            }],
            "usage": { "prompt_tokens": input, "completion_tokens": output }
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn describe_returns_text_and_usage() {
    let server = MockServer::start().await;
    // genai joins "chat/completions" onto the endpoint; with base `.../v1/`
    // the path becomes `/v1/chat/completions`.
    let cfg = resolved_openai(format!("{}/v1/", server.uri()));

    mount_openai_ok(&server, "类型：截图\n一个按钮", 10, 5).await;

    let res = core::describe(
        &cfg,
        &[png_image()],
        "描述这张图",
        false,
        &DescribeOpts::default(),
    )
    .await
    .expect("describe ok");

    assert_eq!(res.text, "类型：截图\n一个按钮");
    assert_eq!(res.provider, "openai");
    assert_eq!(res.model, "gpt-4o");
    assert_eq!(res.input_tokens, 10);
    assert_eq!(res.output_tokens, 5);
    assert!(res.elapsed_ms < 5000);
}

#[tokio::test]
async fn describe_propagates_http_error() {
    let server = MockServer::start().await;
    let cfg = resolved_openai(format!("{}/v1/", server.uri()));

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .mount(&server)
        .await;

    let err = core::describe(
        &cfg,
        &[png_image()],
        "x",
        false,
        &DescribeOpts::default(),
    )
    .await
    .expect_err("should error");

    let msg = format!("{err:#}");
    // genai surfaces the status code / body in its error text.
    assert!(
        msg.contains("429") || msg.contains("rate") || msg.contains("Too Many"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn describe_json_strict_extracts_and_validates_object() {
    let server = MockServer::start().await;
    let cfg = Arc::new(resolved_openai(format!("{}/v1/", server.uri())));

    // The model returns prose-wrapped JSON; core must extract & validate it.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": { "content": "好的，结果如下：\n```json\n{\"type\":\"截图\",\"text\":\"提交\"}\n```" }
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
        })))
        .mount(&server)
        .await;

    let cfg = &*cfg;
    let opts = DescribeOpts {
        json_mode: true,
        ..Default::default()
    };
    let res = core::describe(cfg, &[png_image()], "提取文字", true, &opts)
        .await
        .expect("ok");

    let parsed: serde_json::Value = serde_json::from_str(&res.text).expect("valid json object");
    assert_eq!(parsed["type"], "截图");
    assert_eq!(parsed["text"], "提交");
}

#[tokio::test]
async fn describe_json_strict_falls_back_to_error_object() {
    let server = MockServer::start().await;
    let cfg = Arc::new(resolved_openai(format!("{}/v1/", server.uri())));

    // Both attempts return non-JSON text → core should fall back to {"error":...}.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "这不是JSON，是普通文字描述。" } }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
        })))
        .mount(&server)
        .await;

    let cfg = &*cfg;
    let opts = DescribeOpts {
        json_mode: true,
        ..Default::default()
    };
    let res = core::describe(cfg, &[png_image()], "提取文字", true, &opts)
        .await
        .expect("ok");

    let parsed: serde_json::Value = serde_json::from_str(&res.text).expect("still valid json");
    assert!(
        parsed.get("error").is_some(),
        "should be error object: {}",
        res.text
    );
}

#[tokio::test]
async fn describe_empty_images_errors() {
    let server = MockServer::start().await;
    let cfg = resolved_openai(format!("{}/v1/", server.uri()));

    let err = core::describe(&cfg, &[], "x", false, &DescribeOpts::default())
        .await
        .expect_err("should error");
    assert!(format!("{err}").contains("image"));
}

#[tokio::test]
async fn describe_handles_reasoning_content_when_content_null() {
    // Some OpenAI-compatible reasoning models return `content: null` and put
    // the answer in `reasoning_content`. genai normalizes this into
    // ChatResponse.reasoning_content, which our adapter falls back to.
    let server = MockServer::start().await;
    let cfg = resolved_openai(format!("{}/v1/", server.uri()));

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "reasoning_content": "类型：照片\n一只猫"
                }
            }],
            "usage": { "prompt_tokens": 8, "completion_tokens": 4 }
        })))
        .mount(&server)
        .await;

    let res = core::describe(
        &cfg,
        &[png_image()],
        "描述这张图",
        false,
        &DescribeOpts::default(),
    )
    .await
    .expect("describe ok");

    assert_eq!(res.text, "类型：照片\n一只猫");
}
