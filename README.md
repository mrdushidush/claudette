# Claudette

**A local-first AI secretary that runs on your own 8 GB GPU.** REPL, fullscreen TUI, one-shot CLI, and a Telegram bot — all driving the same [Ollama](https://ollama.com) backend. No cloud brain, no subscription, no telemetry. Single Rust binary.

```bash
ollama pull qwen3.5:4b      # 3.4 GB brain
cargo install claudette
ollama serve &
claudette "what time is it?"
```

[![Crates.io](https://img.shields.io/crates/v/claudette.svg)](https://crates.io/crates/claudette)
[![CI](https://github.com/mrdushidush/claudette/actions/workflows/ci.yml/badge.svg)](https://github.com/mrdushidush/claudette/actions/workflows/ci.yml)
[![Apache 2.0](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust 1.75+](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)

![Claudette TUI — chat + live tool-call panel side-by-side, one turn covering notes, weather, BTC price, and calendar](docs/images/claudette-tui.png)

> One turn driving four tool groups (`note_list`, `weather_forecast`, `tv_get_quote`, `calendar_list_events`) — the brain enables groups on demand and dispatches calls. TUI tabs: `[1]Chat [2]Tools [3]Notes [4]Todos [5]HW`.

---

## Why Claudette

The open-source AI agent space is crowded with coding-focused tools (Aider, Cline, OpenHands, opencode). Claudette is aimed at a different slot: **a general-purpose personal assistant you can voice-note from a bus stop, that runs entirely on your own laptop GPU, with no cloud brain in the loop.**

- **Truly local.** No cloud-brain code path exists. Ollama on `localhost` is the only required dependency. Voice TTS is the only optional outbound network call (Microsoft edge-tts) and lives behind `/voice on`.
- **Fits a single 3060-class GPU.** The default `qwen3.5:4b` brain uses ~3.4 GB VRAM; auto-fallback to `qwen3.5:9b` only fires on stuck signals. No 32 GB-VRAM hidden requirement.
- **Messaging-first.** None of the comparable tools ship a Telegram bot interface — voice in (Whisper), voice out (edge-tts), and full agent control from your phone.
- **Personal, not just code.** Tool groups cover Google Calendar, Gmail, scheduler/briefings, notes, todos, markets, weather, web search — code-gen is *one* capability (via the Codet sidecar), not the whole point.

Honest side-by-side vs. OpenHands, Aider, opencode, Cline, Continue: [`docs/comparison.md`](docs/comparison.md). Claudette isn't the winner in most of them — it's the only one aimed at this specific slot.

---

## Highlights

### Four interfaces, one brain
| Mode | Command | What it's for |
|------|---------|---------------|
| **REPL** | `claudette` | Conversational shell. Autosaves every turn. |
| **One-shot** | `claudette "your question"` | Print a reply and exit. Pipe-friendly. |
| **TUI** | `claudette --tui` | Ratatui fullscreen UI with 5 tabs. |
| **Telegram bot** | `claudette --telegram` | Voice-capable remote chat. |

### 80+ tools, ~170 token base schema
Every tool except `enable_tools` and `get_current_time` lives in a group the model opts into via `enable_tools(group)`. 18 groups (notes, todos, files, code, git, github, ide, search, advanced, facts, registry, markets, telegram, calendar, schedule, gmail, recall, meta) — schema cost stays flat until the model actually needs the surface.

### Brownfield missions: clone, edit, ship a PR — in one tool chain
`mission_start("owner/repo")` clones into `~/.claudette/missions/<slug>/` and silently re-routes `git_status` / `glob_search` / `grep_search` / `write_file` / `bash` into the mission tree. `mission_submit` auto-branches, commits, pushes, and opens the PR via `gh_create_pr`. Resumable across sessions via `mission_attach`.

### Forge-mode: autonomous code-change pipeline
`claudette --forge "<prompt>"` or `/forge <prompt>` runs a Planner → Coder → Verifier loop against the active mission, with a configurable fix-loop (default 2 rounds) before the PR opens. Roles are routable via `~/.claudettes-forge/models.toml` so you can pin a stronger model to Verifier and keep a cheap model on Coder.

### Tiered-brain auto-fallback
Three presets (Fast / Auto / Smart). Auto runs `qwen3.5:4b` and escalates to `qwen3.5:9b` on stuck signals (empty response after retry, max-iterations hit with no text, ≥ 3 consecutive tool errors). Per-turn revert — not session-sticky.

### Voice in, voice out, and vision in
Whisper transcription for Telegram voice notes, edge-tts for replies (English or Hebrew). Image attachments in the TUI/REPL via Alt+V (clipboard), drag-drop, or `@/path/to/img.png` when the loaded brain is multimodal.

### Codet sidecar for code generation
`generate_code` routes through a dedicated coder model (default `qwen3-coder:30b`, fallback `qwen2.5-coder:14b`). Runs a real syntax check (`py_compile`, `rustc --emit=metadata`, `tsc --noEmit`, etc. — 5 languages), then an Aider-style SEARCH/REPLACE fix loop on failure, then optional pytest/cargo-test/jest. Hot-swaps into VRAM on demand on memory-constrained boxes.

### Cross-session semantic recall
`/recall <query>` searches past conversation turns across sessions via an embedding index (works on Ollama or LM Studio's `/v1/embeddings`). Drops fragments of relevant past turns straight into the current context.

### Three sub-agents
`spawn_agent` delegates to a Researcher (web + file + code search, 10 turn cap), GitOps (rebase/squash/push, 8 turn cap), or Code Reviewer (read-only, 5 turn cap). Only the final text comes back — sub-agent chatter doesn't pollute the main context.

### Per-tool permission gating
ReadOnly tools auto-allow, WorkspaceWrite tools auto-allow, DangerFullAccess prompts `[y/N]` every time (bash, `edit_file`, `git add/commit/push/checkout`, cross-org PRs). Telegram default-denies DangerFullAccess (no TTY).

---

## Hardware

| Component | Minimum | Recommended | Tested on |
|-----------|---------|-------------|-----------|
| GPU | 6 GB VRAM | 8 GB VRAM | RTX 3060 Ti 8 GB |
| RAM | 16 GB | 32 GB | 32 GB DDR4 |
| Disk | ~3 GB (brain only) | ~27 GB (brain + fallback + 30b coder) | NVMe SSD |
| OS | Windows 10+, Linux, macOS | Windows 11 / Ubuntu 24.04 / macOS 14+ | Windows 11 Pro |

Full model footprint table and the 30b-coder-on-8GB-VRAM env recipe: [`docs/hardware.md`](docs/hardware.md).

---

## Quick start

```bash
# 1. Pull models with Ollama.
ollama pull qwen3.5:4b           # brain (default Auto preset)
ollama pull qwen3.5:9b           # fallback brain (optional)
ollama pull qwen3-coder:30b      # Codet coder, only if you'll use generate_code

# 2. Install.
cargo install claudette

# 3. (Optional) Tokens for tools that need them.
export BRAVE_API_KEY=...         # web_search
export GITHUB_TOKEN=ghp_...      # github group
export TELEGRAM_BOT_TOKEN=...    # --telegram mode

# 4. Run.
claudette                        # REPL
claudette --tui                  # TUI
claudette "what time is it?"     # one-shot
claudette --resume               # resume last session
claudette --telegram             # Telegram bot
```

First launch auto-creates `~/.claudette/` and probes `http://localhost:11434`. Bypass the probe with `CLAUDETTE_SKIP_OLLAMA_PROBE=1` for offline sessions.

Out of the box: notes, todos, files, time, weather, Wikipedia, code search. Brave / GitHub / Google Calendar / Gmail tools light up when you set the relevant token — full table in [`docs/configuration.md`](docs/configuration.md).

---

## Docs

- [`docs/quickstart.md`](docs/quickstart.md) — 30-second start, common flows
- [`docs/configuration.md`](docs/configuration.md) — every env var, token file fallbacks, recall settings
- [`docs/hardware.md`](docs/hardware.md) — VRAM/RAM/disk by preset, 30b-on-8GB env recipe
- [`docs/usage.md`](docs/usage.md) — CLI flags, slash commands, Telegram-only commands
- [`docs/architecture.md`](docs/architecture.md) — module layout, tool-group contract, Codet sidecar contract
- [`docs/comparison.md`](docs/comparison.md) — honest side-by-side vs. opencode / Aider / OpenHands / Cline / Continue
- [`docs/google_setup.md`](docs/google_setup.md) — Calendar + Gmail OAuth walkthrough

---

## Storage layout

```
~/.claudette/
├── notes/            # Markdown notes (ISO-timestamped, optional tags)
├── files/            # Sandboxed scratch dir for write_file/generate_code
├── sessions/         # Auto-saved + named sessions
├── secrets/          # Token files (github.token, telegram.token, brave.token, …)
├── missions/         # Brownfield mission clones
├── models/           # Whisper model (download separately)
├── recall.sqlite     # Cross-session semantic-recall index
├── todos.json        # Task list
├── models.toml       # Optional model-config overlay
├── fallback.jsonl    # Auto-fallback event log
├── .env              # Persistent env-var overrides
└── CLAUDETTE.MD      # Optional user memory (800-char cap)
```

Nothing outside `~/.claudette/` is written without explicit permission.

---

## Build from source

```bash
git clone https://github.com/mrdushidush/claudette
cd claudette
cargo build --release -p claudette
./target/release/claudette --help
```

Tests: **703 passing, 6 ignored** (4 POSIX-only hook tests, 2 live-recall smokes that need an LM Studio embedding server). Before committing: `cargo fmt --all && cargo clippy --all-targets --no-deps -- -D warnings && cargo test --lib`.

---

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). Quick version:

- File bugs at <https://github.com/mrdushidush/claudette/issues>.
- Conventional Commits: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `style:`, `ci:`.
- By contributing, you agree your work is licensed under Apache 2.0.

Security issues: please use the private advisory flow in [`SECURITY.md`](SECURITY.md) — don't open a public issue.

Be kind — [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) has the short version.

---

## License

Apache License 2.0 — see [LICENSE](LICENSE). Use, modify, redistribute commercially or personally. No trademark grant; don't imply endorsement.

Copyright © 2026 [mrdushidush](https://github.com/mrdushidush).
