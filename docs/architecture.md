# Architecture

The repo is a Cargo workspace with a single published member: `crates/claudette/`. Path references below are inside `crates/claudette/src/`.

## Module layout

```
src/
├── main.rs           — Binary entry point (arg parsing, Ollama probe, mode dispatch)
├── lib.rs            — Module declarations + public re-exports
├── runtime/          — Embedded agent-loop kernel (vendored)
│   ├── conversation.rs — Turn loop, tool dispatch, hook integration, ApiClient trait
│   ├── session.rs      — Session / ConversationMessage / ContentBlock types
│   ├── compact.rs      — Auto-compaction + token estimation
│   ├── permissions.rs  — Three-tier permission policy
│   ├── usage.rs        — TokenUsage + cumulative-usage tracker (local models = free)
│   ├── hooks.rs        — Pre/post tool-use hooks (shell snippets)
│   ├── prompt.rs       — ProjectContext discovery (cwd, git status, instruction files)
│   ├── config.rs       — Optional configuration loaders (settings.json: hooks, model)
│   └── json.rs         — Hand-rolled JSON for the no-serde-dep runtime paths
├── api.rs            — OllamaApiClient: /api/chat streamer, truncation, budget math, probe
├── run.rs            — Runtime builder, REPL loop, autosave, session compaction, forge pipeline
├── executor.rs       — SecretaryToolExecutor: enable_tools meta-tool + dispatch
├── tools.rs          — Aggregates per-group schemas + routes dispatch_tool() through each sub-module
├── tools/            — One module per tool cluster (calendar, codegen, facts, file_ops, git, github, gmail, ide, notes, registry, schedule, search, shell, telegram, todos, web_search)
├── tool_groups.rs    — ToolRegistry + the 21 on-demand tool-group definitions
├── codet.rs          — Code-generation sidecar (syntax check, surgical fix loop, tests)
├── test_runner.rs    — Python/Rust/JS/TS syntax + test runners
├── commands.rs       — Slash-command parsers and handlers
├── prompt.rs         — Claudette system prompt builder
├── model_config.rs   — Preset + RoleConfig + TOML overlay
├── brain_selector.rs — Tiered-brain fallback + stuck diagnostics
├── memory.rs         — CLAUDETTE.MD loader
├── secrets.rs        — File-backed token storage + Telegram chat-ID persistence
├── google_auth.rs    — Google OAuth loopback flow (per-scope token files)
├── clock.rs          — Clock trait (SystemClock in prod, MockClock for deterministic tests)
├── scheduler.rs      — Persistent jsonl scheduler with catch-up + natural-language expressions
├── briefing.rs       — Morning-briefing prompt (shared by /briefing and --briefing)
├── telegram_mode.rs  — Telegram bot loop (polling, voice, slash commands)
├── voice.rs          — Whisper transcription pipeline
├── tts.rs            — edge-tts TTS integration
├── theme.rs          — Colored output, emoji glyphs, TTY detection
├── tui.rs            — Ratatui TUI app, 5 tabs, render loop
├── tui_events.rs     — TUI event enums (worker ↔ render channel)
├── tui_executor.rs   — ToolExecutor wrapper that fires TUI events
├── tui_worker.rs     — Worker thread that owns the ConversationRuntime
└── forge/            — Forge-mode plumbing (personas, role-map, types) — folded back in v0.5.1
```

## The on-demand tool-group contract

`ToolRegistry` lives behind an `Arc<Mutex<_>>`. The `OllamaApiClient` reads it on every `/api/chat` request, so when the model calls `enable_tools("git")`, the executor mutates the shared registry and the next API call advertises the expanded tool list. Adding a new tool group is a three-step change (add enum variant, register tool set, document the group) and costs zero context until first use.

## Tool groups

21 groups, ~80 tools total as of v0.6.0 (added Quality, Semantic, Vision, Clipboard; collapsed 18 lesser-used tools into polymorphic merges + outright drops). Schema cost: ~840 chars (~210 tokens) on every turn until the model enables a group; the full 21-group surface is ~34 KB if every group is loaded at once. A follow-up will trim back toward the ~26 KB target by dropping the v0.6.0 deprecation-alias arms (still dispatched for one release) and tightening verbose descriptions.

The `gmail`, `calendar`, and `telegram` groups are compiled **only** into an `integrations` build (`cargo install claudette --features integrations`); the default coding-only binary omits them entirely — see [Install](../README.md#install).

| Group | Tools | What it does |
|-------|-------|--------------|
| **core** (always on) | 3 | `enable_tools` (the meta-tool), `get_current_time`, `load_workspace_rules` |
| `notes` | 4 | Personal notes — `note_create` (upsert), list, read, delete |
| `todos` | 4 | Todo list — add, list, set status, delete |
| `files` | 3 | `read_file`, `write_file`, `list_dir` |
| `code` | 1 | `generate_code` — routes through the Codet coder + validator pipeline |
| `meta` | 1 | `get_capabilities` — config, tool inventory, limits |
| `git` | 9 | status, diff, log, add, commit, branch, checkout, push, clone |
| `ide` | 3 | Open in editor (`code`), reveal in file manager, open URL in browser |
| `search` | 5 | `repo_map`, `grep_search`, `glob_search`, `web_search` (Brave), `web_fetch` |
| `advanced` | 7 | `bash` (+ `bash_background` / `bash_status` / `bash_tail`), `edit_file`, `apply_diff`, `ask_user` |
| `facts` | 2 | `wikipedia` (summary/search), `weather` (Open-Meteo) |
| `registry` | 2 | `crate_info` (crates.io), `npm_info` (npmjs) |
| `github` | 15 | PRs (status, view, fork, create), issues (inbox, get, create, comment, list-repo), code search, **brownfield missions** (start, state, submit), `forge_tail` |
| `telegram` | 1 | `tg_send` — bot messaging (text or photo). **`integrations` build only.** |
| `calendar` | 4 | Google Calendar: list / create / update / delete events (RSVP via update). **`integrations` build only.** |
| `schedule` | 4 | Proactive reminders: one-shot + recurring schedules that fire prompts back at you |
| `gmail` | 4 | Gmail (read-only): list, search, read, list labels — with `<email>` provenance wrapping. **`integrations` build only.** |
| `recall` | 1 | Cross-session memory: semantic search over past conversation turns (`recall <query>`) |
| `quality` | 3 | `run_tests`, `diagnostics` (cargo check / clippy / tsc / mypy / ruff), `apply_patch` (atomic multi-file unified diff) |
| `semantic` | 1 | `semantic_grep` — workspace search with token-overlap ranking (fuzzier than grep) |
| `vision` | 2 | `screenshot_capture` (PNG to `~/.claudette/files/`), `image_describe` (needs a VLM loaded in LM Studio) |
| `clipboard` | 2 | `clipboard_read`, `clipboard_write` (text only, 1 MB cap) |

## Codet sidecar contract

Codet is invoked exclusively through the `generate_code` tool. The main conversation never sees Codet's internal fix-loop exchanges — only the one-line summary + file path on disk. This is deliberate: Codet's iteration chatter would otherwise fill 20 KB of context per coding task.

Pipeline:

1. Writes the code with a dedicated coder model (default `qwen3-coder:30b`, fallback `qwen2.5-coder:14b`).
2. Runs a syntax check (`python -m py_compile`, `rustc --emit=metadata`, `tsc --noEmit` for JS + TS — 4 languages).
3. On failure, runs a **surgical SEARCH/REPLACE fix loop** (Aider-style patches, ~50 output tokens per attempt) before falling back to full-file regeneration.
4. Optionally runs associated pytest/cargo-test/jest suites.
5. Retries up to 3 times, then reports honestly if it can't fix the file.

Codet is hot-swapped into VRAM on demand — the main brain is evicted first on memory-constrained machines, then restored after Codet finishes.

## Forge-mode pipeline

`run_forge_mission` (in `run.rs`) orchestrates five phases against the active brownfield mission:

1. **Planner** — tool-less brain turn (`Role::Planner` from `~/.claudettes-forge/models.toml`) decomposes the user's request into a 3–5 step numbered plan, prepended to the Coder's input.
2. **Coder (round 0)** — full forge runtime (`files`, `search`, `git`, `advanced`, `github` groups enabled) with `should_submit=false`. Brain commits its change but the system prompt forbids `mission_submit`/`git_push`.
3. **Verifier** — tool-less brain turn (`Role::Verifier`) reads `git diff HEAD` and emits one-line JSON: `{"score": <1-10>, "pass": <bool>, "feedback": "<reason>"}`. Resilient to code fences and trailing prose.
4. **Fix-loop** — if `pass=false` and `round < MAX_FIX_ROUNDS` (2), re-runs the Coder with the Verifier's feedback prepended to the prompt.
5. **Submitter** — final Coder turn with `should_submit=true` that just calls `mission_submit`. PR opens here, never earlier.

Persona overlay: `personas/codex7.md` is baked into the binary via `include_str!` and parsed at startup. Its `voice` one-liner + backstory prose are appended to the forge-mode system prompt so the brain adopts a consistent style.

## Project layout rules

- Runtime modules (`src/runtime/*.rs`) are mounted at the crate root via `#[path = "runtime/..."]` attributes. Their internal `use crate::session::X` paths resolve without rewriting. Don't move these files or add `mod` declarations in `runtime/mod.rs`.
- All workspace-shared lints live in `crates/*/Cargo.toml` per-crate; the root `Cargo.toml` is a virtual manifest.
- Build the published crate explicitly with `cargo build -p claudette` (or `cargo build` for the whole workspace); `cargo test --lib` runs against every workspace member.

## Coding standards

- `#![forbid(unsafe_code)]` in the crate root — no unsafe.
- Clippy pedantic is on workspace-wide. Allow-list lives in `Cargo.toml` and covers ergonomic exceptions.
- `#[must_use]` on any function returning a non-trivial value.
- No `panic!` in production paths — every `Result` returns a typed error. Panics are only acceptable inside `#[cfg(test)] mod tests` blocks.
- Tests that mutate environment variables must acquire `crate::test_env_lock()` to avoid parallel-test races.

## Adding a new tool

1. Add a JSON schema entry to the relevant `src/tools/<group>.rs` (or create a new group module if none fits).
2. Add a `run_my_tool(input: &str) -> Result<String, String>` handler in the same module.
3. Wire it into the `dispatch` match at the top of the module.
4. For a new group: add a `ToolGroup` variant in `src/tool_groups.rs`, then register the group's `schemas()` and `dispatch()` in `src/tools.rs` (follow the existing groups as templates).
5. Add at least one unit test for the happy path and one for a known failure mode.
