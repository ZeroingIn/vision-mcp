---
name: vision-mcp
description: How to use the vision-mcp MCP tools (view_image, extract_text, compare_images) to give text-only LLM agents the ability to see images. Use whenever the user references an image (file path, URL, screenshot, photo, chart, diagram, OCR, "看这张图", "截图里", "对比这两张"), needs text extracted from an image, needs to compare images, or describes a UI/visual that the agent cannot directly perceive. Also use when an error message references on-screen elements the agent should verify.
---

# vision-mcp — seeing images via MCP

`vision-mcp` is an MCP server that proxies image requests to a multimodal model
the user has configured. As a text-only agent, you have no vision of your own;
these three tools are your eyes. Use them whenever the task involves image
content you cannot read as text.

This skill covers **when to call which tool and how to fill the arguments**.
For install / config / secrets, read the project's `README.md`.

## When to trigger

Call a vision tool when ANY of these are true:

- The user gives an image: a file path ending in `.png/.jpg/.jpeg/.webp/.gif`, an
  `http(s)://` image URL, or says "看这张图/截图/这张照片".
- The user references something visual you can't see: "截图里的按钮", "这个报错
  弹窗", "图表显示的数字", "界面右上角".
- The task needs OCR: "提取图片里的文字", "这张图写了什么", "把发票转录成文本".
- The task needs comparison: "对比 v1 和 v2", "这两张图有什么不同", "改版前后".
- An error you're debugging references on-screen UI you should verify visually.

Do NOT trigger for: pure-text files, code the user pasted, or when the user
only describes an image conceptually without wanting you to actually look.

## Tool selection

| Task | Tool | Notes |
|---|---|---|
| Describe / understand / answer a question about an image | `view_image` | Default choice. Returns a structured Markdown description: image type → type-specific detail → verbatim text. |
| Get the text out of an image (OCR) | `extract_text` | Pure transcription, preserves layout. Use when the user wants the *words*, not a description. |
| Diff two or more images | `compare_images` | Requires **≥2** images. Returns a side-by-side diff with bolded differences. |
| Structured yes/no or field-extraction from an image | `extract_text` with `format=json` | Returns `{"text","type"}`. Use when you need to parse the result programmatically. |

Rule of thumb: **want words → `extract_text`; want understanding → `view_image`;
want a diff → `compare_images`.** When unsure, `view_image` is the safe default
— its output includes any visible text anyway.

## Filling the arguments

### `images` / `image` (required)

Accepts a local path, an `http(s)://` URL, or `"-"` (stdin). Multiple images
allowed for `view_image` and `compare_images`.

### `fetch_url` — the most important flag to get right

URL images can be sent two ways: pass the URL directly (provider downloads it),
or `vision-mcp` downloads it to base64 first (`fetch_url: true`).

- **OpenAI-compatible providers** (OpenAI, Azure, Kimi, DeepSeek, Qwen-VL):
  accept URL images directly → **leave `fetch_url` unset** (faster, no double
  transfer).
- **Anthropic (Claude)**: does NOT accept remote URLs → **set `fetch_url: true`**
  or the call errors with "URL images not supported".
- **Unknown provider / private LAN endpoint**: set `fetch_url: true` to be safe
  (the provider may not be able to reach the URL, or the URL may be
  intranet-only).

If a `view_image`/`extract_text` call on a URL returns an error mentioning
"URL", "image_url", or "unsupported", retry once with `fetch_url: true` before
giving up.

### `detail` — cost vs accuracy (OpenAI only)

Controls the image-processing resolution. Ignored by Anthropic.

- `low` (or omit on tiny images): cheapest, fastest. Fine for "what is this a
  picture of" on large clear images.
- `high`: more tokens, but can read small text / fine UI details. **Use `high`
  whenever the task involves reading text, inspecting a small UI element, or
  comparing fine details.** The extra cost is worth not failing the task.
- `auto` (default): provider decides. Reasonable, but if a first call misses
  detail, retry with `high`.

### `max_tokens`

Leave unset to use the config default. Only lower it for trivial tasks
("is there a red button?" → 256 is plenty). **Never set it below 128** for
`view_image`/`compare_images` — the structured description gets truncated and
becomes useless.

### `prompt`

Optional instruction / question. If omitted, a strong built-in default is used
(type-aware description / pure OCR / diff). Provide a `prompt` when the user has
a *specific* question ("is the submit button enabled?", "what's the error code
in the top-right?") — it gets answered after the description.

### `model` / `profile`

Override the configured model or switch provider profile per-call. Use only when
the user explicitly asks ("try this with claude", "use the local model").

## Handling errors

- **"URL images not supported"** → retry with `fetch_url: true` (Anthropic).
- **HTTP 429 / "rate limited" / 5xx** → transient; wait briefly and retry once.
- **"base_url is not set" / "model is not set" / "api_key = (unset)"** →
  configuration error, not retryable. Tell the user to run
  `vision-mcp config show` and fix `config.toml` (or set the `VMCP_*` env vars).
- **"cannot infer mime type"** → the input wasn't a recognizable image, or the
  URL returned HTML (often a login page). Check the path/URL.
- **`compare_images` with <2 images** → the tool returns an error string; ask
  the user for the second image rather than retrying.

## Output

All tools return a single string. `view_image`/`compare_images` → Markdown.
`extract_text` → plain transcription (or a JSON object when `format=json`).
Treat the returned text as the ground truth about the image — cite it, don't
paraphrase critical details (error codes, file paths, numbers) from memory.
