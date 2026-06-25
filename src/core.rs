//! Core orchestration: default prompts, describe(), strict JSON enforcement.

use std::time::Instant;

use anyhow::{Context, Result, anyhow};

use crate::adapter::{Adapter, AdapterResult, DescribeOpts};
use crate::config::ResolvedConfig;
use crate::image::ImageInput;
/// The default `view_image` instruction (Chinese). The user explicitly asked for:
/// first identify the image type (screenshot vs photo vs ...), then describe it
/// using a type-appropriate style.
pub fn default_view_instruction() -> String {
    r#"你是一位专业的图像分析助手。请按以下要求描述图片：

第一步——判断图片类型，并首先说明：
- 截图：软件界面、网页、终端、对话窗口、图表等屏幕抓取
- 照片：相机/手机拍摄的真实场景、人物、物体、风景
- 图表/图示：流程图、架构图、示意图、思维导图
- 文档扫描：证件、票据、书页、手写稿等
- 图像/插画：绘画、设计图、图标、动漫等

第二步——根据类型用对应方式描述：
【截图】说明所属平台/应用、整体布局分区、可见文字原文（关键处逐字给出）、交互元素（按钮/输入框/菜单及状态）、配色风格、当前操作上下文。
【照片】说明拍摄场景与主体、构图与景别、光照与色调、显著细节、可能的拍摄用途。
【图表/图示】说明类型与主题、节点/区块及连接关系、流向与层级、图例、关键数据。
【文档扫描】说明文档种类、版式结构、逐段转录文字（保留表格/列表结构）、清晰度与遮挡。
【图像/插画】说明风格与媒介、主体内容、构图与色彩、情感/氛围、可能用途。

第三步——附加：
- 若图中有任何文字，务必原样转写（代码/路径/报错尤其要精确，保留大小写与符号）。
- 若用户给了具体提问，在描述末尾用"回答："单独作答。
- 用 Markdown 输出，先一行"类型：xxx"。"#.to_string()
}

/// The default `compare_images` instruction.
pub fn default_compare_instruction() -> String {
    r#"你是一位专业的图像分析助手。请对比给出的多张图片：

1. 先分别用一句话概括每张图。
2. 再从主体内容、布局结构、文字、颜色、尺寸/比例等维度逐项对比。
3. 明确指出差异点（新增/删除/修改/位置变化）。
4. 若是改版/前后对比，按"变更项 -> 变更说明"列表输出。
- 用 Markdown 输出，关键差异用加粗标注。"#.to_string()
}

/// The default `extract_text` instruction (pure transcription).
pub fn default_ocr_instruction() -> String {
    r#"请对图片进行文字提取（OCR）。要求：
1. 原样转录所有可见文字，保留大小写、标点、空格与换行。
2. 保留表格、列表、代码块的结构。
3. 区分标题、正文、注释层级。
4. 若有手写文字，尽量辨认并标注"(手写)"。
5. 不添加任何解释或评论，只输出提取到的文字。
若图中无文字，输出"（未检测到文字）"。"#.to_string()
}

/// Final result of a describe call.
#[derive(Debug, Clone)]
pub struct DescribeResult {
    pub text: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub elapsed_ms: u128,
    pub images: Vec<String>,
}

/// Run a describe call end-to-end.
///
/// `instruction` is the final instruction text (caller composes defaults).
/// `json_strict`: when true, force the model to emit a valid JSON object; the
/// returned `text` is guaranteed to be a valid JSON object string.
pub async fn describe(
    cfg: &ResolvedConfig,
    images: &[ImageInput],
    instruction: &str,
    json_strict: bool,
    opts: &DescribeOpts,
) -> Result<DescribeResult> {
    let image_names: Vec<String> = images
        .iter()
        .map(|i| {
            crate::image::is_url_sentinel(i)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("<{} image, {} bytes>", i.mime, i.bytes.len()))
        })
        .collect();

    if images.is_empty() {
        return Err(anyhow!("at least one image is required"));
    }

    let start = Instant::now();
    let adapter = crate::adapter::make_adapter(cfg)?;

    let result = if json_strict {
        describe_json(cfg, &adapter, instruction, images, opts).await?
    } else {
        adapter.describe(cfg, instruction, images, opts).await?
    };

    Ok(DescribeResult {
        text: result.text,
        provider: cfg.provider_label().to_string(),
        model: cfg.model.clone(),
        input_tokens: result.input_tokens,
        output_tokens: result.output_tokens,
        elapsed_ms: start.elapsed().as_millis(),
        images: image_names,
    })
}

/// Force a valid JSON object. Strategy:
/// 1. Augment instruction with a strict JSON demand.
/// 2. Call adapter with json_mode.
/// 3. Extract the outermost {...} substring and parse it.
/// 4. If parse fails, retry once with a stronger instruction.
/// 5. If still fails, return a JSON object `{"error": "..."}`.
async fn describe_json(
    cfg: &ResolvedConfig,
    adapter: &Adapter,
    instruction: &str,
    images: &[ImageInput],
    opts: &DescribeOpts,
) -> Result<AdapterResult> {
    let augmented = format!(
        "{instruction}\n\n\
         【输出格式要求】你必须只输出一个合法的 JSON 对象，不要输出任何其他文字、\
         不要使用 Markdown 代码围栏。JSON 对象顶层须包含字段：\n\
         - \"type\": 图片类型字符串（截图/照片/图表图示/文档扫描/图像插画）\n\
         - \"description\": 详细描述字符串\n\
         - \"text\": 图中可见文字的逐字转写字符串（无则空串）\n\
         示例：{{\"type\":\"截图\",\"description\":\"...\",\"text\":\"...\"}}"
    );

    let mut last_text = String::new();
    for attempt in 0..2u8 {
        let effective = if attempt == 0 {
            augmented.clone()
        } else {
            format!(
                "{augmented}\n\n\
                 【重要】你上次的输出不是合法的 JSON 对象。请只输出 JSON 对象本身，\
                 不要有任何前后缀文字或代码围栏。上次输出片段：{snippet}",
                snippet = last_text.chars().take(200).collect::<String>()
            )
        };

        let r = adapter
            .describe(cfg, &effective, images, opts)
            .await
            .context("adapter call during json_strict")?;

        if let Some(obj) = extract_json_object(&r.text) {
            // Re-serialize canonically to guarantee validity.
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&obj) {
                return Ok(AdapterResult {
                    text: serde_json::to_string(&val).unwrap_or(obj),
                    input_tokens: r.input_tokens,
                    output_tokens: r.output_tokens,
                });
            }
        }
        last_text = r.text.clone();
    }

    // Could not get valid JSON — return a well-formed error object.
    let err = serde_json::json!({
        "error": "model did not return a valid JSON object after retry",
        "raw_excerpt": last_text.chars().take(500).collect::<String>(),
    });
    Ok(AdapterResult {
        text: serde_json::to_string(&err).unwrap(),
        input_tokens: 0,
        output_tokens: 0,
    })
}

/// Extract the outermost balanced `{ ... }` substring from `s`.
pub fn extract_json_object(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in bytes[start..].iter().enumerate() {
        let c = b as char;
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..start + i + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_json() {
        assert_eq!(
            extract_json_object(r#"{"a":1,"b":"}"}"#),
            Some(r#"{"a":1,"b":"}"}"#.to_string())
        );
    }

    #[test]
    fn extracts_json_with_prose_around() {
        let s = "Here is the result:\n```json\n{\"type\":\"截图\",\"description\":\"x\"}\n```\nDone.";
        let got = extract_json_object(s).unwrap();
        assert!(got.contains("\"type\":\"截图\""));
        assert!(got.starts_with('{') && got.ends_with('}'));
    }

    #[test]
    fn extracts_nested() {
        let s = r#"prefix {"a":{"b":2},"c":3} suffix"#;
        assert_eq!(extract_json_object(s), Some(r#"{"a":{"b":2},"c":3}"#.to_string()));
    }

    #[test]
    fn no_brace_returns_none() {
        assert!(extract_json_object("no json here").is_none());
    }

    #[test]
    fn handles_escaped_quotes() {
        let s = r#"{"a":"he said \"hi\""}"#;
        assert_eq!(extract_json_object(s), Some(s.to_string()));
    }

    #[test]
    fn default_prompts_are_chinese_and_nonempty() {
        assert!(default_view_instruction().contains("截图"));
        assert!(default_view_instruction().contains("照片"));
        assert!(default_compare_instruction().contains("对比"));
        assert!(default_ocr_instruction().contains("OCR"));
    }
}
