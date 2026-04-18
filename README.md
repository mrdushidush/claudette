# Claudette

**Local-first AI personal secretary.** Runs entirely on your hardware — no cloud, no subscription, no telemetry. Powered by [Ollama](https://ollama.com) and a Rust agent loop. Works on a single 8 GB GPU.

```bash
cargo install --path .          # build the binary
ollama serve &                  # in another shell
claudette                       # interactive REPL
```

[![Apache 2.0](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust 1.75+](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)

---

## What Claudette does

Claudette is a conversational agent that runs in four modes — REPL, fullscreen TUI, one-shot CLI, and Telegram bot — and calls 58 local tools across 9 on-demand groups. It has three specialised sub-agents, a dedicated code-generation sidecar called **Codet** that auto-validates generated code, a file-backed session store with auto-compaction, and a three-tier permission system for destructive actions.

The tagline: *a general-purpose AI assistant that your laptop can actually run*.

---

## Feature tour

### Four interfaces, same brain

| Mode | Command | What it's for |
|------|---------|---------------|
| **REPL** | `claudette` | Conversational shell. Autosaves after every turn. |
| **One-shot** | `claudette "your question"` | Print a reply and exit. Great for scripts and shell pipelines. |
| **TUI** | `claudette --tui` | Fullscreen ratatui UI with 5 tabs: Chat, Tools, Notes, Todos, HW. |
| **Telegram bot** | `claudette --telegram` | Remote-chat access with voice input (Whisper) and voice output (TTS). |

Each mode reuses the same conversation runtime, the same tool set, and the same session format. Switching modes is just a different entry point.

### 58 tools across 9 on-demand groups

To keep the model's context window small, Claudette advertises only ~17 "core" tools by default. The rest load when the model calls `enable_tools(group)`. Each group is a self-contained capability:

| Group | Tools | What it does |
|-------|-------|--------------|
| **core** (always on) | 17 | Notes, todos, files, time, capabilities, web search, code generation, `enable_tools` itself |
| `git` | 8 | status, diff, log, add, commit, branch, checkout, push |
| `ide` | 3 | Open in editor (`code`), reveal in file manager, open URL in browser |
| `search` | 3 | Glob patterns, grep file contents, fetch + strip web pages |
| `advanced` | 3 | Bash shell, `edit_file` (find/replace), `spawn_agent` (delegate to a sub-agent) |
| `facts` | 4 | Wikipedia search/summary, Open-Meteo weather (current/forecast) |
| `registry` | 4 | crates.io info/search, npmjs info/search |
| `github` | 6 | List PRs/issues, get/create/comment issues, code search |
| `markets` | 7 | TradingView quotes/ratings/calendar, Algorand ASA stats via vestige.fi |
| `telegram` | 3 | Bot messaging: send messages, poll updates, send photos |

Schema cost: **~4.7 KB (core) vs ~18.2 KB (all)** — loading groups on demand saves roughly 72 % of the context per turn.

### Three specialised sub-agents

Claudette can delegate complex tasks to sub-agents via the `spawn_agent` tool. Each agent gets its own isolated conversation context — only the final text comes back to Claudette.

| Agent | What it does | Max turns |
|-------|--------------|-----------|
| **Researcher** | Web search + file read + code search. For open-ended investigations. | 10 |
| **GitOps** | Git workflows with bash. For "rebase this, squash that, push it." | 8 |
| **Code Reviewer** | Read-only. Spots bugs, security issues, style problems. | 5 |

### Codet: dedicated code-generation sidecar

Every call to `generate_code` goes through **Codet** — a separate LLM pipeline that:

1. Writes the code with a dedicated coder model (default `qwen3-coder:30b`, fallback `qwen2.5-coder:14b`).
2. Runs a syntax check (`python -m py_compile`, `rustc --emit=metadata`, `tsc --noEmit`, etc. — 5 languages).
3. On failure, runs a **surgical SEARCH/REPLACE fix loop** (Aider-style patches, ~50 output tokens per attempt) before falling back to full-file regeneration.
4. Optionally runs associated pytest/cargo-test/jest suites.
5. Retries up to 3 times, then reports honestly if it can't fix the file.

Codet is **hot-swapped into VRAM on demand** — the main brain model is evicted first on memory-constrained machines, then restored after Codet finishes. Swap cost is ~5–10 seconds on a 3060 Ti.

### Tiered-brain auto-fallback

Claudette ships with three presets:

- **Fast**: brain is `qwen3.5:4b` (fast, 3.4 GB VRAM), no fallback.
- **Auto** (default): `qwen3.5:4b` with an auto-escalation to `qwen3.5:9b` on stuck signals (empty response after retry, max iterations hit with no text, ≥ 3 consecutive tool errors). Reverts to 4b after the failed turn — per-turn revert, not session-sticky.
- **Smart**: brain is `qwen3.5:9b`, no fallback.

Switch at runtime with `/preset fast | auto | smart`, or pin a specific brain with `/brain <model>`.

### Permissions: three tiers, enforced per-tool

| Tier | Behaviour | Example tools |
|------|-----------|---------------|
| **ReadOnly** | Auto-allowed | time, note_list, file reads, git status, all external APIs |
| **WorkspaceWrite** | Auto-allowed | note_create, todo_add, web_search, generate_code, github comment |
| **DangerFullAccess** | Prompts `[y/N]` every time | bash, edit_file, git add/commit/push/checkout |

The REPL prompter is interactive. The TUI renders the permission dialog in its tool pane. Telegram bot denies DangerFullAccess by default (no TTY to confirm with).

### Sessions and auto-compaction

- **Autosave** after every REPL turn to `~/.claudette/sessions/last.json`.
- **Resume** with `--resume` or `-r`.
- **Named sessions** via `/save <name>` and `/load <name>` (stored at `~/.claudette/sessions/<name>.json`).
- **Auto-compaction** fires at 12 K estimated tokens (configurable via `CLAUDETTE_COMPACT_THRESHOLD`) — summarises old turns, keeps recent ones verbatim, preserves tool-result anchoring so the runtime never ends up in a broken state.
- **Sliding-window truncator** acts as a safety net inside the API client.

### Voice in, voice out

Telegram voice messages are transcribed end-to-end locally via [Whisper](https://github.com/ggerganov/whisper.cpp) (default model `ggml-large-v3-turbo`). The reply can be spoken back via [edge-tts](https://github.com/rany2/edge-tts) in English (`en-US-AriaNeural`) or Hebrew (`he-IL-HilaNeural`). Toggle voice output with `/voice`.

### On-demand tool enablement

The `enable_tools(group)` meta-tool lets the model pull in capability groups when it realises it needs them. This is Sprint 8's flagship architectural decision: adding 100 tools to Claudette costs zero context until the model actually calls one.

The model can also be told to pre-load groups in Telegram mode where the user can't confirm permissions turn-by-turn — `markets`, `facts`, `search`, `ide`, and `git` are pre-loaded when `--telegram` is passed.

---

## Quick start

```bash
# 1. Pull the required models with Ollama.
ollama pull qwen3.5:4b           # brain (default Auto preset)
ollama pull qwen3.5:9b           # fallback brain (optional but recommended)
ollama pull qwen3-coder:30b      # Codet coder (recommended; needs 32 GB RAM)
# or a smaller coder if you're RAM-constrained:
ollama pull qwen2.5-coder:14b    # Codet fallback

# 2. Build Claudette.
cargo build --release

# 3. (Optional) Set secrets for tool groups that need them.
export BRAVE_API_KEY=...         # web_search
export GITHUB_TOKEN=ghp_...      # github group
export TELEGRAM_BOT_TOKEN=...    # telegram bot mode

# 4. Run.
./target/release/claudette                 # REPL
./target/release/claudette --tui           # fullscreen TUI
./target/release/claudette "what time is it?"   # one-shot
./target/release/claudette --resume        # resume last session
./target/release/claudette --telegram      # Telegram bot
```

On first launch Claudette auto-creates `~/.claudette/` and probes `http://localhost:11434` for Ollama. If Ollama isn't running it prints a friendly error and exits. Bypass the probe with `CLAUDETTE_SKIP_OLLAMA_PROBE=1` for offline sessions that only hit saved state.

---

## Hardware requirements

| Component | Minimum | Recommended | Tested on |
|-----------|---------|-------------|-----------|
| GPU | 6 GB VRAM (CUDA or Metal) | 8 GB VRAM | RTX 3060 Ti 8 GB |
| RAM | 16 GB | 32 GB | 32 GB DDR4 |
| Disk | ~3 GB (brain only) | ~27 GB (brain + fallback + coder) | NVMe SSD |
| OS | Windows 10+, Linux, macOS | Windows 11 / Ubuntu 24.04 / macOS 14+ | Windows 11 Pro |

### Model footprint summary

| Model | Role | VRAM | Throughput (3060 Ti) |
|-------|------|------|----------------------|
| `qwen3.5:4b` | Brain (default) | ~3.4 GB | ~55 t/s |
| `qwen3.5:9b` | Fallback brain | ~5.5 GB | ~30 t/s |
| `qwen3-coder:30b` | Codet coder (quality) | ~19 GB total (MoE, partial RAM spill) | ~20 t/s effective |
| `qwen2.5-coder:14b` | Codet coder (fallback) | ~9 GB | ~8 t/s with partial spill |

**For the 30b coder on 8 GB VRAM / 32 GB RAM,** set these Ollama env vars:

```bash
OLLAMA_MAX_LOADED_MODELS=1    # forces brain eviction before coder loads
OLLAMA_FLASH_ATTENTION=1      # halves the KV cache
OLLAMA_KV_CACHE_TYPE=q8_0     # quantised KV cache
```

---

## Usage

### CLI flags

| Flag | Effect |
|------|--------|
| `--resume`, `-r` | Continue the most recent saved session. |
| `--telegram`, `-t` | Run as a Telegram bot (needs `TELEGRAM_BOT_TOKEN`). |
| `--tui` | Launch the fullscreen TUI. |
| `--chat <id>` | Restrict Telegram bot to a specific chat ID. Repeatable. |

### Slash commands (REPL + TUI)

```
/help                Show this list.
/agents              List available sub-agent types.
/validate <path>     Run Codet on an existing code file.
/status              Session info + token counts.
/cost                Lifetime token usage.
/tools               List all tools grouped by capability.
/model               Show the active brain and coder models.
/models              Alias for /model.
/preset fast|auto|smart  Switch model preset.
/brain <model>       Pin the brain model (or "auto" to re-enable fallback).
/coder <model>       Pin the coder model.
/memory              Show CLAUDETTE.MD contents.
/reload              Re-read CLAUDETTE.MD into the system prompt.
/sessions, /ls       List saved sessions.
/save <name>         Save the current session under <name>.
/load <name>         Load a named session.
/compact             Force context compaction now.
/clear               Reset to a fresh session.
/capabilities        Full configuration dump.
/exit                Leave the REPL.
```

### Telegram-mode slash commands

A subset of the REPL commands works identically inside Telegram chats: `/help`, `/status`, `/compact`, `/clear`, `/save`, `/load`. `/exit` and the destructive DangerFullAccess commands are blocked.

Two additional commands are **Telegram-only** (they have no effect in the REPL or TUI):

```
/voice               Toggle voice output (edge-tts on / off).
/lang he|en          Switch voice transcription + TTS language.
```

---

## Environment variables

All variables are optional; defaults are shown. Set them in the shell, in a `.env` file at the current directory, or at `~/.claudette/.env` (the recommended persistent location).

### Core

| Variable | Default | Purpose |
|----------|---------|---------|
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama API endpoint. Honoured exactly like Ollama itself. |
| `CLAUDETTE_MODEL` | `qwen3.5:4b` (Auto preset) | Brain model override. |
| `CLAUDETTE_NUM_CTX` | `16384` | Brain context window in tokens. |
| `CLAUDETTE_NUM_PREDICT` | `6144` | Max output tokens per request. |
| `CLAUDETTE_COMPACT_THRESHOLD` | `12000` | Auto-compaction trigger (estimated tokens). |
| `CLAUDETTE_SESSION` | `~/.claudette/sessions/last.json` | Override the session file path. |
| `CLAUDETTE_SKIP_OLLAMA_PROBE` | unset | Set to `1` to skip the startup probe (CI / offline). |
| `CLAUDETTE_FALLBACK_BRAIN_MODEL` | `qwen3.5:9b` (Auto preset) | Brain to fall back to on stuck signals. |

### Codet (code-generation sidecar)

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_CODER_MODEL` | `qwen3-coder:30b` | Coder model. Set to `qwen2.5-coder:14b` on RAM-constrained hosts. |
| `CLAUDETTE_CODER_NUM_CTX` | `49152` | Coder context window. Drop to `16384` on 32 GB RAM boxes. |
| `CLAUDETTE_CODER_NUM_PREDICT` | `12288` | Max output tokens the coder can emit in one call. |
| `CLAUDETTE_VALIDATE_CODE` | `true` | Enable/disable Codet auto-validation after `generate_code`. |

### Tokens (per-tool)

| Variable | Purpose |
|----------|---------|
| `BRAVE_API_KEY` | Brave Search API key — required for `web_search`. |
| `GITHUB_TOKEN` | GitHub PAT — required for the `github` tool group. Falls back to `CLAUDETTE_GITHUB_TOKEN` if unset. |
| `TELEGRAM_BOT_TOKEN` | Bot token from `@BotFather` — required for `--telegram`. |
| `VESTIGE_API_BASE` | Override for the vestige.fi Algorand API (`markets` group). |

All tokens also support file-based fallback: save them to `~/.claudette/secrets/<name>.token` (for example `github.token`, `telegram.token`, `brave.token`). Environment variables win over files when both are present.

### Voice

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_WHISPER_BIN` | `whisper-cli` on PATH | Path to the `whisper.cpp` binary. |
| `CLAUDETTE_WHISPER_MODEL` | `~/.claudette/models/ggml-large-v3-turbo.bin` | Path to the Whisper GGML model file. |

### Sub-agent tuning

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAUDETTE_RESEARCHER_MODEL` | inherits brain | Override the Researcher agent's model. |
| `CLAUDETTE_GITOPS_MODEL` | inherits brain | Override the GitOps agent's model. |
| `CLAUDETTE_RESEARCHER_MAX_ITER` | `10` | Hard cap on Researcher tool calls per delegation. |
| `CLAUDETTE_GITOPS_MAX_ITER` | `8` | Hard cap on GitOps tool calls per delegation. |
| `CLAUDETTE_TELEGRAM_CHAT` | unset | Comma-separated chat-ID allowlist for Telegram bot. |

---

## Storage layout

```
~/.claudette/
├── notes/                       # Markdown notes (ISO-timestamped, optional tags)
├── files/                       # Sandboxed scratch dir for write_file/generate_code
├── sessions/
│   ├── last.json                # Auto-saved REPL session
│   └── <name>.json              # Named sessions via /save
├── secrets/
│   ├── github.token             # GitHub PAT (plain text)
│   ├── telegram.token           # Telegram bot token
│   ├── brave.token              # Brave Search API key
│   └── telegram_chat.id         # Auto-persisted Telegram chat IDs (one per line)
├── models/
│   └── ggml-large-v3-turbo.bin  # Whisper model (download separately)
├── todos.json                   # Task list
├── models.toml                  # Optional model-config overlay (preset + per-role overrides)
├── fallback.jsonl               # Auto-fallback event log (one JSON line per escalation)
├── .env                         # Persistent env-var overrides
└── CLAUDETTE.MD                 # Optional user memory (800-char cap, loaded into system prompt)
```

Nothing outside `~/.claudette/` is written without explicit permission.

---

## Architecture

```
src/
├── main.rs           — Binary entry point (arg parsing, Ollama probe, mode dispatch)
├── lib.rs            — Module declarations + public re-exports
├── runtime/          — Embedded agent-loop kernel (~2K LOC, vendored)
│   ├── conversation.rs — Turn loop, tool dispatch, hook integration, ApiClient trait
│   ├── session.rs      — Session / ConversationMessage / ContentBlock types
│   ├── compact.rs      — Auto-compaction + token estimation
│   ├── permissions.rs  — Three-tier permission policy
│   ├── usage.rs        — TokenUsage tracker + pricing lookup (Ollama = free)
│   ├── hooks.rs        — Pre/post tool-use hooks (shell snippets)
│   ├── prompt.rs       — ProjectContext discovery (cwd, git status, instruction files)
│   ├── config.rs       — Optional configuration loaders
│   ├── json.rs         — Hand-rolled JSON for the no-serde-dep runtime paths
│   └── sandbox.rs      — Sandbox config types (Linux-only sandbox runner)
├── api.rs            — OllamaApiClient: /api/chat streamer, truncation, budget math, probe
├── run.rs            — Runtime builder, REPL loop, autosave, session compaction
├── executor.rs       — SecretaryToolExecutor: enable_tools meta-tool + dispatch
├── tools.rs          — 58 tool schemas + all run_* handlers
├── tool_groups.rs    — ToolRegistry + the 9 on-demand tool-group definitions
├── agents.rs         — AgentType, FilteredToolExecutor, spawn_agent orchestrator
├── codet.rs          — Code-generation sidecar (syntax check, surgical fix loop, tests)
├── test_runner.rs    — Python/Rust/JS/TS syntax + test runners
├── commands.rs       — 22 slash-command parsers and handlers
├── prompt.rs         — Claudette system prompt builder
├── model_config.rs   — Preset + RoleConfig + TOML overlay
├── brain_selector.rs — Tiered-brain fallback + stuck diagnostics
├── memory.rs         — CLAUDETTE.MD loader
├── secrets.rs        — File-backed token storage + Telegram chat-ID persistence
├── telegram_mode.rs  — Telegram bot loop (polling, voice, slash commands)
├── voice.rs          — Whisper transcription pipeline
├── tts.rs            — edge-tts TTS integration
├── theme.rs          — Colored output, emoji glyphs, TTY detection
├── tui.rs            — Ratatui TUI app, 5 tabs, render loop
├── tui_events.rs     — TUI event enums (worker ↔ render channel)
├── tui_executor.rs   — ToolExecutor wrapper that fires TUI events
└── tui_worker.rs     — Worker thread that owns the ConversationRuntime
```

### The on-demand tool-group contract

`ToolRegistry` lives behind an `Arc<Mutex<_>>`. The `OllamaApiClient` reads it on every `/api/chat` request, so when the model calls `enable_tools("markets")`, the executor mutates the shared registry and the next API call advertises the expanded tool list. Adding a new tool group is a three-step change (add enum variant, register tool set, document the group) and costs zero context until first use.

### Codet sidecar contract

Codet is invoked exclusively through the `generate_code` tool. The main conversation never sees Codet's internal fix-loop exchanges — only the one-line summary + file path on disk. This is deliberate: Codet's iteration chatter would otherwise fill 20 KB of context per coding task.

---

## Development

### Build

```bash
cargo build --release
```

### Verify

```bash
cargo clippy --all-targets --no-deps -- -D warnings
cargo test --lib
```

Tests: **371 passing, 4 ignored on Windows** (hook tests that use POSIX `printf`). Run `cargo fmt --check` before committing.

### Project layout rules

- Runtime modules (`src/runtime/*.rs`) are mounted at the crate root via `#[path = "runtime/..."]` attributes. Their internal `use crate::session::X` paths resolve without rewriting. Don't move these files or add `mod` declarations in `runtime/mod.rs`.
- Single binary, single library. Both are named `claudette` and live in the same crate.
- No `workspace = true` in dependencies — this is a standalone repo.

### Adding a new tool

1. Add a JSON schema entry to `src/tools.rs` inside `secretary_tools_json!`.
2. Add a `run_my_tool(input: &str) -> Result<String, String>` handler in the same file.
3. Wire it into `dispatch_tool` (the big match at the bottom of `tools.rs`).
4. If it needs a new capability group, add a `ToolGroup` variant in `tool_groups.rs` and extend `ToolRegistry`.
5. Add at least one unit test for the happy path and one for a known failure mode.

### Coding standards

- `#![forbid(unsafe_code)]` in the crate root — no unsafe.
- Clippy pedantic is on workspace-wide. Allow-list lives in `Cargo.toml` and covers ergonomic exceptions.
- `#[must_use]` on any function returning a non-trivial value.
- No `panic!` in production paths — every `Result` returns a typed error. Panics are only acceptable inside `#[cfg(test)] mod tests` blocks.
- Tests that mutate environment variables must acquire `crate::test_env_lock()` to avoid parallel-test races.

---

## Roadmap

Short-term (things being actively evaluated):

- Threshold tuning for the tiered-brain fallback, using real `fallback.jsonl` data from the field.
- A runnable brownfield correctness check (not just syntax smoke-testing) for the `generate_code` pipeline.
- Split `src/tools.rs` (currently ~6.4 K lines) into `tools/git.rs`, `tools/web.rs`, etc.
- Module-level quality polish for the speculative tool groups (markets, github).

Longer-term vision:

- A vision sidecar (`analyze_screenshot`) once a multimodal model with strong tool calling fits 8 GB VRAM.
- Continuous ambient mode (watch-and-interrupt).
- Optional, opt-in phone-home for anonymous usage telemetry — only if the community asks for it. Today everything is local.

---

## Contributing

- File bugs at <https://github.com/mrdushidush/claudette/issues>.
- Run `cargo fmt --check`, `cargo clippy --all-targets --no-deps -- -D warnings`, and `cargo test --lib` before opening a PR.
- Follow Conventional Commits: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`.
- By contributing, you agree your work is licensed under Apache 2.0.

Be kind — treat fellow contributors with respect in issues, PRs, and discussions.

---

## License

Apache License 2.0 — see [LICENSE](LICENSE). You can use, modify, and redistribute Claudette commercially or personally. No trademark grant; don't imply endorsement.

Copyright © 2026 [mrdushidush](https://github.com/mrdushidush)).
