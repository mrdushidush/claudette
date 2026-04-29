# Claudette

**Local-first AI personal secretary.** Runs entirely on your own hardware — no cloud brain, no subscription, no telemetry from Claudette itself. Powered by [Ollama](https://ollama.com) and a Rust agent loop. The default brain (`qwen3.5:4b`) fits comfortably on an 8 GB GPU; the optional Codet code-generation sidecar wants 32 GB RAM + a bigger coder model (see [Hardware requirements](#hardware-requirements) below). TTS voice replies use Microsoft's public edge-tts endpoint when `/voice` is enabled — everything else stays on-device.

```bash
cargo install claudette                     # from crates.io
ollama serve &                              # in another shell
claudette "what time is it?"                # 30-second smoke test
claudette                                   # interactive REPL
```

**Works zero-config out of the box**: notes, todos, files, time, weather, Wikipedia, and code search. Brave, GitHub, Google Calendar, and Gmail tools light up when you set the relevant API key — see [Tokens (per-tool)](#tokens-per-tool) below.

[![Crates.io](https://img.shields.io/crates/v/claudette.svg)](https://crates.io/crates/claudette)
[![CI](https://github.com/mrdushidush/claudette/actions/workflows/ci.yml/badge.svg)](https://github.com/mrdushidush/claudette/actions/workflows/ci.yml)
[![Apache 2.0](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust 1.75+](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)

---

## What Claudette does

Claudette is a conversational agent built around **messaging-app access to a local LLM**. Four interfaces — REPL, fullscreen TUI, one-shot CLI, and a Telegram bot — all drive the same Ollama backend, so you can voice-note your own laptop from a bus stop and get a reply back. 70+ tools cover calendar, email, code generation, web research, and shell + git workflows, loaded on demand so the schema stays small.

**What it's not:** a coding assistant competing with Cline or Aider on IDE integration, nor a general-purpose agent framework. Claudette is intentionally single-binary, single-machine, single-user — see [`docs/comparison.md`](docs/comparison.md) for an honest side-by-side against OpenHands, Aider, opencode, Cline, and Continue (Claudette isn't the winner in most of them).

> **v0.2.0 — the Life Agent.** Google Calendar and Gmail (read-only) tool groups, a persistent scheduler that fires prompts back at you, and a `/briefing` Telegram command (or `claudette --briefing` for a recurring 07:00 weekday briefing) that covers the day's calendar, weather, and unread email. See [`docs/life_agent.md`](docs/life_agent.md) and [`docs/google_setup.md`](docs/google_setup.md).

Short walkthroughs (quick tour, Telegram setup, morning briefing, code generation, the brain100 harness) live in [`examples/`](examples/).

<!--
ASCIINEMA SLOT — replace this comment with the embed once recorded.

Suggested demo path (~60s, run after `cargo install claudette &&
ollama serve`):
  $ claudette
  > what time is it?
  > take a note: pick up bread tomorrow
  > what's on my todo list?
  > /briefing
  > /quit

Record:  asciinema rec claudette-demo.cast
Upload:  asciinema upload claudette-demo.cast    (returns a URL like https://asciinema.org/a/123456)
Replace: this comment with:

  [![asciicast](https://asciinema.org/a/123456.svg)](https://asciinema.org/a/123456)

The `.svg` form renders inline on GitHub as a static thumbnail; clicking
it opens the asciinema player. No autoplay, no JS — works inside the
README markdown rendering.
-->

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

### 70+ tools across 12 on-demand groups

To keep the model's context window small, Claudette advertises only ~18 "core" tools by default. The rest load when the model calls `enable_tools(group)`. Each group is a self-contained capability:

| Group | Tools | What it does |
|-------|-------|--------------|
| **core** (always on) | 18 | Notes (incl. `note_update`), todos, files, time, capabilities, web search, code generation, `enable_tools` itself |
| `git` | 8 | status, diff, log, add, commit, branch, checkout, push |
| `ide` | 3 | Open in editor (`code`), reveal in file manager, open URL in browser |
| `search` | 3 | Glob patterns, grep file contents, fetch + strip web pages |
| `advanced` | 3 | Bash shell, `edit_file` (find/replace), `spawn_agent` (delegate to a sub-agent) |
| `facts` | 4 | Wikipedia search/summary, Open-Meteo weather (current/forecast) |
| `registry` | 4 | crates.io info/search, npmjs info/search |
| `github` | 6 | List PRs/issues, get/create/comment issues, code search |
| `markets` | 7 | TradingView quotes/ratings/calendar, Algorand ASA stats via vestige.fi |
| `telegram` | 3 | Bot messaging: send messages, poll updates, send photos |
| `calendar` | 5 | Google Calendar: list / create / update / delete events, RSVP |
| `schedule` | 4 | Proactive reminders: one-shot + recurring schedules that fire prompts back at you |
| `gmail` | 4 | Gmail (read-only): list, search, read, list labels — with `<email>` provenance wrapping |

Schema cost: the core advertises only ~4.7 KB of tool schema on every turn; enabling all 12 groups at once is roughly 5× that. Loading on demand keeps typical conversations well under the full schema cost.

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
| **WorkspaceWrite** | Auto-allowed | note_create, note_update, todo_add, web_search, generate_code, github comment |
| **DangerFullAccess** | Prompts `[y/N]` every time | bash, edit_file, git add/commit/push/checkout |

The REPL prompter is interactive. The TUI renders the permission dialog in its tool pane. Telegram bot denies DangerFullAccess by default (no TTY to confirm with).

### Sessions and auto-compaction

- **Autosave** after every REPL turn to `~/.claudette/sessions/last.json`.
- **Resume** with `--resume` or `-r`.
- **Named sessions** via `/save <name>` and `/load <name>` (stored at `~/.claudette/sessions/<name>.json`).
- **Auto-compaction** is effectively off by default (1 M estimated tokens) — opt in for tight context windows via `CLAUDETTE_COMPACT_THRESHOLD=12000`. When it does fire, it summarises old turns, keeps recent ones verbatim, and preserves tool-result anchoring so the runtime never ends up in a broken state.
- **Sliding-window truncator** acts as a safety net inside the API client.

### Voice in, voice out

Telegram voice messages are transcribed end-to-end locally via [Whisper](https://github.com/ggerganov/whisper.cpp) (default model `ggml-large-v3-turbo`). The reply can be spoken back via [edge-tts](https://github.com/rany2/edge-tts) in English (`en-US-AriaNeural`) or Hebrew (`he-IL-HilaNeural`). Toggle voice output with `/voice`.

### On-demand tool enablement

The `enable_tools(group)` meta-tool lets the model pull in capability groups when it realises it needs them. Adding a new group to Claudette costs zero context until the model actually calls one — the trick that lets a 70-tool surface fit a 16K context window.

The model can also be told to pre-load groups in Telegram and TUI modes where the user can't confirm permissions turn-by-turn — `markets`, `facts`, `search`, `advanced`, and `git` are pre-loaded when `--telegram` or `--tui` is passed.

---

## Quick start

```bash
# 1. Pull the required models with Ollama.
ollama pull qwen3.5:4b           # brain (default Auto preset)
ollama pull qwen3.5:9b           # fallback brain (optional but recommended)
ollama pull qwen3-coder:30b      # Codet coder (best quality; needs 32 GB RAM)

# Or smaller coders if you're disk/RAM-constrained:
ollama pull qwen2.5-coder:14b    # Codet fallback (~9 GB)
ollama pull qwen2.5-coder:7b     # lightweight coder (~4.5 GB); fine for
                                 # routine Python/Rust/TS generation

# 2. Install Claudette onto your PATH.
cargo install claudette
# (Or build from source: `cargo install --path .` inside a clone, or
# `cargo build --release` for a local binary at ./target/release/claudette.)

# 3. (Optional) Set secrets for tool groups that need them.
export BRAVE_API_KEY=...         # web_search
export GITHUB_TOKEN=ghp_...      # github group
export TELEGRAM_BOT_TOKEN=...    # telegram bot mode

# 4. Run.
claudette                        # REPL
claudette --tui                  # fullscreen TUI
claudette "what time is it?"     # one-shot
claudette --resume               # resume last session
claudette --telegram             # Telegram bot
```

On first launch Claudette auto-creates `~/.claudette/` and probes `http://localhost:11434` for Ollama. If Ollama isn't running it prints a friendly error and exits. Bypass the probe with `CLAUDETTE_SKIP_OLLAMA_PROBE=1` for offline sessions that only hit saved state.

---

## Hardware requirements

| Component | Minimum | Recommended | Tested on |
|-----------|---------|-------------|-----------|
| GPU | 6 GB VRAM (CUDA or Metal) | 8 GB VRAM | RTX 3060 Ti 8 GB |
| RAM | 16 GB | 32 GB | 32 GB DDR4 |
| Disk | ~3 GB (brain only) — or ~8 GB with the lightweight 7b coder | ~27 GB (brain + fallback + 30b coder) | NVMe SSD |
| OS | Windows 10+, Linux, macOS | Windows 11 / Ubuntu 24.04 / macOS 14+ | Windows 11 Pro |

### Model footprint summary

| Model | Role | VRAM | Throughput (3060 Ti) |
|-------|------|------|----------------------|
| `qwen3.5:4b` | Brain (default) | ~3.4 GB | ~55 t/s |
| `qwen3.5:9b` | Fallback brain | ~5.5 GB | ~30 t/s |
| `qwen3-coder:30b` | Codet coder (quality) | ~19 GB total (MoE, partial RAM spill) | ~20 t/s effective |
| `qwen2.5-coder:14b` | Codet coder (fallback) | ~9 GB | ~8 t/s with partial spill |
| `qwen2.5-coder:7b` | Codet coder (lightweight) | ~4.5 GB | ~30 t/s |

The 4b brain alone is viable as a standalone setup — it handles tool-calling, note-taking, calendar, and conversation perfectly fine on its own. Add the 9b only when you want better multi-step reasoning. Add a coder only when you use `generate_code`; the 7b fits happily alongside the 4b on 8 GB VRAM.

**For the 30b coder on 8 GB VRAM / 32 GB RAM,** set these Ollama env vars:

```bash
OLLAMA_MAX_LOADED_MODELS=1    # forces brain eviction before coder loads
OLLAMA_FLASH_ATTENTION=1      # halves the KV cache
OLLAMA_KV_CACHE_TYPE=q8_0     # quantised KV cache
```

---

## Usage

### CLI flags

Run `claudette --help` for the authoritative reference. Summary:

| Flag | Effect |
|------|--------|
| `--resume`, `-r` | Continue the most recent saved session. |
| `--telegram`, `-t` | Run as a Telegram bot (needs `TELEGRAM_BOT_TOKEN`). |
| `--tui` | Launch the fullscreen TUI. |
| `--chat <id>` | Restrict Telegram bot to a specific chat ID. Repeatable, or set `CLAUDETTE_TELEGRAM_CHAT` to a comma-separated list. The bot **default-denies** when no allowlist is provided. |
| `--chat any` | Explicit accept-all: serve every incoming Telegram chat. Required to start the bot with no allowlist. Prints a loud warning. |
| `--auth-google [scope]` | Run the loopback OAuth flow. Scope is `calendar` (default) or `gmail`. Stores tokens under `~/.claudette/secrets/`. |
| `--revoke` | Pair with `--auth-google` to revoke consent and delete the local token file. |
| `--briefing` | Write a recurring morning-briefing schedule entry and exit. See [examples/04-morning-briefing.md](examples/04-morning-briefing.md). |
| `--time HH:MM` | Modifier for `--briefing`. Default `07:00`. |
| `--days <spec>` | Modifier for `--briefing`. One of `weekdays` (default), `daily`, or a single weekday name. |
| `--help`, `-h` | Show the flag reference and exit. |
| `--version`, `-V` | Show the claudette version and exit. |

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

Three additional commands are **Telegram-only** (they have no effect in the REPL or TUI):

```
/voice               Toggle voice output (edge-tts on / off).
/lang he|en          Switch voice transcription + TTS language.
/briefing            Run the morning briefing now (calendar + weather + VIP unread).
```

---

## Environment variables

All variables are optional; defaults are shown. Set them in your shell environment, or at `~/.claudette/.env` (the canonical persistent location). Claudette intentionally does **not** auto-load `.env` from the current working directory or its parents — that would let a shared project smuggle `OLLAMA_HOST`, `GITHUB_TOKEN`, etc. into the agent without the user noticing. For per-project overrides, use `direnv` or `source path/to/.env` before invoking.

### Core

| Variable | Default | Purpose |
|----------|---------|---------|
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama API endpoint. Honoured exactly like Ollama itself. |
| `CLAUDETTE_ALLOW_REMOTE_OLLAMA` | unset | Set to `1` to silence the startup warning when `OLLAMA_HOST` is non-loopback. Default posture is local-only. |
| `CLAUDETTE_MODEL` | `qwen3.5:4b` (Auto preset) | Brain model override. |
| `CLAUDETTE_NUM_CTX` | `16384` | Brain context window in tokens. |
| `CLAUDETTE_NUM_PREDICT` | `6144` | Max output tokens per request. |
| `CLAUDETTE_COMPACT_THRESHOLD` | `1000000` | Auto-compaction trigger (estimated tokens). Default makes auto-compact a no-op for typical 16K–128K context windows; set to `12000` (or a fraction of your `num_ctx`) on tight contexts. |
| `CLAUDETTE_MAX_ITERATIONS` | `40` | Per-turn (model → tool → result) loop ceiling. Lower it (e.g. `15`) to fail-fast on small-model spirals; raise it for legitimate long tool chains. |
| `CLAUDETTE_SESSION` | `~/.claudette/sessions/last.json` | Override the session file path. |
| `CLAUDETTE_MEMORY` | `~/.claudette/CLAUDETTE.MD` | Override the path Claudette loads user-memory from. |
| `CLAUDETTE_SKIP_OLLAMA_PROBE` | unset | Set to `1` to skip the startup probe (CI / offline). |
| `CLAUDETTE_FALLBACK_BRAIN_MODEL` | `qwen3.5:9b` (Auto preset) | Brain to fall back to on stuck signals. |
| `CLAUDETTE_LIVE_GOOGLE` | unset | Set to `1` to run live Google integration tests via `cargo test --ignored`. Never set in CI. |
| `CLAUDETTE_WORKSPACE` | unset | Extra read roots outside `$HOME`, colon-separated on Unix, semicolon-separated on Windows. Example: `D:\dev\claudette` for developing Claudette itself. Reads under `$HOME` and under a `$HOME`-rooted CWD are always allowed regardless. |

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
| `CLAUDETTE_GOOGLE_CLIENT_ID` | Google OAuth client ID — required for `--auth-google` + the Calendar / Gmail tool groups. Falls back to `GOOGLE_CLIENT_ID`, or to `~/.claudette/secrets/google_oauth_client.json`. |
| `CLAUDETTE_GOOGLE_CLIENT_SECRET` | Google OAuth client secret. Same fallback chain as the client ID. |
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
├── tools.rs          — Aggregates per-group schemas into secretary_tools_json() and routes dispatch_tool() through each sub-module's dispatch()
├── tools/            — One module per tool cluster (calendar, codegen, facts, file_ops, git, github, gmail, ide, markets, notes, registry, schedule, search, shell, telegram, todos, web_search); each exports schemas() + dispatch()
├── tool_groups.rs    — ToolRegistry + the 12 on-demand tool-group definitions
├── agents.rs         — AgentType, FilteredToolExecutor, spawn_agent orchestrator
├── codet.rs          — Code-generation sidecar (syntax check, surgical fix loop, tests)
├── test_runner.rs    — Python/Rust/JS/TS syntax + test runners
├── commands.rs       — 22 slash-command parsers and handlers
├── prompt.rs         — Claudette system prompt builder
├── model_config.rs   — Preset + RoleConfig + TOML overlay
├── brain_selector.rs — Tiered-brain fallback + stuck diagnostics
├── memory.rs         — CLAUDETTE.MD loader
├── secrets.rs        — File-backed token storage + Telegram chat-ID persistence
├── google_auth.rs    — Google OAuth loopback flow (per-scope token files under ~/.claudette/secrets/)
├── clock.rs          — Clock trait (SystemClock in prod, MockClock for deterministic scheduler tests)
├── scheduler.rs      — Persistent jsonl scheduler with catch-up policies + natural-language expression parsing
├── briefing.rs       — Morning-briefing prompt (shared by /briefing command and the --briefing scheduled entry)
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

Tests: **521 passing, 4 ignored on Windows** (hook tests that use POSIX `printf`). Run `cargo fmt --all --check` before committing.

### Project layout rules

- Runtime modules (`src/runtime/*.rs`) are mounted at the crate root via `#[path = "runtime/..."]` attributes. Their internal `use crate::session::X` paths resolve without rewriting. Don't move these files or add `mod` declarations in `runtime/mod.rs`.
- Single binary, single library. Both are named `claudette` and live in the same crate.
- No `workspace = true` in dependencies — this is a standalone repo.

### Adding a new tool

1. Add a JSON schema entry to the relevant `src/tools/<group>.rs` (or create a new group module if none fits).
2. Add a `run_my_tool(input: &str) -> Result<String, String>` handler in the same module.
3. Wire it into the `dispatch` match at the top of the module.
4. For a new group: add a `ToolGroup` variant in `src/tool_groups.rs`, then register the group's `schemas()` and `dispatch()` in `src/tools.rs` (follow the existing groups as templates).
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
- Module-level quality polish for the speculative tool groups (markets, github).

Longer-term vision:

- A vision sidecar (`analyze_screenshot`) once a multimodal model with strong tool calling fits 8 GB VRAM.
- Continuous ambient mode (watch-and-interrupt).

---

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the full guide. Quick summary:

- File bugs at <https://github.com/mrdushidush/claudette/issues>.
- Run `cargo fmt --all --check`, `cargo clippy --all-targets --no-deps -- -D warnings`, and `cargo test --lib` before opening a PR.
- Follow Conventional Commits: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `style:`, `ci:`.
- By contributing, you agree your work is licensed under Apache 2.0.

Security issues: please use the private advisory flow described in [`SECURITY.md`](SECURITY.md) — don't open a public issue.

Be kind — [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) has the short version.

---

## License

Apache License 2.0 — see [LICENSE](LICENSE). You can use, modify, and redistribute Claudette commercially or personally. No trademark grant; don't imply endorsement.

Copyright © 2026 [mrdushidush](https://github.com/mrdushidush).
