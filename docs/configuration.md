# Configuration

All variables are optional; defaults are shown. Set them in your shell environment, or at `~/.claudette/.env` (the canonical persistent location).

Claudette intentionally does **not** auto-load `.env` from the current working directory or its parents — that would let a shared project smuggle `OLLAMA_HOST`, `GITHUB_TOKEN`, etc. into the agent without the user noticing. For per-project overrides, use `direnv` or `source path/to/.env` before invoking.

## Core

| Variable | Default | Purpose |
|----------|---------|---------|
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama API endpoint. Honoured exactly like Ollama itself. |
| `CLAUDETTE_ALLOW_REMOTE_OLLAMA` | unset | Set to `1` to silence the startup warning when `OLLAMA_HOST` is non-loopback. Default posture is local-only. |
| `CLAUDETTE_MODEL` | `qwen3.5:4b` (Auto preset) | Brain model override. |
| `CLAUDETTE_NUM_CTX` | `16384` | Brain context window in tokens. |
| `CLAUDETTE_NUM_PREDICT` | `6144` | Max output tokens per request. |
| `CLAUDETTE_COMPACT_THRESHOLD` | `1000000` | Auto-compaction trigger (estimated tokens). Default makes auto-compact a no-op for typical 16K–128K context windows; set to `12000` (or a fraction of your `num_ctx`) on tight contexts. |
| `CLAUDETTE_SOFT_COMPACT_THRESHOLD` | unset | Optional intermediate compaction tier. Fires below the hard threshold and preserves 12 recent messages instead of 4 — useful on long real-world sessions with 35B+ brains where the hard 1M default never triggers but turns pay hundreds of K input tokens. Set e.g. `200000`. |
| `CLAUDETTE_MAX_ITERATIONS` | `40` | Per-turn (model → tool → result) loop ceiling. Lower it (e.g. `15`) to fail-fast on small-model spirals; raise it for legitimate long tool chains. |
| `CLAUDETTE_SESSION` | `~/.claudette/sessions/last.json` | Override the session file path. |
| `CLAUDETTE_MEMORY` | `~/.claudette/CLAUDETTE.MD` | Override the path Claudette loads user-memory from. |
| `CLAUDETTE_OPENAI_COMPAT` | unset | Set to `1` to talk to an OpenAI-compatible server (LM Studio, vLLM, llama.cpp's `--api`) instead of native Ollama. Brain calls switch to `/v1/chat/completions`; recall embeddings switch to `/v1/embeddings`. `OLLAMA_HOST` doubles as the compat-server URL. |
| `CLAUDETTE_SKIP_OLLAMA_PROBE` | unset | Set to `1` to skip the Ollama startup probe (CI / offline). |
| `CLAUDETTE_SKIP_LM_STUDIO_PROBE` | unset | Set to `1` to skip the LM Studio probe (only used when `CLAUDETTE_OPENAI_COMPAT=1`). The probe checks `/v1/models` returns a non-empty model list — set this if you load models post-launch. |
| `CLAUDETTE_FALLBACK_BRAIN_MODEL` | `qwen3.5:9b` (Auto preset) | Brain to fall back to on stuck signals. |
| `CLAUDETTE_LIVE_GOOGLE` | unset | Set to `1` to run live Google integration tests via `cargo test --ignored`. Never set in CI. |
| `CLAUDETTE_WORKSPACE` | unset | Extra read roots outside `$HOME`, colon-separated on Unix, semicolon-separated on Windows. Example: `D:\dev\claudette` for developing Claudette itself. Reads under `$HOME` and under a `$HOME`-rooted CWD are always allowed regardless. |

## Codet (code-generation sidecar)

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_CODER_MODEL` | `qwen3-coder:30b` | Coder model. Set to `qwen2.5-coder:14b` on RAM-constrained hosts. |
| `CLAUDETTE_CODER_NUM_CTX` | `49152` | Coder context window. Drop to `16384` on 32 GB RAM boxes. |
| `CLAUDETTE_CODER_NUM_PREDICT` | `12288` | Max output tokens the coder can emit in one call. |
| `CLAUDETTE_VALIDATE_CODE` | `true` | Enable/disable Codet auto-validation after `generate_code`. |

## Tokens (per-tool)

| Variable | Purpose |
|----------|---------|
| `BRAVE_API_KEY` | Brave Search API key — required for `web_search`. |
| `GITHUB_TOKEN` | GitHub PAT — required for the `github` tool group. Falls back to `CLAUDETTE_GITHUB_TOKEN` if unset. |
| `TELEGRAM_BOT_TOKEN` | Bot token from `@BotFather` — required for `--telegram`. |
| `CLAUDETTE_GOOGLE_CLIENT_ID` | Google OAuth client ID — required for `--auth-google` + the Calendar / Gmail tool groups. Falls back to `GOOGLE_CLIENT_ID`, or to `~/.claudette/secrets/google_oauth_client.json`. |
| `CLAUDETTE_GOOGLE_CLIENT_SECRET` | Google OAuth client secret. Same fallback chain as the client ID. |
| `VESTIGE_API_BASE` | Override for the vestige.fi Algorand API (`markets` group). |

All tokens also support file-based fallback: save them to `~/.claudette/secrets/<name>.token` (for example `github.token`, `telegram.token`, `brave.token`). Environment variables win over files when both are present.

## Voice

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_WHISPER_BIN` | `whisper-cli` on PATH | Path to the `whisper.cpp` binary. |
| `CLAUDETTE_WHISPER_MODEL` | `~/.claudette/models/ggml-large-v3-turbo.bin` | Path to the Whisper GGML model file. |

## Cross-session recall

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_RECALL_DISABLE` | unset | Set to `1` to disable post-turn recall indexing entirely (privacy / no embed model available). |
| `CLAUDETTE_RECALL_MODEL` | `nomic-embed-text` | Embed model id. Under `CLAUDETTE_OPENAI_COMPAT=1`, set to whatever embedding model you've loaded in LM Studio (e.g. `text-embedding-nomic-embed-text-v1.5`). |
| `CLAUDETTE_RECALL_DB` | `~/.claudette/recall.sqlite` | Override the recall DB path (mostly useful in tests). |

## Sub-agent tuning

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_RESEARCHER_MODEL` | inherits brain | Override the Researcher agent's model. |
| `CLAUDETTE_GITOPS_MODEL` | inherits brain | Override the GitOps agent's model. |
| `CLAUDETTE_RESEARCHER_MAX_ITER` | `10` | Hard cap on Researcher tool calls per delegation. |
| `CLAUDETTE_GITOPS_MAX_ITER` | `8` | Hard cap on GitOps tool calls per delegation. |
| `CLAUDETTE_TELEGRAM_CHAT` | unset | Comma-separated chat-ID allowlist for Telegram bot. |
