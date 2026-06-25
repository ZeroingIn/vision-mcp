//! Image source resolution: path / URL / stdin -> normalized bytes + mime.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use base64::Engine;

/// A resolved image ready to send to an adapter.
#[derive(Debug, Clone)]
pub struct ImageInput {
    /// MIME type, e.g. "image/png".
    pub mime: String,
    /// Raw bytes.
    pub bytes: Vec<u8>,
}

impl ImageInput {
    /// Raw base64 (no data: prefix).
    pub fn b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.bytes)
    }

    /// `data:<mime>;base64,<b64>` URI.
    pub fn data_uri(&self) -> String {
        format!("data:{};base64,{}", self.mime, self.b64())
    }

    /// The real MIME type when this input holds actual image bytes (not a URL
    /// sentinel). Returns `None` for the deferred-URL sentinel case.
    pub fn mime_if_real(&self) -> Option<&str> {
        if self.mime == "url" {
            None
        } else {
            Some(&self.mime)
        }
    }
}

/// Resolve a single image source.
///
/// `src` may be:
/// - `"-"`      -> read raw bytes from stdin
/// - `http(s)://` -> downloaded to bytes when `fetch_url` is true; otherwise left as a URL
/// - anything else -> treated as a local file path
///
/// When `src` is a URL and `fetch_url` is false, the image is **not** downloaded;
/// instead `ImageInput` is returned with `bytes` empty and `mime` set to "url".
/// Adapters that can pass a remote URL directly (OpenAI/Azure/Gemini/Anthropic) will
/// then use `src` as-is. Adapters that require base64 must download first.
pub async fn resolve_image(src: &str, fetch_url: bool, client: &reqwest::Client) -> Result<ImageInput> {
    if src == "-" {
        use std::io::Read;
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .context("reading image from stdin")?;
        let mime = detect_mime(&buf)
            .ok_or_else(|| anyhow!("cannot infer mime type from stdin bytes"))?;
        return Ok(ImageInput { mime, bytes: buf });
    }

    if let Some(url) = http_url(src) {
        if !fetch_url {
            // Defer fetching; signal URL mode with a sentinel.
            return Ok(ImageInput {
                mime: "url".to_string(),
                bytes: url.into_bytes(),
            });
        }
        let bytes = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("downloading image {url}"))?
            .error_for_status()
            .with_context(|| format!("image URL returned error {url}"))?
            .bytes()
            .await
            .with_context(|| format!("reading image bytes {url}"))?
            .to_vec();
        let mime = detect_mime(&bytes)
            .ok_or_else(|| anyhow!("cannot infer mime type from downloaded image"))?;
        return Ok(ImageInput { mime, bytes });
    }

    // Local path.
    let path = Path::new(src);
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading image file {}", path.display()))?;
    let mime = detect_mime(&bytes)
        .or_else(|| mime_from_ext(path))
        .ok_or_else(|| anyhow!("cannot infer mime type for {}", path.display()))?;
    Ok(ImageInput { mime, bytes })
}

/// Whether this ImageInput is a deferred URL (not yet downloaded).
pub fn is_url_sentinel(img: &ImageInput) -> Option<&str> {
    if img.mime == "url" {
        std::str::from_utf8(&img.bytes).ok()
    } else {
        None
    }
}

/// If `s` is an http(s) URL, return it; else None.
fn http_url(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(s.to_string())
    } else {
        None
    }
}

/// Detect MIME from magic bytes.
fn detect_mime(bytes: &[u8]) -> Option<String> {
    infer::get(bytes).map(|t| t.mime_type().to_string())
}

/// Fallback MIME detection from file extension.
fn mime_from_ext(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => return None,
    }
    .to_string())
}

/// Validate that a mime is one the vision APIs accept.
pub fn is_supported_mime(mime: &str) -> bool {
    matches!(mime, "image/png" | "image/jpeg" | "image/webp" | "image/gif")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal valid PNG header (8-byte signature + IHDR chunk start).
    const PNG_BYTES: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
        0x00, 0x00, 0x00, 0x0D, // IHDR length
        0x49, 0x48, 0x44, 0x52, // "IHDR"
    ];

    #[test]
    fn detect_png() {
        assert_eq!(detect_mime(PNG_BYTES).as_deref(), Some("image/png"));
    }

    #[test]
    fn ext_fallback() {
        assert_eq!(
            mime_from_ext(Path::new("/tmp/x.jpeg")).as_deref(),
            Some("image/jpeg")
        );
        assert!(mime_from_ext(Path::new("/tmp/x.bin")).is_none());
    }

    #[test]
    fn url_sentinel_roundtrip() {
        let img = ImageInput {
            mime: "url".into(),
            bytes: b"https://example.com/a.png".to_vec(),
        };
        assert_eq!(is_url_sentinel(&img), Some("https://example.com/a.png"));
    }

    #[test]
    fn data_uri_format() {
        let img = ImageInput {
            mime: "image/png".into(),
            bytes: vec![0x89, 0x50, 0x4E, 0x47],
        };
        assert!(img.data_uri().starts_with("data:image/png;base64,"));
        assert_eq!(img.b64(), "iVBORw==");
    }

    #[test]
    fn supported_mime_check() {
        assert!(is_supported_mime("image/png"));
        assert!(is_supported_mime("image/webp"));
        assert!(!is_supported_mime("image/bmp"));
        assert!(!is_supported_mime("url"));
    }
}
