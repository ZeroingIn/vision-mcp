# vision-mcp

A multimodal **vision proxy** MCP server + CLI that lets text-only LLM agents
see images. Point it at any multimodal model you already have access to (OpenAI
`gpt-4o`, Anthropic Claude, Kimi, DeepSeek, Qwen-VL, a local Ollama / vLLM, …)
and your text-only agent gains `view_image`, `extract_text`, and `compare_images`
tools — speaking the standard [Model Context Protocol][mcp].

> Why? Many agent hosts (Claude Code, Cursor, ZCode, …) ship excellent text
> reasoning but no built-in vision. `vision-mcp` bridges that gap by routing
> image requests to *your own* multimodal endpoint, so you keep control of the
> model, the cost, and the data path.

[mcp]: https://modelcontextprotocol.io/

---

## Features

- **Three MCP tools** — `view_image`, `extract_text` (OCR), `compare_images`.
- **One config, any provider** — OpenAI-compatible Chat Completions *or*
  Anthropic Messages, via the [`genai`][genai] crate. Switch models by editing
  one line.
- **Cross-platform** — one codebase builds for Windows (MSVC x86_64), macOS
  (arm64 + x86_64), and Linux (x86_64 glibc + aarch64 musl). TLS via
  `rustls`+`ring` (no CMake/NASM/OpenSSL), so it compiles with just `rustc` + a
  C compiler on every target.
- **Two transports** — `stdio` (the MCP default) and streamable `http`
  (axum, for remote / multi-client setups).
- **Portable, single-file binary** — `config.toml` lives next to the executable,
  no `%APPDATA%` / registry / install step. On Windows the MSVC + static-CRT
  build is a true single `.exe` with zero runtime DLL dependencies.
- **Secret-safe by design** — real `config.toml` is git-ignored; ship
  `config.example.toml` instead; every setting has a `VMCP_*` env-var fallback.
- **Local-path / URL / stdin** image inputs; optional `fetch_url` to base64
  remote images for providers that lack URL image support.
- **System-proxy bypass** — `vision-mcp` always talks directly to your
  `base_url`, so a local Clash/V2Ray on `:7897` returning 502 for LAN hosts
  won't break calls.

[genai]: https://crates.io/crates/genai

---

## Quick start

### 1. Get the binary

Download a release `.exe` **or** build from source (see [Build](#build)).
Put `vision-mcp.exe` anywhere you like.

### 2. Configure

Copy the example next to the binary and edit it:

```bat
copy config.example.toml config.toml
notepad config.toml
```

Minimal `config.toml`:

```toml
active = "default"

[profiles.default]
provider = "openai"
base_url = "https://api.openai.com/v1"
api_key = "sk-YOUR-API-KEY-HERE"
model = "gpt-4o"
max_tokens = 1024
detail = "auto"
timeout_secs = 60
```

> ⚠️ **Never commit your real `config.toml`.** It is git-ignored by default.
> To share config, share `config.example.toml`. See
> [Keeping secrets out of git](#keeping-secrets-out-of-git).

### 3. Register with your MCP host

Add `vision-mcp` to your host's MCP config. Example for ZCode / Claude Code /
Cursor `mcp.json`-style registration:

```jsonc
{
  "mcpServers": {
    "vision-mcp": {
      "command": "C:/path/to/vision-mcp.exe",
      "args": ["serve"]
    }
  }
}
```

Restart the host. Your agent can now call `view_image`, `extract_text`, and
`compare_images`.

### 4. (Optional) Use it as a plain CLI

```bat
vision-mcp describe photo.jpg --prompt "What breed is this dog?"
vision-mcp ocr screenshot.png --format markdown
vision-mcp compare v1.png v2.png
vision-mcp config show
```

Run `vision-mcp --help` for the full surface.

---

## Configuration

`config.toml` is resolved from (first wins):

1. `$VMCP_CONFIG` — explicit path to a config file.
2. `config.toml` **next to the executable** (portable layout — the default).
3. `./config.toml` in the current directory (fallback if the exe path can't be
   determined).

Run `vision-mcp config path` to see which file is in effect.

### Profiles

Define multiple providers under `[profiles.<name>]` and switch with the `active`
key, the `VMCP_ACTIVE` env var, or `--profile`:

```toml
active = "default"

[profiles.default]
provider = "openai"
base_url = "https://api.openai.com/v1"
api_key  = "sk-..."
model    = "gpt-4o"

[profiles.local]
provider = "openai"
base_url = "http://localhost:11434/v1"   # Ollama
api_key  = ""                            # Ollama needs no key
model    = "llama3.2-vision"
```

### Providers

| `provider`     | Wire format                                | Use when                                   |
|----------------|--------------------------------------------|--------------------------------------------|
| `"openai"`     | OpenAI Chat Completions (`/v1/chat/completions`) | OpenAI, Azure, Kimi, DeepSeek, Qwen-VL, Ollama, vLLM, LocalAI |
| `"anthropic"`  | Anthropic Messages (`/v1/messages`)        | Claude 3.5 / 4 vision models               |
| `"auto"`       | Detect from `base_url`                     | `anthropic.com` / `/v1/messages` → Anthropic; else OpenAI |

### Environment-variable overrides

Every field can be set via env var; env wins over the file (so CI / shared
machines can inject secrets without touching the file):

| Env var            | Overrides                  |
|--------------------|----------------------------|
| `VMCP_CONFIG`      | config file path           |
| `VMCP_ACTIVE`      | active profile name        |
| `VMCP_PROVIDER`    | `provider`                 |
| `VMCP_BASE_URL`    | `base_url`                 |
| `VMCP_API_KEY`     | `api_key`                  |
| `VMCP_MODEL`       | `model`                    |
| `VMCP_MAX_TOKENS`  | `max_tokens`               |
| `VMCP_DETAIL`      | `detail`                   |
| `VMCP_TIMEOUT`     | `timeout_secs`             |
| `VMCP_INSTRUCTION` | default instruction text   |

Precedence (highest → lowest): **CLI flag** > **`VMCP_*` env** > **profile file**
> **`Config::default`**.

### `base_url` trailing-slash note

`vision-mcp` normalizes `base_url` to end with `/` before joining
`chat/completions` (or `messages`). You can write `…/v1` or `…/v1/` — both work.
Do **not** append `/chat/completions` yourself.

---

## MCP tools

| Tool            | Args                                                | Returns                          |
|-----------------|-----------------------------------------------------|----------------------------------|
| `view_image`    | `images[]`, `prompt?`, `model?`, `detail?`, `fetch_url?`, `max_tokens?`, `profile?` | Markdown description: image type → type-specific detail → verbatim text |
| `extract_text`  | `image`, `prompt?`, `format?` (`text`/`markdown`/`json`), … | OCR transcription; `format=json` returns `{"text","type"}` |
| `compare_images`| `images[]` (≥2), `prompt?`, …                       | Side-by-side diff with bolded differences |

`images` entries accept: a local path, an `http(s)` URL, or `"-"` (stdin).
Set `fetch_url: true` to download URL images to base64 first (needed for
providers that don't accept remote URLs, e.g. some Anthropic setups).

---

## Build

vision-mcp is **cross-platform**: one codebase, no platform-specific code, and a
TLS stack (`rustls` + `ring`) that compiles with just `rustc` + a C toolchain —
**no CMake, no NASM, no OpenSSL**. The matrix below is what the CI workflow
(`.github/workflows/build.yml`) produces on every release tag.

| Platform | Target triple | Runner | Notes |
|---|---|---|---|
| Windows x86_64 | `x86_64-pc-windows-msvc` | `windows-latest` | Single-file `.exe`, static CRT (no runtime DLL). |
| macOS arm64 | `aarch64-apple-darwin` | `macos-14` | Apple Silicon; universal-ready. |
| macOS x86_64 | `x86_64-apple-darwin` | `macos-13` | Intel. |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `ubuntu-latest` | glibc. |
| Linux aarch64 | `aarch64-unknown-linux-musl` | `ubuntu-latest` | Fully static (musl); runs on any Linux. |

### TLS note (why no `aws-lc-rs`)

The default `reqwest` rustls feature pulls in `aws-lc-rs`, a C library that
needs CMake + NASM (Windows) to build and breaks cross-compilation. We disable
it: `reqwest` is built with `rustls-no-provider`, and we explicitly select the
`ring` CryptoProvider (`rustls` `ring` feature + `install_default()` at
runtime in `adapter::http_client`). `ring` ships prebuilt assembly / fallback C
that compiles on every target with just a C toolchain, so the whole project
stays pure-Rust-buildable across all five targets above.

### Prerequisites by platform

**Windows (MSVC, recommended):** Rust stable + **VS 2022 Build Tools** with the
*Desktop development with C++* workload (installs `link.exe` + Windows SDK):

```bat
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
rustup default stable-x86_64-pc-windows-msvc
```

For a **single-file `.exe` with no runtime DLL** (static CRT), also create
`.cargo/config.toml` with:
```toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```
(The released Windows zip ships this file pre-made.) The result runs on any
Windows 10+ machine with **zero** extra DLLs.

**macOS:** Rust stable (the host toolchain targets the running arch natively):
```bash
rustup default stable
rustup target add aarch64-apple-darwin   # if building for Apple Silicon from Intel, or vice-versa
```

**Linux:** Rust stable + a C compiler (`gcc` or `musl-gcc`):
```bash
rustup default stable
# for the static musl aarch64 build:
sudo apt-get install -y musl-tools gcc-aarch64-linux-gnu
rustup target add aarch64-unknown-linux-musl
```

### Build it

```bash
cargo build --release
# -> target/release/vision-mcp[.exe]
```

To cross-build for a non-host target, add the target and pass `--target`:
```bash
rustup target add aarch64-apple-darwin
cargo build --release --target aarch64-apple-darwin
```

The release profile already enables `lto = true`, `codegen-units = 1`, and
`strip = true` for a smaller binary.

### Release artifacts (CI)

Pushing a tag (`git tag v0.1.0 && git push origin v0.1.0`) triggers
`.github/workflows/build.yml`, which builds all five targets and attaches a
compressed archive (`.zip` for Windows, `.tar.gz` otherwise) to the GitHub
Release. Each archive contains the binary + `README.md` + `LICENSE` +
`config.example.toml` (+ the `.cargo/config.toml` for the Windows static-CRT
build). Download, extract, copy `config.example.toml` → `config.toml`, fill in
your key, and run.

### Run the tests

```bash
cargo test
```

Integration tests (`tests/adapters.rs`) exercise the full describe path against
an in-process `wiremock` mock server speaking the OpenAI Chat Completions shape —
no real API key needed.

### Binary size

A release build is roughly 6–12 MB depending on platform. The mass comes from
the unavoidable runtime: `rmcp` + `axum` + `tokio` (MCP + HTTP server), `genai`
+ `reqwest` + `rustls` + `ring` (HTTPS to providers), and `windows-sys`
bindings (Windows only). LTO + strip are already on. To shrink further you can
feature-gate the HTTP transport off (drop `axum` /
`transport-streamable-http-server`) if you only need stdio — that is the single
biggest cut and is left as a TODO.

---

## Keeping secrets out of git

`vision-mcp` is designed so a real API key never has to live in version control.
The shipped `.gitignore` already excludes `config.toml`, `*.exe`, `*.dll`,
`/target`, and `/.cargo/config.toml`.

**Recommended workflow** (synthesized from [OWASP Secrets Management][owasp]
and [12-Factor Config][12f]):

1. **Ship `config.example.toml` with placeholders** — `api_key = "sk-YOUR-API-KEY-HERE"`.
   This is what goes in git. (Done — see the file in this repo.)
2. **Keep your real `config.toml` local only** — it's git-ignored. Verify with:
   ```bat
   git check-ignore config.toml     :: should print "config.toml"
   ```
3. **Prefer env vars on shared/CI machines** — set `VMCP_API_KEY` (and
   `VMCP_BASE_URL`, `VMCP_MODEL`, …) in the environment instead of writing them
   to disk. 12-Factor prefers env vars because they're "language- and
   OS-agnostic" and hard to accidentally commit; OWASP cautions that env vars
   are *visible to all processes* and may leak into logs/dumps, so for
   high-value keys prefer a real secret store (Vault, AWS Secrets Manager,
   Azure Key Vault, `docker secrets`, …) and have `vision-mcp` read the key
   from a file via `VMCP_API_KEY="$(cat /run/secrets/vmcp_key)"`.
4. **Add a pre-commit secret scanner** to catch accidents before they land:
   - [detect-secrets][detect-secrets] (Yelp) — mature, ~20 built-in signatures,
     the one OWASP names explicitly.
   - [gitleaks][gitleaks] / [trufflehog][trufflehog] — broader coverage, good
     for CI.
   - [git-secrets][git-secrets] — AWS's lightweight git filter.

   Example `detect-secrets` baseline:
   ```bat
   pip install detect-secrets
   detect-secrets scan --baseline .secrets.baseline
   :: add a pre-commit hook:
   detect-secrets-hook --baseline .secrets.baseline
   ```
5. **If a key leaks, rotate it immediately** — assume anything pushed to a
   public repo (even briefly) is compromised. Scanner-found keys in git history
   stay retrievable even after `git rm`.

[owasp]: https://cheatsheetseries.owasp.org/cheatsheets/Secrets_Management_Cheat_Sheet.html
[12f]: https://12factor.net/config
[detect-secrets]: https://github.com/Yelp/detect-secrets
[gitleaks]: https://github.com/gitleaks/gitleaks
[trufflehog]: https://github.com/trufflesecurity/trufflehog
[git-secrets]: https://github.com/awslabs/git-secrets

---

## Project layout

```
vision-mcp/
├── Cargo.toml             # manifest (deps, release profile)
├── Cargo.lock             # pinned dep versions (committed for a bin crate)
├── config.example.toml    # placeholder config — committed
├── config.toml            # YOUR real config — git-ignored, never committed
├── LICENSE                # Apache-2.0
├── README.md              # this file
├── .gitignore             # keeps secrets/binaries out of git
├── examples/
│   └── red32.png          # tiny test image for smoke tests
├── src/
│   ├── main.rs            # entrypoint: tracing init + CLI dispatch
│   ├── lib.rs             # crate root, module declarations
│   ├── cli.rs             # clap CLI: serve / describe / ocr / compare / config
│   ├── config.rs          # config file + env + override resolution
│   ├── adapter/mod.rs     # genai Client builder + vision request assembly
│   ├── core.rs            # default prompts + describe() + strict-JSON logic
│   ├── image.rs           # path/URL/stdin → ImageInput (bytes + mime)
│   └── mcp.rs             # rmcp server: view_image / extract_text / compare_images
└── tests/
    └── adapters.rs        # integration tests against a wiremock mock server
```

---

## Troubleshooting

- **`502 Bad Gateway` / connection refused to a LAN `base_url`** — you almost
  certainly have a system proxy (Clash/V2Ray on `:7897`) intercepting LAN
  traffic. `vision-mcp` calls `.no_proxy()` on its HTTP client, so this should
  not happen; if it does, check that you're running a current build and that
  the proxy isn't pinned via `HTTP_PROXY`/`HTTPS_PROXY` in a way reqwest
  honors despite `.no_proxy()`.
- **`base_url` seems to lose `/v1`** — make sure you wrote `…/v1` (vision-mcp
  appends the `/`). Don't write `…/v1/chat/completions`.
- **Anthropic returns "URL images not supported"** — set `fetch_url: true` so
  the image is downloaded and sent as base64.
- **`reasoning_content` but empty `content`** — handled automatically; the
  adapter falls back to `reasoning_content` (common with some OpenAI-compatible
  reasoning models).
- **`vision-mcp config show` says `api_key = (unset)`** — your `config.toml`
  has an empty `api_key` and no `VMCP_API_KEY` env var. Set one.

---

## License

Apache-2.0. See [LICENSE](LICENSE).
