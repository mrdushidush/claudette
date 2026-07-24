# Sprint 3 — forge pipeline + concrete tools

**Target version:** Ships as v0.2.0 (first mission-capable release).
**Duration estimate:** 3-5 focused weeks. Largest single sprint in the plan (6 tools + 7 stages + orchestration + CLI + session flow).
**Implementer:** Claude Sonnet (fresh session, cold start) after user sign-off on §"Decisions needed before implementation starts" below.
**Outcome:** `claudettes-forge forge "<prompt>"` runs a real mission through 7 pipeline stages against a live Ollama, writes generated files to disk, runs Rust verifier checks, produces a gated quality report. `claudettes-forge learnings promote` flows mission findings into `CLAUDETTES_FORGE.md`. Six concrete tools (`file_read`, `file_write`, `shell_run`, `grep`, `web_fetch`, `gh_*`) land in `core` behind the 5-tier permission gate.

## Source repo correction — read this before anything else

The earlier `next_session_plan.md` said *"reference `D:/dev/claudette/src/mission.rs`"*. **That file does not exist.** The 7-stage pipeline in AD-4 is a downscope of the 9-stage pipeline that lives in **BCF-ABCC**, an internal iteration of battle-command-forge kept inside the abcc_projects monorepo.

Two battle-command-forge repos — do not confuse:

| Alias | Path | What it is |
|---|---|---|
| **BCF-SHIPPED** | `D:/dev/battle-command-forge/` | Shipped v0.1.0 on GitHub (`mrdushidush/battle-command-forge`, Apache-2.0, ~3.7 MB binary). Public release. |
| **BCF-ABCC** | `D:/dev/abcc_projects/abcc_projects/battle-command-forge/` | Internal iteration with the fuller 9-stage pipeline + 30 modules + empirically-validated learnings. **This is the port source.** |

BCF-ABCC's `CLAUDE.md` lists 29 numbered benchmark-validated learnings (§Key Learnings) — treat them as invariants while porting.

Claudette's role in Sprint 3: **tool source** (`src/tools/*.rs`), not pipeline source. The tool porting table below lifts from `D:/dev/claudette/src/tools/`.

## Current status (2026-04-24, fresh Sprint 3 start)

- Sprint 2 complete. Tag `v0.1.0-rc1` on `28e0e74`. 135 tests green from a fresh Windows clone. Working tree clean.
- AD-6 toolchain baseline landed (MSRV 1.85; `.gitattributes` with `* text=auto eol=lf`).
- `core/src/tool.rs` exposes the `Tool` trait + `ToolRegistry` + 15 `ToolGroup` variants. **Zero concrete tool implementations exist.**
- `core/src/tools/` does not exist yet.
- `forge/src/lib.rs` is a 6-line stub.
- Pedantic is enforced in `core` only; the other six crates compile under default clippy. **Measured fallout 2026-04-24:** tui=42, claudettes-forge=46, forge=3, bench=5, verifier=0, integrations=0 → **96 warnings total**.
- `OLLAMA_HOST` resolution is forked across three sites: `tui/src/worker.rs::normalise_host`, `claudettes-forge/src/doctor.rs::resolve_host`, `core/src/providers/ollama.rs::resolve_ollama_host` (private).

## Architectural decisions resolved (2026-04-24, captured in AD-7)

Sprint 3 kickoff surfaced five choices. User decisions signed off 2026-04-24; AD-7 in `docs/decisions.md` is the durable record. Summary:

- **D1 — Coder → TestCoder order.** AD-4 literal order confirmed (production code first, then test suite validates it). BCF-ABCC's TDD-first empirical advantage noted but not adopted; if benchmarks show an unrecoverable gap, revisit.
- **D2 — Gate = single LLM call with structured JSON.** Security verdict + 5-score critique + CTO verdict emerge from one call. Promotion to 3 sub-stages only if benchmarks show unstable judgements.
- **D3 — Pipeline stays sync.** No `tokio` / `async fn` / `.await` in Sprint 3 deltas. Threads + `std::sync::mpsc` is the concurrency primitive, consistent with Sprint 2 worker. Tokio retrofit remains an independent workspace-wide decision.
- **D4 — `models.toml` gains optional `context_size` + `max_predict` per role.** Backwards-compatible with Sprint 2 files (defaults logged when missing). Role mapping: Router→`complexity`, Planner→`architect`, Coder→`coder`, TestCoder→`tester`, SurgicalCoder→`fix_coder`, Gate→`critique`. Verifier is code, not LLM.
- **D5 — Pre-flight items** (pedantic workspace-wide + `OLLAMA_HOST` helper) land as steps 1 and 2. 96 warnings measured; 4-6h budget.

**Quality gate threshold is bench-driven, not hardcoded.** Sprint 3 ships with default **8.0** (user's "good enough to ship" bar). Configurable via `models.toml::pipeline.gate_threshold` + CLI `--gate-threshold`. BCF-ABCC's 9.2/8.5/8.0 complexity-scaled tiers stay available as an "aspirational" preset — reachable with cloud models, documented as such. The user plans to run local-only + local+cloud benchmarks after Sprint 3 to tune the default; the tuning happens outside Sprint 3 scope.

## Scope

### IN for Sprint 3

1. **Pre-flight #1** — `#![warn(clippy::pedantic)]` + `#![allow(clippy::module_name_repetitions)]` on all 6 remaining member crates; fix ~96 warnings.
2. **Pre-flight #2** — `core::providers::resolve_ollama_host` public helper; collapse 3 current sites.
3. **Concrete tools in `core/src/tools/`** — `file_read`, `file_write`, `shell_run`, `grep`, `web_fetch`, `gh_pr_create` + `gh_issue_create`. Each registers into `ToolRegistry` at the correct `PermissionTier`.
4. **`core/src/context.rs`** — verbatim port of BCF-ABCC `context.rs` (137 LOC). 95% compaction threshold, 60% target.
5. **`forge` crate — 7 stages in AD-4 order:** Router → Planner → Coder → TestCoder → Verifier → SurgicalCoder → Gate. Each stage in its own module; `MissionRunner` orchestrates.
6. **Surgical-by-default fix-pass loop** (BCF-ABCC `MAX_FIX_ROUNDS = 5`). Smart-stopping on 2-round score decline. Best-round restore.
7. **Configurable quality gate.** Default threshold 8.0 (`MissionConfig::gate_threshold`). Optional `--gate-preset aspirational` enables BCF-ABCC's 9.2/8.5/8.0 Campbell-tier ladder. Threshold is bench-tuneable via `models.toml`.
8. **`claudettes-forge forge "<prompt>"`** — replace Sprint 2 stub with real dispatch into `forge::run_mission`. Progress events to stderr.
9. **Session autosave** — `~/.claudettes-forge/sessions/<mission-id>.md` append-only journal: prompt, stage transitions, per-round scores, verdicts.
10. **`claudettes-forge learnings promote <session-id>`** — interactive subcommand; read session journal, user picks which findings to graduate, append to `CLAUDETTES_FORGE.md` (no free-form edit).
11. **`--git-workspace` flag** — when set and the cwd is a git repo, create branch `mission/<id>` and commit generated files with synthetic author `mrdushidush-forge-mission <mrdushidush@gmail.com>`.

### OUT of Sprint 3 (scope-guard — binding)

- **Anthropic / Grok / any cloud provider** — v0.2 (AD-3). Sprint 3 stays Ollama-only.
- **Swarm mode** — BCF-ABCC `swarm.rs` (329 LOC). Deferred per decisions.
- **Editor mode** (`claudettes-forge edit <path>`) — BCF-ABCC `editor.rs` (321 LOC). v0.2.
- **SWE-bench / benchmark / stress** — Sprint 7 (bench).
- **Interactive CTO chat agent** — BCF-ABCC `cto.rs` (748 LOC, 10-tool interactive agent). Sprint 3 folds CTO into `Gate` as a prompt role only. Interactive CTO is v0.2.
- **Sandbox enforcement** — BCF-ABCC `sandbox.rs` (390 LOC). Sprint 5 (security hardening). Sprint 3's `shell_run` relies on the 5-tier permission gate alone.
- **Language-agnostic verifier** — Sprint 3's Verifier runs `cargo fmt/clippy/test` only. Python (pytest/ruff/venv) port is v0.2.
- **Tools beyond the 6 listed** — calendar, gmail, markets, notes, schedule, telegram, todos, IDE, codegen-as-tool: S6 (integrations) or v0.2.
- **LanceDB / rich memory** — v0.2 behind `--features rich-memory`.
- **Multi-model per-role VRAM offloading optimisation** — nice-to-have; not a v0.2 blocker.
- **Rich-JSON / HTML mission reports** — v0.2. Sprint 3 prints plain-text + the autosave journal.
- **`claudettes-forge tui` forge integration** — Sprint 3 wires CLI only. TUI forge-tab is a follow-up sprint (likely part of S4 polish).

## Per-module lift plan

Source sizes refer to BCF-ABCC (`D:/dev/abcc_projects/abcc_projects/battle-command-forge/src/`) except where noted.

| Target module | Source | LOC | Strategy | Notes |
|---|---|:---:|---|---|
| `core/src/tools/mod.rs` (new) | — | — | **New** | Module index + re-exports. Registers all 6 tools via a helper. |
| `core/src/tools/file_read.rs` (new) | claudette `src/tools/file_ops.rs` | 468 (read subset) | **Refactor** | Path-bound open, UTF-8 decode, size cap, optional `offset`/`limit` args. Tier: `ReadOnly`. |
| `core/src/tools/file_write.rs` (new) | claudette `src/tools/file_ops.rs` | 468 (write subset) | **Refactor** | Write + Edit (find-replace) + atomic rename. Tier: `WorkspaceWrite`. Gate on `Operation::WriteFile`. |
| `core/src/tools/shell_run.rs` (new) | claudette `src/tools/shell.rs` | 316 | **Refactor** | `std::process::Command` + timeout (thread + kill) + stdout/stderr capture + env-strip. Tier: `DangerFullAccess`. No sandbox yet (S5). |
| `core/src/tools/grep.rs` (new) | claudette `src/tools/search.rs` | 361 (partial) | **Refactor** | Prefer `rg` shell-out; fall back to hand-rolled walker if `rg` absent. Match-format + context lines + glob filter. Tier: `ReadOnly`. |
| `core/src/tools/web_fetch.rs` (new) | claudette `src/tools/web_search.rs` | 175 (partial) | **Refactor** | Blocking GET + HTML text extraction (via `html2text`) + redirect limit + size cap. Wrap body in `<web-fetch …>` provenance envelope. Tier: `Network`. |
| `core/src/tools/gh.rs` (new) | claudette `src/tools/github.rs` | 481 (subset) | **Refactor** | Two tools only: `gh_pr_create`, `gh_issue_create`. Shell out to `gh` CLI (user's auth). Tier: `Network` + `Execute`. |
| `core/src/context.rs` (new) | BCF-ABCC `context.rs` | 137 | **Verbatim lift** | Tiny module, ports cleanly. Keep 95% threshold, 60% target constants + all 3 unit tests. |
| `core/src/sessions.rs` (new) | — | — | **New** | Append-only markdown journal. Filename `<mission-id>.md`. Per-stage sections. Cap each turn at N chars (use `memory::MAX_MEMORY_CHARS` as ceiling). |
| `forge/src/types.rs` (new) | BCF-ABCC `mission.rs` (lines 19-108) | ~90 | **Refactor** | `MissionEvent` (rename of `TuiEvent`), `MissionReport`, `Plan`, `GeneratedFile`, `AttemptResult`. Strip async. |
| `forge/src/router.rs` (new) | BCF-ABCC `router.rs` | 493 | **Refactor** | Async→sync. Keep: dual-scoring (rules + AI + blending disagreement handler), `Tier` enum, Campbell 1-10 ladder. Rule scorer is pure — unit-testable without a provider. |
| `forge/src/planner.rs` (new) | BCF-ABCC `mission.rs` §architect + `cto.rs` prompt templates | ~300 | **Refactor** | Single LLM call → `Plan { adr: String, manifest: Vec<String>, test_plan: String }`. |
| `forge/src/coder.rs` (new) | BCF-ABCC `mission.rs` §coder + `codegen.rs` | 450 + ~250 | **Refactor** | Streams tokens via callback; runs through `codegen::extract_files` → `Vec<GeneratedFile>`. Production-code output. Complexity-scaled model upgrade path (C7+ swap to premium if available). |
| `forge/src/test_coder.rs` (new) | BCF-ABCC `mission.rs` §tester + `codegen.rs` | same sources | **Refactor** | Same extraction as `coder`, test-suite output. Validates the code produced in the prior stage against the `Plan::test_plan` from Planner. |
| `forge/src/verifier_stage.rs` (new) | BCF-ABCC `verifier.rs` | 862 | **Refactor + prune** | Python-specific parts (venv/pip/pytest/ruff) DROP. Keep: secret-pattern scanner, TODO-counter, file-tree walker. Add Rust equivalent: invoke `shell_run` tool with `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` + `cargo test`. |
| `forge/src/surgical_coder.rs` (new) | BCF-ABCC `mission.rs` §fix_coder | ~400 | **Refactor** | Import-chain tracing (NameError / AttributeError / TypeError / ImportError), per-file LLM call with specific findings, reasoning-leak detector (rejects output containing LLM thinking text). Per BCF learning #12 never full-regen. |
| `forge/src/gate.rs` (new) | BCF-ABCC `mission.rs` §security + §critique + §cto + §quality_gate | ~400 | **Refactor** | Single LLM call with JSON schema returning security verdict + 5-score critique (DEV/ARCH/TEST/SEC/DOCS) + CTO verdict. Final score = `critique_avg * 0.4 + verifier_score * 0.6`. Threshold defaults to `MissionConfig::gate_threshold` (default 8.0, bench-tuneable). BCF's 9.2/8.5/8.0 Campbell-tier ladder available as `--gate-preset aspirational`. |
| `forge/src/mission.rs` (new) | BCF-ABCC `mission.rs` (orchestration core) | ~800 | **Refactor** | `MissionRunner::run(prompt) -> MissionReport` + event channel. Per-round attempt loop, smart-stopping, best-round restore. |
| `forge/src/lib.rs` (rewrite) | — | — | **New** | `pub fn run_mission(cfg, prompt, event_tx) -> Result<MissionReport>` as single public entry. |
| `claudettes-forge/src/forge_cmd.rs` (new) | — | — | **New** | CLI dispatch: parse args → assemble `MissionConfig` from `models.toml` + flags → open session journal → call `forge::run_mission` → stream events to stderr → exit 0/1 per gate. |
| `claudettes-forge/src/learnings.rs` (new) | — | — | **New** | `learnings promote <session-id>` subcommand. Walks sessions dir, prints summary, prompts which round/finding to graduate, appends to `CLAUDETTES_FORGE.md`. |
| `claudettes-forge/src/git_workspace.rs` (new) | claudette `src/tools/git.rs` (partial) | 533 (small subset) | **New** | `--git-workspace` flag handling. Create `mission/<id>` branch; commits use synthetic author env override. Refuse to run with dirty tree unless `--force-dirty`. |
| `claudettes-forge/src/cli.rs` + `src/main.rs` (amend) | — | — | **Amend** | Wire `forge` from stub into real dispatch. Add `--git-workspace`, `--force-dirty`, `--model-preset`. Add `learnings promote` subcommand. |
| `core/src/models_toml.rs` (amend) | — | — | **Amend** | Optional `context_size` + `max_predict` per role + optional `pipeline.gate_threshold` + optional `pipeline.gate_preset`. Backwards-compat with Sprint 2 files (defaults logged when fields missing). |

## Concrete checklist

One step = one commit. Green `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all -- --check` between every step.

**Pre-flight (steps 1-2):**

1. **[ ]** Enable `#![warn(clippy::pedantic)]` + scoped `#![allow(clippy::module_name_repetitions)]` in: `tui/src/lib.rs`, `forge/src/lib.rs`, `verifier/src/lib.rs`, `integrations/src/lib.rs`, `bench/src/lib.rs`, `claudettes-forge/src/main.rs`. Fix the 96 resulting warnings. Commit: `chore(workspace): enforce clippy pedantic workspace-wide`.
2. **[ ]** Add `pub fn resolve_ollama_host(flag: Option<&str>) -> String` in `core::providers`. Replace the 3 current sites with calls to the helper; delete duplicates. Unit tests cover flag → env → fallback + scheme normalisation + trailing-slash trim. Commit: `feat(core): centralise Ollama host resolution`.

**Core tools (steps 3-8):**

3. **[ ]** `core/src/tools/mod.rs` + `core/src/tools/file_read.rs`. Tool registration helper. `ReadOnly` tier. 6-8 unit tests (happy path, path escape, size cap, missing file, utf-8 boundary, offset/limit).
4. **[ ]** `core/src/tools/file_write.rs`. Write + Edit + atomic-rename path. `WorkspaceWrite` tier. Tests cover workspace boundary rejection, concurrent-write safety, non-existent-parent creation.
5. **[ ]** `core/src/tools/shell_run.rs`. `std::process::Command` + thread-based timeout + env-strip. `DangerFullAccess` tier. Cross-platform tests use `cmd /C echo` on Windows, `/bin/sh -c echo` on Unix. Timeout test uses `sleep 5` with 100ms cap.
6. **[ ]** `core/src/tools/grep.rs`. Ripgrep shell-out with graceful fallback to hand-rolled walker. `ReadOnly` tier. Tests assert match format + context lines + glob filter.
7. **[ ]** `core/src/tools/web_fetch.rs`. Blocking GET + `html2text` + redirect cap + size cap + `<web-fetch>` provenance envelope. `Network` tier. Live test gated behind `live-web` feature; unit tests use a local wiremock-server fixture OR a stream-from-string reader if wiremock feels heavy.
8. **[ ]** `core/src/tools/gh.rs`. `gh_pr_create` + `gh_issue_create` via `gh` CLI shell-out. `Network` + `Execute` tier. Tests assert arg construction; live test requires `gh auth status` green and is gated behind `live-gh`.

**Context manager (step 9):**

9. **[ ]** `core/src/context.rs` — verbatim port of BCF-ABCC `context.rs`. All 3 tests pass. Rename `ContextMessage` / `ContextManager` unchanged.

**Forge pipeline — stages in isolation (steps 10-17):**

10. **[ ]** `forge/Cargo.toml` + `forge/src/types.rs`. Deps: `claudettes-forge-core`, `serde`, `serde_json`, `thiserror`. Types: `MissionEvent`, `MissionReport`, `Plan`, `GeneratedFile`, `AttemptResult`, `Stage` enum.
11. **[ ]** `forge/src/router.rs` — rules + AI blending. Rules-only pure-function test (no LLM). `assess_complexity_dual` with `MockProvider`.
12. **[ ]** `forge/src/planner.rs` — single LLM call returning `Plan`. Mock-provider test with canned JSON response.
13. **[ ]** `forge/src/coder.rs` — LLM call + multi-file extraction via `codegen` helper. Complexity-scaled model upgrade path. Mock-provider test with fixture multi-file output.
14. **[ ]** `forge/src/test_coder.rs` — runs after Coder; generates test suite validating the produced code against `Plan::test_plan`. Same extraction shape as `coder`. Mock-provider test.
15. **[ ]** `forge/src/verifier_stage.rs` — uses `shell_run` tool to invoke `cargo fmt/clippy/test`. Parses output. Secret/TODO scanners ported from BCF-ABCC. Unit tests on the parsers.
16. **[ ]** `forge/src/surgical_coder.rs` — import-chain tracing, per-file fix LLM call, reasoning-leak detector, smart-stopping heuristic. Unit tests on the tracer + leak detector.
17. **[ ]** `forge/src/gate.rs` — single structured-JSON call per D2=a. Complexity-scaled threshold. Unit tests on JSON parse + score math + gate decision.

**Orchestration (steps 18-19):**

18. **[ ]** `forge/src/mission.rs` — `MissionRunner::run` wiring: routes stages, handles surgical loop + best-round restore + smart-stopping. Integration test at `forge/tests/smoke.rs`: mock provider drives 1 mission through all 7 stages, gate passes, report snapshot matches.
19. **[ ]** `forge/src/lib.rs` — `pub fn run_mission(...) -> Result<MissionReport>` + re-exports.

**CLI wire-through (steps 20-24):**

20. **[ ]** `core/src/sessions.rs` — session autosave primitive. File format spec in module docs.
21. **[ ]** `core/src/models_toml.rs` amendment per D4. Backwards-compat test loads a Sprint-2 models.toml and logs defaults for missing fields.
22. **[ ]** `claudettes-forge/src/forge_cmd.rs` + `cli.rs` amend + `main.rs` amend — real `forge` dispatch. Session file opened per mission. Manual smoke: `claudettes-forge forge "write a rust cli that counts words" --model-preset fast` against live Ollama.
23. **[ ]** `claudettes-forge/src/learnings.rs` — `learnings promote <session-id>` subcommand. Interactive prompt via `TtyPrompter`. Appends to `CLAUDETTES_FORGE.md`.
24. **[ ]** `claudettes-forge/src/git_workspace.rs` + flags — `--git-workspace`, `--force-dirty`. Branch creation + synthetic-author commit. Tests use a tempdir git init.

**Sprint-close (step 25):**

25. **[ ]** Fresh-clone gate: `git stash && git clone --depth 1 . /tmp/freshclone && cd /tmp/freshclone && cargo test --workspace && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings`. Catches unstaged-dependence + CRLF regressions. Tag `v0.2.0-rc1`.

## Dependencies to add

### `crates/core/Cargo.toml`

```toml
html2text = "0.6"   # web_fetch HTML → plain text; lightweight, no browser engine
```

No new deps for `shell_run` (`std::process`), `grep` (ripgrep shell-out), `gh.rs` (`gh` CLI shell-out), `sessions.rs` (std-only).

### `crates/forge/Cargo.toml`

```toml
[dependencies]
claudettes-forge-core = { path = "../core" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
# No tokio — see D3.
```

### `crates/claudettes-forge/Cargo.toml`

```toml
claudettes-forge-forge = { path = "../forge" }
```

### Deliberately NOT added

- `tokio` — sync stays the discipline (D3). Tokio retrofit is an independent AD.
- `anyhow` — BCF uses it; we stay on `thiserror` + typed error enums per Sprint 1.
- `chrono` — `std::time::SystemTime` + `UNIX_EPOCH` arithmetic suffices (same choice as OAuth port).
- `rayon` — pipeline is sequential.
- `syntect` — still out of scope.
- `wiremock` (possibly) — if lightweight-enough for `web_fetch` tests, in; otherwise use a hand-rolled `impl Read` fixture.

## Exit criteria

- [ ] `cargo build --workspace` clean.
- [ ] `cargo test --workspace` green. Target: 135 + ~60 new = **~195 total**.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` green with pedantic workspace-wide.
- [ ] `cargo fmt --all --check` clean.
- [ ] `claudettes-forge forge "write a rust cli that counts words in a file" --model-preset fast` runs end-to-end against live Ollama. Output includes stage-progression log + generated crate + cargo-green verifier + gate verdict with scores.
- [ ] `claudettes-forge forge "<an intentionally-hard prompt>"` produces a best-round-restored output, flags `score < threshold`, exits 1.
- [ ] `claudettes-forge learnings promote <session-id>` reads a session journal, prompts, writes to `CLAUDETTES_FORGE.md`.
- [ ] `claudettes-forge forge --git-workspace "..."` creates branch `mission/<id>` and commits with `mrdushidush-forge-mission` as author.
- [ ] 6 concrete tools callable via the registry, permission-gated via `DefaultPolicy`.
- [ ] `context::ContextManager` auto-compacts at 95% capacity (existing BCF-ABCC invariant).
- [ ] Fresh-clone sprint-close audit passes.

## Known risks

1. **Scope is large.** 25 steps vs 19/13 for Sprints 2/1. If tools alone exceed 2 weeks, pause and ship `v0.1.1` as a "tools-only" intermediate, then resume with pipeline as `v0.2.0`. Decision point: after step 9.
2. **Cargo-based Verifier is Rust-specific.** BCF targets Python. Missions generating Go/Python/JS can't be verified in v0.2. Documented in CHANGELOG; language-agnostic runner is v0.3.
3. **Reasoning-leak detection is model-dependent.** BCF-ABCC learnings #22/#25 flag nemotron meta-reasoning and MoE models returning empty. Port the detector with test fixtures for known-bad outputs.
4. **Gate threshold tuning lives outside Sprint 3 scope.** Default 8.0 ships; user plans to run local-only and local+cloud benchmarks after Sprint 3 to tune it. Do not hardcode 9.2 anywhere the threshold is consulted — read from `MissionConfig::gate_threshold`. `--gate-preset aspirational` gives access to BCF's 9.2/8.5/8.0 ladder for cloud-model runs. If benchmark data post-Sprint-3 shows the default should move, adjust in a follow-up commit without reopening Sprint 3.
5. **Session autosave grows unbounded.** 100 missions × many turns = thousands of files in one dir. Rotation is v0.3; Sprint 3 ships one file per mission with no rotation.
6. **`gh` CLI auth assumed.** Tool invocation fails confusingly if `gh auth status` isn't green. `doctor` should preflight check; tool should emit clear error otherwise.
7. **Dirty-tree git workspace.** If working tree has staged/unstaged changes, branch-create silently includes them or fails. Policy: refuse to run without `--force-dirty`.
8. **BCF-ABCC async → sync translation volume.** Every `async fn` / `.await` / `tokio::sync::mpsc` gets hand-translated. Suggested pattern: mechanical pass first (compile-green with potential deadlocks), then a semantic pass that fixes any concurrent-read/-write issues revealed.
9. **ContextManager + forge pipeline state boundaries.** Core's `ContextManager` is cross-turn memory; forge's per-mission state is a `Vec<Message>`. Don't double-manage. Recommended: pipeline keeps its own `Vec<Message>`, passes to provider; `ContextManager` consumed only by `learnings` flow.
10. **`models.toml` schema migration.** Sprint 2 files don't have the new `context_size`/`max_predict` fields. Migration: read old-format, add defaults, log a warning. CHANGELOG entry required.

## After this sprint

- **Sprint 4 — standalone verifier.** `crates/verifier` body + `claudette-verify <path>` binary. Lint + security scan + LLM review of arbitrary codebases.
- **Sprint 5 — security hardening.** Provenance-wrapping on all inbound text. Platform sandboxing (`sandbox-exec`, `bwrap`, Windows Job Objects) around `DangerFullAccess`.
- **Sprint 6 — integrations.** Telegram / voice / MCP, each feature-flagged.
- **Sprint 7 — bench.** `claudettes-forge bench`, A/B harness, SWE-bench runner (dev-only).
- **v0.3 targets** after Sprint 7: Anthropic provider, Grok provider, LanceDB rich memory, antipattern auto-detection, editor mode, interactive CTO agent, swarm mode, language-agnostic verifier, TUI forge-tab.

## Next-session kickoff checklist

**If you are Sonnet picking up Sprint 3 cold, do this IN ORDER:**

1. Read this file end-to-end.
2. Read `docs/decisions.md` AD-4 (pipeline) + AD-6 (toolchain) + AD-7 (Sprint 3 architectural decisions: Gate collapse, sync discipline, models.toml schema, bench-driven gate threshold).
3. Skim `D:/dev/abcc_projects/abcc_projects/battle-command-forge/CLAUDE.md` §Key Learnings (29 numbered benchmark invariants).
4. Verify current state:
   ```bash
   cd D:/dev/claudettes-forge
   git log --oneline -1                                            # expect 28e0e74
   cargo test --workspace                                          # expect 135 green
   cargo clippy --workspace --all-targets -- -D warnings           # expect clean
   git status                                                      # expect "working tree clean"
   ```
5. Start at step 1 (pedantic workspace-wide, 4-6h budget).

## Hard constraints for the implementer

- **Git identity per commit:** `git -c user.name="mrdushidush" -c user.email="mrdushidush@gmail.com" commit …`. Never `git config --global`.
- **Conventional Commits.** Sprint 3 scopes: `feat(core)`, `feat(forge)`, `feat(tools)`, `feat(cli)`, `test(forge)`, `docs(sprint-3)`, `chore(workspace)`.
- **One step = one commit.** Green `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all --check` between every step.
- **Fresh-clone sprint-close audit required before tagging** (lesson from pre-Sprint-3 audit).
- **MSRV = 1.85** (AD-6). `is_multiple_of` needs 1.87 — stays disallowed. `% == 0` + `#[allow(clippy::manual_is_multiple_of)]` is the sanctioned pattern.
- **Clippy pedantic workspace-wide** from step 1 onwards. Any new `#[allow]` must be scoped + justified with a comment.
- **No `unwrap()` in library code.** OK in tests; OK in `main.rs` where panic-to-user-error is the natural boundary.
- **Do not touch Sprint 1/2 code** unless forge needs force a revision. If it happens, the touch commits separately with justification (see `d89dfd9` fmt-drift for the pattern).
- **Sync only (AD-7).** No `tokio`, `async fn`, `.await` in any Sprint 3 delta. Thread + `std::sync::mpsc` is the concurrency primitive (matching Sprint 2 worker).
- **Gate threshold is config-driven (AD-7).** Default 8.0. No hardcoded 9.2. Read from `MissionConfig::gate_threshold`.
- **Scope-creep list is binding.** If you find yourself porting `swarm.rs`, `sandbox.rs`, `editor.rs`, `swebench*.rs`, or adding a cloud provider — stop. Those are other sprints.
- **BCF-ABCC learnings are invariants.** 29 numbered rules in `D:/dev/abcc_projects/abcc_projects/battle-command-forge/CLAUDE.md` §Key Learnings. If one is contradicted by the port, flag it in the commit message rather than silently re-introducing the problem (e.g. "full regen always degrades score" ⇒ no full-regen path in SurgicalCoder).
- **Blocker protocol.** If blocked, write `feat(...): WIP — blocked on <X>` and stop. Do not invent unscoped solutions.

---

**Written 2026-04-24 by Opus 4.7 (1M context). D1-D5 decisions signed off 2026-04-24 (see AD-7). Ready for Sonnet to pick up at step 1.**
