# Configuration

All variables are optional; defaults are shown. Set them in your shell environment, or at `~/.claudette/.env` (the canonical persistent location).

Claudette intentionally does **not** auto-load `.env` from the current working directory or its parents — that would let a shared project smuggle `OLLAMA_HOST`, `GITHUB_TOKEN`, etc. into the agent without the user noticing. For per-project overrides, use `direnv` or `source path/to/.env` before invoking.

## Installer (`install.sh` / `install.ps1`)

These are read by the one-line install scripts, not by the binary itself:

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_FLAVOR` | `lean` | `full` downloads the prebuilt `--features integrations` binary (Telegram, Gmail, Calendar, voice, morning briefing) instead of the lean coding-only one — no Rust toolchain needed. |
| `CLAUDETTE_VERSION` | latest | Pin a release version, e.g. `0.16.0`. |
| `CLAUDETTE_INSTALL_DIR` | `~/.local/bin` (Unix) / `%LOCALAPPDATA%\Programs\claudette` (Windows) | Install location. |
| `CLAUDETTE_NO_MODIFY_PATH` | unset | (Unix only) Set to anything to suppress the PATH hint. |

## Core

| Variable | Default | Purpose |
|----------|---------|---------|
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama API endpoint. Honoured exactly like Ollama itself. |
| `CLAUDETTE_ALLOW_REMOTE_OLLAMA` | unset | Set to `1` to silence the startup warning when `OLLAMA_HOST` is non-loopback. Default posture is local-only. |
| `CLAUDETTE_OFFLINE` | unset | Set to `1` (or pass `--offline`) to **enforce the air-gap**: hard-block every outbound network call except the local model backend + loopback. See [Enforced offline mode](#enforced-offline-mode---offline) below. |
| `CLAUDETTE_MODEL` | `qwen3.5:4b` (Auto preset) | Brain model override. See [Recommended brain](#recommended-brain-measured-2026-07-11) below for the measured per-GPU picks. |
| `CLAUDETTE_NUM_CTX` | `16384` | Brain context window in tokens. |
| `CLAUDETTE_NUM_PREDICT` | `6144` | Max output tokens per request. |
| `CLAUDETTE_COMPACT_THRESHOLD` | `num_ctx / 2` (adaptive) | Auto-compaction trigger (estimated tokens). Unset → half the active brain's `num_ctx`, clamped to `[4000, 1000000]`, so a real 16K–128K window compacts *before* it overflows. Pin an exact value like `12000` to override; the `1000000` cap is only the ceiling for enormous windows. |
| `CLAUDETTE_SOFT_COMPACT_THRESHOLD` | unset | Optional intermediate compaction tier. Fires below the hard threshold and preserves 12 recent messages instead of 4 — useful on long real-world sessions with 35B+ brains where you want gentler compaction before the hard `num_ctx / 2` threshold fires. Set e.g. `200000`. |
| `CLAUDETTE_MAX_ITERATIONS` | `40` | Per-turn (model → tool → result) loop ceiling. Lower it (e.g. `15`) to fail-fast on small-model spirals; raise it for legitimate long tool chains. |
| `CLAUDETTE_SESSION` | `~/.claudette/sessions/last.json` | Override the session file path. |
| `CLAUDETTE_MEMORY` | `~/.claudette/CLAUDETTE.MD` | Override the path Claudette loads user-memory from. |
| `CLAUDETTE_OPENAI_COMPAT` | unset | Set to `1` to talk to an OpenAI-compatible server (LM Studio, vLLM, llama.cpp's `--api`) instead of native Ollama. Brain calls switch to `/v1/chat/completions`; recall embeddings switch to `/v1/embeddings`. `OLLAMA_HOST` doubles as the compat-server URL. |
| `CLAUDETTE_SKIP_OLLAMA_PROBE` | unset | Set to `1` to skip the Ollama startup probe (CI / offline). |
| `CLAUDETTE_SKIP_LM_STUDIO_PROBE` | unset | Set to `1` to skip the LM Studio probe (only used when `CLAUDETTE_OPENAI_COMPAT=1`). The probe checks `/v1/models` returns a non-empty model list — set this if you load models post-launch. |
| `CLAUDETTE_FALLBACK_BRAIN_MODEL` | `qwen3.5:9b` (Auto preset) | Brain to fall back to on stuck signals. |
| `CLAUDETTE_WORKSPACE` | unset | Extra read roots outside `$HOME`, colon-separated on Unix, semicolon-separated on Windows. Example: `D:\dev\claudette` for developing Claudette itself. Reads under `$HOME` and under a `$HOME`-rooted CWD are always allowed regardless. |

### Recommended brain (measured 2026-07-11)

The shipping default stays `qwen3.5:4b` — it runs anywhere (8 GB GPU or plain CPU)
and scored 45/50 (90%) on the 50-task battery. On a **16 GB GPU with LM Studio**, the
measured best is the byteshape MTP quant of qwen3.6-35b (50/50 on the same battery,
~70–76 tok/s, fully VRAM-resident):

```bash
export CLAUDETTE_OPENAI_COMPAT=1
export OLLAMA_HOST=http://localhost:1234
export CLAUDETTE_MODEL=byteshape/qwen3.6-35b-a3b-mtp
```

Rollback if it misbehaves: `CLAUDETTE_MODEL=qwen3.6-35b-a3b@q3_k_xl` (the previous
16 GB default — 47/50, known-good). Load commands, per-tier table, and
choosing-a-model guidance: [`hardware.md`](hardware.md#which-model-for-which-gpu-measured).

### Backend quirks: LM Studio variant suffix

LM Studio exposes models with a `@<quant>` suffix in `/v1/models` — for example `qwen3.6-35b-a3b@q3_k_xl` rather than the bare `qwen3.6-35b-a3b`. If you set `CLAUDETTE_MODEL=qwen3.6-35b-a3b` (bare id) against LM Studio, the server treats it as an unknown id, attempts a JIT-load for a different variant, and (when VRAM is tight) returns HTTP 400 `{"error":"Model is unloaded."}`. **Use the exact id from `lms ps` or `/v1/models`** when targeting LM Studio — e.g. `CLAUDETTE_MODEL=qwen3.6-35b-a3b@q3_k_xl`. llama.cpp's `llama-server` (and the MTP fork) ignores the `model` field entirely since it only has one loaded, so the bare id works there.

### Backend quirks: streaming on the OpenAI-compat path

Under `CLAUDETTE_OPENAI_COMPAT=1` the brain request sends `stream: true`, so the
server replies with Server-Sent Events (`text/event-stream`) and claudette
renders tokens as they arrive instead of waiting for the whole reply — the same
behaviour as the native Ollama path. It also sets `stream_options.include_usage`,
which asks the server to append a final chunk carrying the real
`prompt_tokens`/`completion_tokens`; LM Studio honours this, and servers that
don't recognise the option simply ignore it (token counts then show as `0`).
If a server ignores `stream: true` and returns a single JSON object (no SSE
framing), claudette detects the non-SSE `Content-Type` and transparently parses
it as a non-streaming response — so an older or minimal backend still works,
just without token-by-token output.

### Backend quirks: brain and embeddings share `OLLAMA_HOST`

Both the brain (`/v1/chat/completions`) and recall (`/v1/embeddings`) resolve to the same `OLLAMA_HOST`. There is no separate `CLAUDETTE_RECALL_HOST` knob. If you run a chat-only server (e.g. an MTP llama-server with no `--embeddings`) you'll see `recall: /v1/embeddings HTTP 501 Not Implemented` from `--doctor` and from `/recall`. Either (a) set `CLAUDETTE_RECALL_DISABLE=1`, or (b) load the embedding model on the same endpoint as the brain (LM Studio supports loading both simultaneously if VRAM allows).

### Enforced offline mode (`--offline` / `CLAUDETTE_OFFLINE`)

`--offline` (or `CLAUDETTE_OFFLINE=1`) turns claudette's local-first *posture* into an *enforced* air-gap. With it on, every outbound network call is checked against an allow-list and anything not on it is hard-blocked with a uniform error — `blocked by offline mode (--offline / CLAUDETTE_OFFLINE)…` — whether the call would have been made via reqwest or by spawning a subprocess.

- **Allowed:** the resolved model backend host (`OLLAMA_HOST`, even a LAN box you opted into with `CLAUDETTE_ALLOW_REMOTE_OLLAMA=1` — matched at the host level, so any port on that box is reachable) and loopback (`localhost`, `127.0.0.0/8`, `::1`). The brain, recall embeddings, and local vision keep working.
- **Blocked:** `web_search` / `web_fetch`, `gmail_*` / `calendar_*` / `--auth-google`, `wikipedia`, `weather`, the `gh_*` GitHub tools, `tg_send`, remote `git_push` / `git_clone`, the brownfield `mission_start` clone and `mission_submit` push, and text-to-speech (edge-tts).
- **Refused wholesale:** `bash` / `bash_background`. A raw shell command can reach the network in ways no allow-list can inspect (`curl`, `scp`, `python -c`, `nc`), and a denylist of those leaks by construction — so under `--offline` the shell tools are refused entirely. Keep coding offline with the structured tools (`edit_file`, search, local `git_*`, the build/test runners).
- **`--offline` + `--telegram`** is refused at startup — the Telegram bridge is a cloud relay (`api.telegram.org`) and can't run air-gapped.

Inspect the live allow-list with `claudette --offline --doctor` — the **egress / air-gap** section prints exactly what's reachable and notes that the Google-OAuth live probe is skipped (it can't run offline).

Two layers enforce it: an HTTP-layer guard in the reqwest path checks the destination host of every in-process request, and a dispatch-layer guard refuses tools that reach the network through a subprocess where the HTTP guard can't see the destination. The host-matching logic lives in [`src/egress.rs`](../crates/claudette/src/egress.rs).

## Forge mode

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_MAX_FIX_ROUNDS` | `3` | Cap on Coder→Verifier fix-loop rounds in `--forge`. Default 3 is the empirical sweet spot for local 8b coders. Raise to 4–6 if you've pinned a stronger Verifier model and want it to keep pushing back. Clamped at 10. |
| `CLAUDETTE_FORGE_ABORT_WINDOW_SECS` | `3` | Grace window (seconds) to Ctrl-C out of a forge run before it starts working. Set `0` to skip the pause in CI / scripted runs. Clamped at 30. |

## Tokens (per-tool)

| Variable | Purpose |
|----------|---------|
| `BRAVE_API_KEY` | Brave Search API key — required for `web_search`. |
| `GITHUB_TOKEN` | GitHub PAT — required for the `github` tool group. Falls back to `CLAUDETTE_GITHUB_TOKEN` if unset. |
| `TELEGRAM_BOT_TOKEN` | Bot token from `@BotFather` — required for `--telegram`. Falls back to `CLAUDETTE_TELEGRAM_TOKEN` if unset. |
| `CLAUDETTE_TELEGRAM_CHAT` | Comma-separated chat-ID allowlist for the Telegram bot (same as repeating `--chat`). The bot default-denies when no allowlist is set. |
| `CLAUDETTE_GOOGLE_CLIENT_ID` | Google OAuth client ID — required for `--auth-google` + the Calendar / Gmail tool groups. Falls back to `GOOGLE_CLIENT_ID`, or to `~/.claudette/secrets/google_oauth_client.json`. |
| `CLAUDETTE_GOOGLE_CLIENT_SECRET` | Google OAuth client secret. Same fallback chain as the client ID. |

All tokens also support file-based fallback: save them to `~/.claudette/secrets/<name>.token` (for example `github.token`, `telegram.token`, `brave.token`). Environment variables win over files when both are present.

## Voice

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_WHISPER_BIN` | `whisper-cli` on PATH | Path to the `whisper.cpp` binary. |
| `CLAUDETTE_WHISPER_MODEL` | `~/.claudette/models/ggml-large-v3-turbo.bin` | Path to the Whisper GGML model file. |
| `CLAUDETTE_FFMPEG_BIN` | `ffmpeg` on PATH | Path to the `ffmpeg` binary (transcodes incoming Telegram voice notes for Whisper). |
| `CLAUDETTE_TTS_VOICE_EN` | built-in English voice | edge-tts voice id used for English speech output. |
| `CLAUDETTE_TTS_VOICE_HE` | built-in Hebrew voice | edge-tts voice id used for Hebrew speech output. |
| `CLAUDETTE_TTS_MAX_CHARS` | `500` | Max reply length (characters) sent to edge-tts; longer replies are spoken truncated. |

## Vision

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_VISION_MODEL` | `vision` | Model id used for image attachments. Override when your multimodal model is loaded under a different id (e.g. an LM Studio `@quant`-pinned name). |

## Cross-session recall

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_RECALL_DISABLE` | unset | Set to `1` to disable post-turn recall indexing entirely (privacy / no embed model available). |
| `CLAUDETTE_RECALL_MODEL` | `nomic-embed-text` | Embed model id. Under `CLAUDETTE_OPENAI_COMPAT=1`, set to whatever embedding model you've loaded in LM Studio (e.g. `text-embedding-nomic-embed-text-v1.5`). |
| `CLAUDETTE_RECALL_DB` | `~/.claudette/recall.sqlite` | Override the recall DB path (mostly useful in tests). |

## Permission bypass (use with care)

These each **remove a confirmation gate**. They exist for unattended/CI runs and power users who know the trade-off — setting them weakens the per-tool permission model, so prefer `--offline` + scoped tokens over blanket bypass when you can.

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_AUTO_APPROVE` | unset | ⚠️ Set to `1` to auto-approve **every** DangerFullAccess tool (`bash`, `edit_file`, `git push`, …) without the `[y/N]` prompt. Intended for trusted unattended runs. Safe to combine with `--offline`: the shell tools (`bash` / `bash_background`) are refused wholesale under offline mode, so even blanket auto-approval can't run a network-capable shell command air-gapped. |
| `CLAUDETTE_ALLOW_DESTRUCTIVE_GIT` | unset | ⚠️ Set to `1` to let destructive git operations (`reset --hard`, `clean -f`, force-push, branch delete) run without the extra destructive-git guard. |
| `CLAUDETTE_ALLOW_SECRET_READS` | unset | ⚠️ Set to `1` to let file-read tools open paths the secret-file denylist normally blocks (`~/.ssh`, `*.pem`, `.env`, token files). |
| `CLAUDETTE_WEB_FETCH_ALLOW_PRIVATE` | unset | ⚠️ Set to `1` to let `web_fetch` reach private / loopback / link-local addresses, disabling the SSRF guard. Only for fetching from a host on your own LAN. |

## Advanced / internal tuning

Rarely needed; defaults are tuned for local small models. Mostly useful for debugging spirals or scripting.

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_MAX_TOOLS` | unset (no cap) | Truncate the `tools` array sent per request to N entries (keeps the schema small for context-tight models). |
| `CLAUDETTE_READ_DEFAULT_LINES` | `400` | Default number of lines `read_file` returns when no explicit range is given. |
| `CLAUDETTE_READ_LOOP_LIMIT` | `2` | How many identical re-reads of the same file are tolerated before the read-loop breaker intervenes. |
| `CLAUDETTE_NO_READ_LOOP_BREAKER` | unset | Set to `1` to disable the read-loop breaker entirely. |
| `CLAUDETTE_NO_SPINNER` | unset | Set to `1` to suppress the REPL/TUI activity spinner (TTY only). |
| `CLAUDETTE_MODEL_RELOAD_RETRY_MS` | `750` | Backoff (ms) before retrying a request after the backend reports the model was unloaded/reloaded. |
| `CLAUDETTE_DISABLE_MODEL_RELOAD_RETRY` | unset | Set to `1` to disable the post-reload retry and fail fast instead. |

### Post-edit checks (opt-in)

Opt-in syntax/type check that runs after a successful `write_file`, `edit_file`, or `apply_diff`. With `CLAUDETTE_POST_EDIT_CHECK=1` the module auto-detects a fast check command for `.rs`, `.py`, `.go`, and `.js`/`.mjs`/`.cjs` files; non-zero output is appended to the same tool result so the brain fixes breakage immediately.

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_POST_EDIT_CHECK` | unset (off) | Set to `1`, `true`, `yes`, or `on` (case-insensitive) to enable post-edit checks. The feature does nothing unless explicitly enabled — the default is OFF, so every byte of behaviour is unchanged when unset. |
| `CLAUDETTE_CHECK_CMD` | auto-detected | Custom check command string. Tokens are split on whitespace; the first token is the program, the rest are arguments. Any argument containing `{file}` gets replaced by the edited file's path. If no token contained `{file}`, the file path is appended as one extra final argument. Set to an empty string to force auto-detection even when the variable exists. |
| `CLAUDETTE_CHECK_TIMEOUT_SECS` | `10` | Timeout in seconds for the check command. Clamped to `[1, 120]`. A timed-out check is silently skipped (treated as success). |
| `CLAUDETTE_CHECK_MAX_ROUNDS` | `2` | Per-file per-turn cap on appended check-failure output. Clamped to `[1, 10]`. When the cap is exceeded for a file in a single turn, further failures are summarized with a one-line notice instead of repeating the full output. |

**Auto-detection (when `CLAUDETTE_CHECK_CMD` is unset or empty):**

| Extension | Command | Notes |
|-----------|---------|-------|
| `.rs` | `cargo check --message-format=short` | Runs in workspace root. |
| `.py` | `ruff check <file>` | Falls back to `python -m py_compile <file>` when `ruff` is not on PATH. |
| `.go` | `go vet .` | Runs in the file's parent directory (workspace root if the file is at the root). |
| `.js`, `.mjs`, `.cjs` | `node --check <file>` | Runs in workspace root. |

**Notes:**

- Success (exit 0) appends nothing to the tool result — only failures surface output.
- The whole feature is a no-op under `--offline` / `CLAUDETTE_OFFLINE=1`.
- `apply_patch` is excluded from v1 because it touches multiple files; only single-file writes (`write_file`, `edit_file`, `apply_diff`) trigger checks.

### Context eviction (opt-in)

Opt-in wire-level pass that, under context pressure, replaces the bodies of *stale* tool results (older than the current turn and outside the 8 most-recent results) with a short recovery stub in the outgoing request. The pass runs on the outgoing request at send time; persisted session data (history, undo, transcript) is never modified.

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_EVICT_TOOL_OUTPUT` | unset (off) | Set to `1`, `true`, `yes`, or `on` (case-insensitive) to evict stale tool-result bodies once the estimated prompt exceeds 60% of `num_ctx`, or set an integer `10`–`90` to pick the trigger percentage directly. Anything else is treated as OFF (fail-closed). Results under 512 chars and already-evicted stubs are never touched. |
