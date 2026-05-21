# Claudette

**A local-first AI secretary that runs on your own laptop.** REPL, fullscreen TUI, one-shot CLI, and a Telegram bot — all driving the same [Ollama](https://ollama.com) backend. No cloud brain, no subscription, no [telemetry](PRIVACY.md). Single Rust binary.

### Install in 30 seconds

**Linux / macOS:**

```sh
curl -fsSL https://raw.githubusercontent.com/mrdushidush/claudette/main/install.sh | sh
```

**Windows (PowerShell):**

```powershell
iwr -useb https://raw.githubusercontent.com/mrdushidush/claudette/main/install.ps1 | iex
```

Then pull a brain and talk:

```sh
ollama pull qwen3.5:4b           # 3.4 GB brain — one-time download
claudette "what time is it?"
```

> Prefer not to pipe the network into a shell? Grab a signed archive from [Releases](https://github.com/mrdushidush/claudette/releases/latest) and unzip `claudette` (or `claudette.exe`) onto your `PATH`. SHA256 sidecar on every artifact.
>
> **Rust user?** `cargo install claudette` still works.
> **Don't have a GPU?** See [CPU-only mode](docs/hardware.md#no-gpu-cpu-only-mode) — the 4b brain runs on plain CPU, just slower.
> **First time?** Open [`docs/show-me.md`](docs/show-me.md) for plain-English examples — calendar, notes, weather, screenshots, voice from your phone.

[![Crates.io](https://img.shields.io/crates/v/claudette.svg)](https://crates.io/crates/claudette)
[![CI](https://github.com/mrdushidush/claudette/actions/workflows/ci.yml/badge.svg)](https://github.com/mrdushidush/claudette/actions/workflows/ci.yml)
[![Apache 2.0](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust 1.75+](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)

![Claudette TUI — chat + live tool-call panel side-by-side, one turn covering notes, weather, BTC price, and calendar](docs/images/claudette-tui.png)

> One turn driving four tool groups (`note_list`, `weather_forecast`, `tv_get_quote`, `calendar_list_events`) — the brain enables groups on demand and dispatches calls. TUI tabs: `[1]Chat [2]Tools [3]Notes [4]Todos [5]HW`.

---

## Why Claudette

The open-source AI agent space is crowded with coding-focused tools (Aider, Cline, OpenHands, opencode). Claudette is aimed at a different slot: **a general-purpose personal assistant you can voice-note from a bus stop, that runs entirely on your own laptop, with no cloud brain in the loop.**

- **Truly local by default.** No cloud-brain code path exists. Ollama on `localhost` is the only required dependency. Every outbound network call (voice TTS, Telegram, web search, GitHub, Google Calendar/Gmail) is opt-in and gated behind a feature you have to turn on. Full inventory in [`PRIVACY.md`](PRIVACY.md).
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

### 80+ tools, ~200 token base schema
Every tool except `enable_tools`, `get_current_time`, and `load_workspace_rules` lives in a group the model opts into via `enable_tools(group)`. **22 groups as of v0.6.0** (notes, todos, files, code, meta, git, ide, search, advanced, facts, registry, github, markets, telegram, calendar, schedule, gmail, recall, **quality** [run_tests / diagnostics / apply_patch], **semantic** [semantic_grep], **vision** [screenshot_capture / image_describe], **clipboard**) — schema cost stays flat until the model actually needs the surface.

### Brownfield missions: clone, edit, ship a PR — in one tool chain
`mission_start("owner/repo")` clones into `~/.claudette/missions/<slug>/` and silently re-routes `git_status` / `glob_search` / `grep_search` / `write_file` / `bash` into the mission tree. `mission_submit` auto-branches, commits, pushes, and opens the PR via `gh_create_pr`. Resumable across sessions via `mission_attach`.

### Forge-mode: autonomous code-change pipeline
`claudette --forge "<prompt>"` or `/forge <prompt>` runs a Planner → Coder → Verifier loop against the active mission, with a configurable fix-loop (default 2 rounds) before the PR opens. Roles are routable via `~/.claudettes-forge/models.toml` so you can pin a stronger model to Verifier and keep a cheap model on Coder. Inside an existing git repo with no mission active, forge auto-bootstraps an ephemeral mission rooted at the repo toplevel — no clone required. Full walkthrough: [`docs/forge.md`](docs/forge.md).

### Tiered-brain auto-fallback
Three presets (Fast / Auto / Smart). Auto runs `qwen3.5:4b` and escalates to `qwen3.5:9b` on stuck signals (empty response after retry, max-iterations hit with no text, ≥ 3 consecutive tool errors). Per-turn revert — not session-sticky. **For 16 GB+ VRAM, pin `qwen3.6-35b-a3b` instead** — see [Recommended models](#recommended-models).

### Voice in, voice out, and vision in
Whisper transcription for Telegram voice notes, edge-tts for replies (English or Hebrew). Image attachments in the TUI/REPL via Alt+V (clipboard), drag-drop, or `@/path/to/img.png` when the loaded brain is multimodal.

### Codet sidecar for code generation
`generate_code` routes through a dedicated coder model (default `qwen3-coder:30b`, fallback `qwen2.5-coder:14b`; **recommended upgrade `qwen3.6-35b-a3b`** — same model as the brain, no swap dance — see [Recommended models](#recommended-models)). Runs a real syntax check (`py_compile`, `rustc --emit=metadata`, `tsc --noEmit`, etc. — 5 languages), then an Aider-style SEARCH/REPLACE fix loop on failure, then optional pytest/cargo-test/jest. Hot-swaps into VRAM on demand on memory-constrained boxes.

### Cross-session semantic recall
`/recall <query>` searches past conversation turns across sessions via an embedding index (works on Ollama or LM Studio's `/v1/embeddings`). Drops fragments of relevant past turns straight into the current context.

### Three sub-agents
`spawn_agent` delegates to a Researcher (web + file + code search, 10 turn cap), GitOps (rebase/squash/push, 8 turn cap), or Code Reviewer (read-only, 5 turn cap). Only the final text comes back — sub-agent chatter doesn't pollute the main context.

### Per-tool permission gating
ReadOnly tools auto-allow, WorkspaceWrite tools auto-allow, DangerFullAccess prompts `[y/N]` every time (bash, `edit_file`, `git add/commit/push/checkout`, cross-org PRs). Telegram default-denies DangerFullAccess (no TTY).

---

## Hardware

The numbers below describe the *comfortable* setup. **You don't need a GPU** — Ollama runs on plain CPU (slower, but viable for a 1b/3b brain). See [`docs/hardware.md#no-gpu-cpu-only-mode`](docs/hardware.md#no-gpu-cpu-only-mode) if you don't have one.

| Component | Comfortable minimum | Recommended | Tested on |
|-----------|---------------------|-------------|-----------|
| GPU | 6 GB VRAM (or CPU-only with a smaller brain) | 8 GB VRAM | RTX 3060 Ti 8 GB |
| RAM | 16 GB | 32 GB | 32 GB DDR4 |
| Disk | ~3 GB (brain only) | ~27 GB (brain + fallback + 30b coder) | NVMe SSD |
| OS | Windows 10+, Linux, macOS | Windows 11 / Ubuntu 24.04 / macOS 14+ | Windows 11 Pro |

Full model footprint table, CPU-only recipes, and the 30b-coder-on-8GB-VRAM env recipe: [`docs/hardware.md`](docs/hardware.md).

> For the recommended `qwen3.6-35b-a3b` setup (best quality), see the [Recommended models](#recommended-models) section below — 16 GB VRAM or 32 GB RAM with CPU-MoE offload is the practical tier.

---

## Recommended models

The defaults (`qwen3.5:4b` brain / `qwen3-coder:30b` coder) are tuned for **broad hardware compatibility** — they install in under a minute and work on any 8 GB GPU or modern CPU. Beyond that, extensive testing (most recently the [100-prompt regression sweep on 2026-05-20](crates/claudette/tests/claudette100_prompts.txt) — 80% raw / ~98% adjusted, zero true regressions) has shown what works best at each tier:

### Brain

| Hardware tier | Recommended brain | Notes |
|---------------|-------------------|-------|
| 8 GB VRAM / 16 GB RAM | `qwen3.5:4b` (Q8) | Default. Fast, fits everywhere, tool-calling solid. |
| 16 GB VRAM / 32 GB RAM | **`qwen3.6-35b-a3b`** | Best overall by a wide margin. MoE — 35 B total / ~3 B active per token, needs CPU-MoE offload. ~24 t/s baseline / ~43 t/s with MTP on RTX 5060 Ti. |
| 24 GB+ VRAM | **`qwen3.6-35b-a3b`** (full GPU) | Top quality, full GPU residency. |

`qwen3.6-35b-a3b` is currently distributed via [LM Studio](https://lmstudio.ai/) (Unsloth GGUF) rather than packaged on Ollama. Flip the backend with `CLAUDETTE_OPENAI_COMPAT=1` — see [`docs/power-user.md`](docs/power-user.md#lm-studio-or-any-openai-compatible-server). When multiple quants are on disk, pin one explicitly (`CLAUDETTE_MODEL=qwen3.6-35b-a3b@q4_k_xl`) — LM Studio picks the smallest match otherwise.

### Codet sidecar coder

When you use `generate_code` or `--forge`:

1. **`qwen3.6-35b-a3b`** — best if the VRAM/RAM budget is there. Same model as the brain means no swap dance between turns.
2. **`qwen3-coder:30b`** — current default. Quality coder, available on Ollama, MoE-friendly on 8 GB VRAM with the [env recipe](docs/hardware.md#running-the-30b-coder-on-8-gb-vram--32-gb-ram).
3. **`qwen3.6-27b` (dense)** — top quality but **very tight on 16 GB VRAM** even at Q4; comfortable on 24 GB+.

Pin a non-default brain via `~/.claudette/.env` (`CLAUDETTE_MODEL=...`) or `/brain <model>` at runtime. Pin the coder via `CLAUDETTE_CODER_MODEL=...`.

---

## Quick start (full setup)

```bash
# 1a. Default path — Ollama with the 3.5 family (works on 8 GB VRAM).
ollama pull qwen3.5:4b           # brain (default Auto preset)
ollama pull qwen3.5:9b           # fallback brain (optional)
ollama pull qwen3-coder:30b      # Codet coder, only if you'll use generate_code

# 1b. Recommended path — LM Studio with qwen3.6 (best on 16 GB+ VRAM).
# Pull `qwen3.6-35b-a3b` from inside LM Studio, then in ~/.claudette/.env:
#   CLAUDETTE_OPENAI_COMPAT=1
#   OLLAMA_HOST=http://localhost:1234
#   CLAUDETTE_MODEL=qwen3.6-35b-a3b@q4_k_xl
#   CLAUDETTE_CODER_MODEL=qwen3.6-35b-a3b@q4_k_xl
# See `docs/power-user.md` for the full LM Studio recipe.

# 2. Install Claudette — pick one.
curl -fsSL https://raw.githubusercontent.com/mrdushidush/claudette/main/install.sh | sh   # Linux/macOS
iwr -useb https://raw.githubusercontent.com/mrdushidush/claudette/main/install.ps1 | iex  # Windows
cargo install claudette                                                                    # Rust users
# Or download an archive from https://github.com/mrdushidush/claudette/releases/latest

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
claudette --doctor               # diagnose Ollama, models, tokens, permissions
```

First launch auto-creates `~/.claudette/` and probes `http://localhost:11434`. Bypass the probe with `CLAUDETTE_SKIP_OLLAMA_PROBE=1` for offline sessions.

Out of the box: notes, todos, files, time, weather, Wikipedia, code search. Brave / GitHub / Google Calendar / Gmail tools light up when you set the relevant token — full table in [`docs/configuration.md`](docs/configuration.md). Want to see what to actually type? Open [`docs/show-me.md`](docs/show-me.md).

---

## Docs

- [`docs/show-me.md`](docs/show-me.md) — **start here:** plain-English example prompts (notes, calendar, vision, voice, code)
- [`docs/quickstart.md`](docs/quickstart.md) — 30-second start, common flows
- [`docs/configuration.md`](docs/configuration.md) — every env var, token file fallbacks, recall settings
- [`docs/power-user.md`](docs/power-user.md) — LM Studio recipe, brain pinning, forge knobs, context tuning
- [`docs/hardware.md`](docs/hardware.md) — VRAM/RAM/disk by preset, CPU-only mode, 30b-on-8GB env recipe
- [`docs/usage.md`](docs/usage.md) — CLI flags, slash commands, Telegram-only commands
- [`docs/architecture.md`](docs/architecture.md) — module layout, tool-group contract, Codet sidecar contract
- [`docs/forge.md`](docs/forge.md) — forge-mode pipeline, Submitter contract, `models.toml` schema, auto-bootstrap
- [`docs/comparison.md`](docs/comparison.md) — honest side-by-side vs. opencode / Aider / OpenHands / Cline / Continue
- [`docs/google_setup.md`](docs/google_setup.md) — Calendar + Gmail OAuth walkthrough
- [`docs/deploy.md`](docs/deploy.md) — Pi / VPS / home-server deploy via docker-compose
- [`editor/vscode/`](editor/vscode/README.md) — VS Code extension (REPL/TUI/forge/"ask about selection" commands)
- [`PRIVACY.md`](PRIVACY.md) — every place data can leave your machine, and the conditions for each

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
