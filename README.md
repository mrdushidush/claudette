# Claudette

**An air-gapped AI coding agent that runs entirely on your own hardware.** One Rust binary, one local model through [Ollama](https://ollama.com) or [LM Studio](https://lmstudio.ai/), no cloud. Run it with `--offline` and it physically can't reach the network - your code and your prompts can't leave the machine. (It's a capable personal assistant too - notes, calendar, Telegram voice - but the headline is a coding agent you can run on a no-cloud box.)

[![Crates.io](https://img.shields.io/crates/v/claudette.svg)](https://crates.io/crates/claudette)
[![CI](https://github.com/mrdushidush/claudette/actions/workflows/ci.yml/badge.svg)](https://github.com/mrdushidush/claudette/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Air-gap: enforced](https://img.shields.io/badge/air--gap-enforced-success.svg)](#-air-gapped-and-enforced)

![Claudette TUI - chat alongside a live tool-call panel](docs/images/claudette-tui.png)

---

## 🔒 Air-gapped, and enforced

`claudette --offline` (or `CLAUDETTE_OFFLINE=1`) hard-blocks every outbound call except your local model server and loopback. Web search, GitHub, Telegram, Google, even `git push` all refuse with a clear `blocked by offline mode` error. Two guard layers cover in-process HTTP *and* subprocesses (`git`, `gh`, TTS), and an integration test drives every networked tool to prove each one refuses - the air-gap is tested, not just documented. `claudette --offline --doctor` prints the exact allow-list.

There's no cloud-brain code in the binary to begin with, so there's no "private mode" to switch on - there is no other mode. Nothing is written outside `~/.claudette/` without a prompt. Full inventory of every place a byte could leave: [PRIVACY.md](PRIVACY.md).

---

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/mrdushidush/claudette/main/install.sh | sh   # Linux / macOS
iwr -useb https://raw.githubusercontent.com/mrdushidush/claudette/main/install.ps1 | iex  # Windows
cargo install claudette                                                                    # Rust
```

```sh
ollama pull qwen3.5:4b        # 3.4 GB model, one-time download
claudette "what time is it?"
```

Prefer not to pipe curl into a shell? Grab a [prebuilt release](https://github.com/mrdushidush/claudette/releases/latest) - each ships a SHA256. No GPU? The 4B model runs on plain CPU. Full setup and first flows → [docs/quickstart.md](docs/quickstart.md).

---

## What it does

| Mode | Command | For |
|------|---------|-----|
| **REPL** | `claudette` | Conversational shell; autosaves every turn |
| **One-shot** | `claudette "..."` | Print a reply and exit; pipe-friendly |
| **TUI** | `claudette --tui` | Fullscreen UI, 5 tabs, live tool panel |
| **Telegram** | `claudette --telegram` | Voice-capable chat from your phone |

- **80+ tools across 22 opt-in groups.** The model turns a group on (`enable_tools("git")`) only when it needs it, so the base schema stays ~200 tokens however many tools exist. Point Claudette at a repo and the coding core - files, search, tests - is pre-enabled.
- **Forge - an autonomous code pipeline.** `claudette --forge "<task>"` runs Planner → Coder → Verifier → fix-loop → Submitter. The Verifier actually builds and runs the tests each round (`cargo`, `go`, `pytest`, `npm`), so a diff that doesn't compile or breaks a test can't pass - and no PR opens until you approve the plan and the full diff. → [docs/forge.md](docs/forge.md)
- **Brownfield missions.** `mission_start("owner/repo")` clones a repo, routes file ops into it, and `mission_submit` branches, commits, pushes, and opens the PR - one tool chain.
- **Also a personal assistant.** Notes, todos, calendar, Gmail, weather, markets, web search, and a Telegram bot with voice in (Whisper) and out (edge-tts, English or Hebrew).
- **Tiered brain, recall, vision.** Auto-escalates the 4B model to 9B only on real stuck signals; `/recall` searches every past session through a local embedding index; image attachments work when the loaded model is multimodal.
- **Per-tool permissions.** Read-only and workspace-write tools auto-allow; `bash`, `edit_file`, and `git push` prompt `[y/N]` every time.

### 🔁 She helps build herself

Claudette is developed *with* Claudette. She runs her own Forge pipeline against this repo, clears the real build-and-test gate (`cargo fmt` / `clippy -D warnings` / `cargo test`) before anything is pushed, and opens genuine pull requests under her own git identity - so she shows up as a [listed contributor on this repo](https://github.com/mrdushidush/claudette/graphs/contributors). A human reviews and merges every change; nothing lands on `main` unattended. Features shipped this way include `repo_map` C#/Java support, `read_file tail=N`, and `grep_search count_only`.

---

## 🏅 Which model should I run?

Every candidate runs the same objective 50-task battery - 11 languages × 12 task types - through Claudette's real tool loop, then an automated verifier checks the result (build/test passes, file is correct, ground-truth tokens appear). No model grades itself. `claudette --doctor` reads your VRAM and names the model that fits your GPU, with the load command.

| Model | VRAM | Pass @ 50 | Best for |
|-------|------|-----------|----------|
| **`qwen3.6-35b-a3b`** (q3_k_xl) | 16 GB | **92%** | Best accuracy - the default |
| `qwen3.5-4b` | **8 GB** | 90% | Best value - runs almost anywhere |
| `gpt-oss-20b` | 13 GB | 86% | Fastest |

Full table, methodology, and the reusable harness → [MODEL-COMPARISON.md](runs/eval-2026-05-29/battery/MODEL-COMPARISON.md). Benching a model we haven't covered is the single most useful way to contribute - no Rust required.

Runs on 8 GB VRAM or plain CPU; 16 GB for the 35B brain. Footprint details → [docs/hardware.md](docs/hardware.md).

---

## Docs

- [docs/show-me.md](docs/show-me.md) - **start here:** plain-English example prompts
- [docs/quickstart.md](docs/quickstart.md) - full setup, common flows, tokens
- [docs/configuration.md](docs/configuration.md) - every env var and token fallback
- [docs/power-user.md](docs/power-user.md) - LM Studio recipe, brain pinning, forge knobs
- [docs/hardware.md](docs/hardware.md) - VRAM/RAM/disk by preset, CPU-only mode
- [docs/usage.md](docs/usage.md) - CLI flags, slash commands, Telegram commands
- [docs/architecture.md](docs/architecture.md) - module layout, tool-group + Codet contracts, storage layout
- [docs/forge.md](docs/forge.md) - forge pipeline, brownfield missions, `models.toml`
- [docs/comparison.md](docs/comparison.md) - side-by-side vs. opencode / Aider / OpenHands / Cline / Continue
- [docs/deploy.md](docs/deploy.md) - Pi / VPS / home-server via docker-compose
- [editor/vscode/](editor/vscode/README.md) - VS Code extension
- [PRIVACY.md](PRIVACY.md) - every place data can leave, and the conditions for each

---

## Build from source

```bash
git clone https://github.com/mrdushidush/claudette && cd claudette
cargo build --release -p claudette
```

1,000+ tests, green on CI. Before committing: `cargo fmt --all && cargo clippy --all-targets --no-deps -- -D warnings && cargo test --lib`.

---

## Contributing

Bugs and PRs welcome - see [CONTRIBUTING.md](CONTRIBUTING.md). Conventional Commits (`feat:`, `fix:`, `docs:`, …). Security issues go through the private advisory flow in [SECURITY.md](SECURITY.md), not a public issue. Contributions are dual-licensed MIT OR Apache-2.0.

## License

Dual-licensed under **MIT** ([LICENSE-MIT](LICENSE-MIT)) **OR Apache-2.0** ([LICENSE-APACHE](LICENSE-APACHE)), at your option. The Apache option adds a patent grant; neither grants a trademark.

© 2026 [mrdushidush](https://github.com/mrdushidush).
