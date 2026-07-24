# Sprint 2 — TUI + CLI wiring

**Target version:** Ships as part of v0.1.0 (user-facing MVP).
**Duration estimate:** 2-3 focused weeks. TUI work is intrinsically heavier than Sprint 1's API-surface work.
**Implementer:** Claude Sonnet (fresh session, cold start).
**Outcome:** `claudettes-forge tui` launches a ratatui chat backed by Ollama. `claudettes-forge doctor` probes Ollama reachability. CLI parses subcommands; `forge`/`verify`/`bench` print "available in Sprint N" stubs.

## Current status (2026-04-23, fresh Sprint 2 start)

- Sprint 1 core complete. Head `1eb9d0b`. 103 lib + 2 integration = **105 green**. Clippy pedantic green. Workspace check clean.
- `crates/tui/src/lib.rs` — 8-line stub.
- `crates/claudettes-forge/src/main.rs` — 16-line stub that prints "scaffold stub" and exits 1.
- `core/src/providers/ollama.rs` — fully ported in Sprint 1; callable via `OllamaProvider::chat()`.
- `models.toml` loader not yet written in core.
- No `clap` parser. No `ratatui`/`crossterm` deps. No CI (intentional — ships when repo gets a GitHub remote).

## Scope

### IN for Sprint 2

1. **CLI parser** — clap derive-based root + 5 subcommands (`tui`, `doctor`, `forge`, `verify`, `bench`).
2. **`tui` subcommand body** — ratatui event loop, chat tab, assistant turn wired to `OllamaProvider`, streaming token display.
3. **`doctor` subcommand body** — HTTP GET `{OLLAMA_HOST}/api/tags`, cross-check models.toml roles against available model list, report missing.
4. **Stub subcommands** — `forge` / `verify` / `bench` accept args, print `"{cmd} is available in Sprint N."` and exit 2.
5. **`models.toml` loader in core** — needed so the TUI respects user's model picks rather than hardcoded defaults.
6. **Paste-to-tempfile** — lifted from `D:/dev/tacticode/src/tui/paste.rs` (81 LOC, verbatim-lift).
7. **Typewriter code-block effect** — referenced from BCF `src/tui.rs`.
8. **Space Invaders easter egg** — redesigned from BCF `src/space.rs` (441 LOC) plus sanitized assets.

### OUT of Sprint 2 (do NOT drift into these)

- **Concrete tool implementations** (`file_read`, `file_write`, `shell_run`, web_fetch, etc.). The TUI ships as a tool-free chat REPL. Tools land in Sprint 3.
- **Forge pipeline stages** — Sprint 3.
- **Verifier body** — Sprint 4.
- **Integrations** (telegram, voice, mcp) — Sprint 6.
- **Anthropic provider** — v0.2.
- **Notes / Todos / Hardware tabs** from claudette's TUI — Sprint 2 is chat-only + space easter egg. Other tabs deferred.
- **Session autosave** / `learnings.md --promote` — Sprint 3 with forge pipeline.
- **CI workflow** — lands when repo gets a GitHub remote.
- **AD-6** — user decided skip (2026-04-23).

## Per-module lift plan

Paths relative to `D:/dev/claudettes-forge/`. `LOC (src)` is the claudette/tacticode/BCF source size for sizing work.

| Target module | Source | LOC | Porting strategy | Notes |
|---|---|:---:|---|---|
| `core/src/models_toml.rs` (new) | claudette `src/model_config.rs` | 415 | **Refactor** | Port the `TomlOverlay` + `apply_role_override` + `default_toml_path` machinery. `ModelMap` already exists in `core::types` from Sprint 1; add `ModelMap::from_file(path) -> Result<ModelMap, ModelsTomlError>`. Env-var `CLAUDETTE_*` → `CLAUDETTES_FORGE_*`. Path: `~/.claudettes-forge/models.toml`. Presets (`Preset::Fast`/`Balanced`/`Premium`) may or may not carry over — default to keeping if it's cheap. |
| `claudettes-forge/src/cli.rs` (new) | — | — | **New** | clap `Args`/`Parser` derive for root + 5 subcommands. One module, ~150 LOC target. |
| `claudettes-forge/src/main.rs` (rewrite) | claudette `src/main.rs` (subcommand-dispatch pattern) | 850 | **Reference only, do not verbatim lift** | claudette's main.rs is 850 LOC doing the whole app; we only need dispatch. Keep main.rs <100 LOC. |
| `claudettes-forge/src/doctor.rs` (new) | claudette `src/main.rs::run_doctor` (search for it) | ~80 | **Refactor** | If claudette has a doctor-equivalent, port. Else write fresh: HTTP GET Ollama `/api/tags`, compare to `models.toml`, print health table. Emit `warn`/`error` to stderr, exit 0/1. |
| `tui/Cargo.toml` | — | — | **New** | Add `ratatui = "0.29"`, `crossterm = "0.28"`, `claudettes-forge-core = { path = "../core" }`. |
| `tui/src/app.rs` (new) | claudette `src/tui.rs::App` struct (line 192-357) + `run_loop` (595-852) | 1690 → ~400 | **Refactor (significant prune)** | Strip to chat-only. Drop Notes/Todos/Hw/Tools tabs. Keep: `App` struct, event loop, tab switching (reduce to chat + hidden space). |
| `tui/src/chat.rs` (new) | claudette `src/tui.rs::render_chat_tab` + `render_messages` (927+) | ~300 of 1690 | **Refactor** | Message buffer, auto-scroll with pin-on-manual-scroll. Syntax-highlighted code blocks (defer syntect — plain with colored monospace OK for v0.1). |
| `tui/src/input.rs` (new) | claudette `src/tui.rs` (input handling around `run_loop`) | ~200 of 1690 | **Refactor** | Prompt area, cursor, paste detection → call into paste.rs. Key bindings. |
| `tui/src/worker.rs` (new) | claudette `src/tui_worker.rs::spawn_worker` (line 93) + `build_tui_runtime` (38) | 190 | **Refactor** | Background thread running `Provider::chat()` blocking call. mpsc channel to UI for token deltas + final response. **Critical:** match Sprint 1's blocking-provider pattern (no tokio in Sprint 2). |
| `tui/src/events.rs` (new) | claudette `src/tui_events.rs` | ~50 | **Verbatim-ish lift** | `TuiEvent` enum — input / ollama-chunk / ollama-done / ollama-err / tick. |
| `tui/src/paste.rs` (new) | tacticode `src/tui/paste.rs` | 81 | **Verbatim lift** | Paste buffer → tempfile when paste exceeds N chars. Windows-safe path handling. |
| `tui/src/typewriter.rs` (new) | BCF `src/tui.rs` (typewriter section) | slice | **Refactor + slim** | 12 chars/tick (~240/sec), blinking block cursor, safe UTF-8 boundary. |
| `tui/src/space.rs` (new) | BCF `src/space.rs` | 441 | **Refactor + sanitize** | Space Invaders as overlay on top of chat tab. Trigger via `/space` slash-command. Memory says "redesigned from battleclaw-v2" — keep gameplay, adjust visual palette to match TUI theme. |
| `tui/src/lib.rs` (rewrite) | — | — | **New** | Re-export `pub fn run_tui(models: ModelMap) -> Result<()>` as single public entry. |

## Concrete checklist

One step = one commit. Green `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` between each step.

1. **[x]** Write this doc (done when you read it).
2. **[x]** `core/src/models_toml.rs` — TOML loader + `ModelMap::from_file`. 8 unit tests. `45dcc3a`
3. **[x]** `claudettes-forge/Cargo.toml` — clap + reqwest + serde deps. `11a64e3` + `3927c35`
4. **[x]** `claudettes-forge/src/cli.rs` — clap derive structs. `3be0a31`
5. **[x]** `claudettes-forge/src/main.rs` — subcommand dispatch. `a7384fa`
6. **[x]** `claudettes-forge/src/doctor.rs` — Ollama probe + health table. `3927c35`
7. **[x]** Smoke-test CLI: `--help` ✓, `doctor` ✓ (live Ollama, 8 models), `tui --help` ✓. `this commit`
8. **[x]** `tui/Cargo.toml` — ratatui 0.29 + crossterm 0.28 + core dep. (batched into step 13 commit)
9. **[x]** `tui/src/events.rs` — `TuiEvent` enum. (batched into step 13 commit)
10. **[x]** `tui/src/app.rs` — bare App struct + event loop skeleton, no LLM yet. Renders an empty chat panel + input box. `q` quits. (batched into step 13 commit)
11. **[x]** `tui/src/chat.rs` — message buffer (`Vec<Message>`), render to scrollable paragraph. Auto-scroll + pin-on-manual-scroll. (batched into step 13 commit)
12. **[x]** `tui/src/input.rs` — prompt area, cursor, line editing, Enter → submit. (batched into step 13 commit)
13. **[x]** `tui/src/worker.rs` — background thread calling `OllamaProvider::chat()`. mpsc channel streams token deltas back to UI. Test with a mock provider implementing the `Provider` trait. `ad9fbbb`
14. **[x]** Wire app ↔ worker: Enter → spawn turn → render deltas → append final message. First end-to-end turn green. `afc0636`
15. **[x]** `tui/src/paste.rs` — verbatim-port from tacticode. Integrate with input.rs (detect paste > threshold → tempfile). `1e06cdc`
16. **[x]** `tui/src/typewriter.rs` — typewriter effect for code-block rendering in chat pane. `92db675`
17. **[x]** `tui/src/space.rs` — Space Invaders overlay. `/space` slash command triggers. Esc closes. `3f68e8e`
18. **[x]** Integration test at `tui/tests/smoke.rs` — mock provider (impls `Provider`), headless ratatui via `TestBackend`, send "hello", assert final message rendered. `d0a5248`
19. **[x]** Exit-criteria sweep — all gates green. 134 tests (23 new TUI/CLI). Tagged `v0.1.0-rc1`.

## Dependencies to add

### `crates/claudettes-forge/Cargo.toml` (binary)

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
claudettes-forge-core = { path = "../core" }
claudettes-forge-tui = { path = "../tui" }
```

### `crates/tui/Cargo.toml` (TUI library)

```toml
[dependencies]
ratatui = "0.29"
crossterm = "0.28"
claudettes-forge-core = { path = "../core" }
```

### `crates/core/Cargo.toml` (addition for models_toml)

No new deps — `toml` and `serde` are already there from Sprint 1.

### Deliberately NOT added in Sprint 2

- `tokio` / `async` — Sprint 2 stays blocking + threads, matching the Sprint 1 provider pattern. Async migration would be a workspace-wide decision, not a Sprint 2 scope item.
- `syntect` (syntax highlighting) — plain monospace in chat for v0.1. Add in Sprint 3 if demand justifies the compile-time cost.
- `tracing` — observability baseline is tagged as minor follow-up in the audit; add when it becomes load-bearing, not speculatively.

## Exit criteria

- [x] `cargo build --workspace` clean, zero warnings.
- [x] `cargo test --workspace` green. 135 total (112 Sprint 1 + 23 new TUI/CLI). Target met. (Sprint 1 gained 1 CRLF regression test during pre-Sprint-3 audit.)
- [x] `cargo clippy --workspace --all-targets -- -D warnings` green.
- [x] `cargo fmt --all --check` clean.
- [x] `claudettes-forge --help` prints the 5 subcommands.
- [x] `claudettes-forge doctor` probes Ollama, lists models, reports health table, exits 0 on healthy / 1 on missing.
- [x] `claudettes-forge tui` launches ratatui, accepts input, streams response from Ollama, auto-scrolls chat. (manual + smoke test verified)
- [x] `claudettes-forge forge "..."` / `verify --path .` / `bench ...` print "available in Sprint N" stubs.
- [x] `~/.claudettes-forge/models.toml` loads correctly; env var overrides work; defaults applied for missing roles.
- [x] Paste-to-tempfile triggers on large paste.
- [x] Typewriter effect renders on fenced code blocks in chat.
- [x] `/space` easter egg launches and exits cleanly.

## Known risks

1. **Streaming + ratatui interaction.** The `OllamaProvider::chat()` blocks; the TUI has to render while the worker streams. Pattern: worker thread + `mpsc::sync_channel(bound)` for back-pressure; UI polls channel each event-loop tick. Don't use `spawn_blocking` on a tokio runtime — we're staying sync.
2. **Ratatui API churn.** 0.29 → 0.30 shipped recently with widget changes. Pin `ratatui = "=0.29"` and `crossterm = "=0.28"` explicitly for Sprint 2. Bump as a follow-up once app.rs stabilizes.
3. **Typewriter + scroll collision.** Streaming pushes lines down while auto-scrolling. If user scrolls up to read history, auto-scroll shouldn't snap them back. Implement pin-on-manual-scroll: set `auto_scroll = false` when user hits `PgUp`/`Up`, reset to `true` on `End`/`PgDn`-to-bottom.
4. **Windows paste handling.** tacticode's `paste.rs` was pre-validated for Windows path semantics — test on Windows cmd.exe AND PowerShell ASAP. crossterm's `KeyEventKind` handling differs across terminals.
5. **Space Invaders asset weight.** BCF's `space.rs` is 441 LOC — see how much of it is game-state vs render. Redesign may want to slim to ≤200 LOC if it's mostly boilerplate.
6. **Terminal color/theme portability.** claudette has a theme module (`src/theme.rs`); Sprint 2 can hardcode a single theme to avoid scope drift. Configurable theme → post-v0.1 polish.
7. **`doctor` when Ollama is offline.** HTTP timeout should be short (e.g. 2s) — no one waits 30s to learn Ollama isn't running. Also handle DNS failure cleanly.

## After this sprint

**Sprint 3 — forge crate + concrete tools.** Landing targets:
- `crates/forge` — 7-stage pipeline (Router → Planner → Coder → TestCoder → Verifier → SurgicalCoder → Gate).
- `crates/core/src/tools/` — concrete tool implementations starting with `file_read`, `file_write`, `shell_run`, then `grep`, `web_fetch`, `gh_*`.
- Session autosave + `learnings.md --promote` flow.
- Context-budgeting port from claudette (`history_budget_chars`, `truncate_to_budget`).
- Branch-per-mission + synthetic-author commits.

After Sprint 3 the binary can actually *do work* — Sprint 2 alone ships a chat-only REPL.

## Next-session kickoff checklist

**If you are Sonnet picking up Sprint 2 cold, do this IN ORDER:**

1. Read this file (`docs/sprints/sprint_02_tui.md`) end-to-end.
2. Read `docs/sprint_01_audit.md` §13 (deferrals map) to understand what's out of scope.
3. Skim `docs/sprints/sprint_01_core.md` for the format + conventions.
4. Verify current state:
   ```bash
   cd D:/dev/claudettes-forge
   cargo test --workspace      # expect 105 green
   cargo clippy --workspace --all-targets -- -D warnings   # expect clean
   git status                   # expect "working tree clean"
   git log --oneline -6         # expect head 1eb9d0b
   ```
5. Start at checklist step 2 (`core/src/models_toml.rs`). Reference `D:/dev/claudette/src/model_config.rs` for loader shape. ~415 LOC of source, your port will be smaller (no Preset UI — programmatic only).

## Hard constraints for the implementer

- **Git identity.** Repo has no local or global git config. Every commit MUST use `git -c user.name="mrdushidush" -c user.email="mrdushidush@gmail.com" commit …`. All Sprint 1 commits use this pair — preserve continuity.
- **Never run `git config --global`.**
- **Conventional Commits.** Examples from Sprint 1 history: `feat(core): port Google OAuth 2.0 loopback flow`, `test(core): add end-to-end smoke integration test`, `docs(sprint-1): land post-Sprint-1 audit`. Sprint 2 scopes: `feat(cli)`, `feat(tui)`, `feat(core)` (for models_toml only), `test(tui)`, `docs(sprint-2)`.
- **One step = one commit.** Don't batch multiple checklist items into a single commit unless they're genuinely atomic.
- **Green between steps.** After each step, `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --check` all pass before committing.
- **Do not scope-creep.** The OUT-OF-SCOPE list is binding. If you find yourself reaching for `file_read`/`shell_run` — stop, that's Sprint 3. If you find yourself wanting Notes/Todos tabs — stop, those are v0.2.
- **Do not touch Sprint 1 code** unless a TUI/CLI need forces a revision. If that happens, commit the Sprint-1 revision separately with a justifying commit message before the Sprint 2 work that depends on it.
- **Avoid `unwrap()` in library code.** `core` and `tui` should use `Result` everywhere. `unwrap()` is OK in tests and in `main.rs` where a panic-to-user-error conversion is the natural boundary.
- **MSRV is 1.75.** Do not use post-1.75 features without a discussion. (Let-else, let-chains, etc. were all stable before 1.75 — you're fine with ordinary idiomatic Rust.)
- **Clippy pedantic.** Workspace runs `clippy --all-targets -- -D warnings` with pedantic on in the same mode as Sprint 1. Don't blanket-allow lints; fix them or explain the exception in a scoped `#[allow(...)]` with a comment.
- **If you hit a blocker**, write a blocker note in the step's commit message (`feat(tui): WIP — blocked on X`) and stop. Don't invent unscoped solutions.

---

**Written 2026-04-23 by Opus 4.7 (1M context); handed off to Sonnet for implementation.**
