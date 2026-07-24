# Sprint 1 audit — implementation vs original design

**Snapshot date:** 2026-04-22, end of Sprint 1 core work.
**Head commit:** `c245776` (smoke test). **Tree state:** clean.
**Tests:** 103 lib + 2 integration = **105 green**. Clippy pedantic green. Workspace check clean.

Purpose: map every feature from the original design (captured in `memory/claudettes_forge_decisions.md`, `docs/architecture.md`, `docs/decisions.md`) to its current implementation status, so the next session can audit the state without re-reading six files.

Legend: **✅ done** / **🟡 partial** / **⏳ Sprint N** / **v0.2** / **dropped**

---

## 1. Architecture skeleton

| Feature (original design)                              | Status | Notes |
|--------------------------------------------------------|--------|-------|
| Multi-crate cargo workspace, monorepo, single binary   | ✅     | 6 library crates + 1 binary. AD-1 captures rationale. |
| Crate: `core`                                          | ✅     | 10 modules filled, 103 lib tests. |
| Crate: `tui`                                           | ⏳ S2  | `src/lib.rs` is a stub. |
| Crate: `forge` (7-stage pipeline)                      | ⏳ S3  | Stub. |
| Crate: `verifier` (standalone + lib)                   | ⏳ S4  | Lib + bin scaffolded, no logic. |
| Crate: `integrations` (telegram, voice, mcp)           | ⏳ S6  | Stub, feature flags declared in design but not in `Cargo.toml` yet. |
| Crate: `bench`                                         | ⏳ S7  | Stub. |
| Crate: `claudettes-forge` (binary, dispatches modes)   | ⏳ S2  | `src/main.rs` is a stub; no CLI parser yet. |
| Cross-platform Mac + Linux + Windows                   | 🟡    | Develops on Windows. Mac/Linux not tested yet; no known blockers. `open_browser` has all three branches. |
| `cargo install` distribution                           | ⏳     | No publish yet. |
| Rust stable, Conventional Commits, scope-guarded sprints | ✅  | Five Conventional-Commits commits on `main`; sprint doc exists. |
| `docs/comparison.md` vs competitors                    | 🟡    | Stub exists; content empty. |

---

## 2. The `core` crate — module-by-module

All paths relative to `crates/core/src/`.

| Module            | Status | What's in                                                    | What's not in                                                            |
|-------------------|--------|--------------------------------------------------------------|--------------------------------------------------------------------------|
| `types.rs`        | ✅     | `Mission`, `MissionId`, `Subtask`, `Role` (8 variants), `Complexity` (C1-C10), `ProviderKind` (Ollama + AnthropicClaude), `ModelMap` + resolve, `ToolCall`, `ToolResult` | — |
| `tool.rs`         | ✅     | `Tool` trait, `ToolCtx`, `ToolGroup` (15 variants), `ToolRegistry` (`register`/`enable_group`/`current_schemas`/`dispatch`), `SharedRegistry` alias | **No actual tool implementations.** Registry is empty at startup until tools are registered. Sprint 3+ will add file ops / shell / web / etc. |
| `memory.rs`       | ✅     | `try_load_memory_at`, `cap_memory`, `default_memory_path`, `MAX_MEMORY_CHARS`, `MEMORY_ENV_VAR` — read-only loader | **Session autosave not built.** The design memo says "cross-session session autosave" — module is currently read-only. |
| `secrets.rs`      | ✅     | `read_secret`, `secret_file_path`, `secrets_dir` with `CLAUDETTES_FORGE_` / `{NAME}_TOKEN` / file-fallback lookup order | Chat-ID persistence intentionally deferred to `integrations/telegram` (not core). |
| `permissions.rs`  | ✅     | `PermissionTier` (5), `Operation` (5), `AuthOutcome` (3), `PermissionPolicy` trait, `PermissionPrompter` trait, `DefaultPolicy`, `TtyPrompter` (stdin+60s timeout via `IsTerminal`), `DenyingPrompter` | **Platform sandboxing not wired.** Design calls for `sandbox-exec` / `bwrap` / Windows-TBD wrappers around `DangerFullAccess` — not implemented. |
| `personas.rs`     | ✅     | `Persona`, `PersonaMap`, `PersonaStatus`, `load_personas`, `parse_persona_file`, TOML frontmatter parser, `## Example moments` splitter, `default_bundled_dir`, `default_user_dir` | Hot-reload explicitly rejected per design. |
| `oauth.rs`        | ✅     | Full loopback OAuth 2.0 for Google. `AuthContext` (Calendar + GmailRead) with `min_tier()` mapping to 5-tier permissions. `access_token` / `run_auth_flow` / `revoke`. Env vars `CLAUDETTES_FORGE_GOOGLE_*` (+ `GOOGLE_*` fallback). Tokens under `~/.claudettes-forge/secrets/`. | **GmailWrite** variant deliberately deferred per design (claudette's "phase 5"). Loopback waits indefinitely if user cancels the browser — documented known-limitation; `accept_callback` could grow a watchdog thread later. |
| `providers/mod.rs`| ✅     | `Provider` trait, `ChatRequest`, `Message` + `MessageRole`, `ChatResponse`, `Usage`, `TextCallback`, `ProviderError` (thiserror) | — |
| `providers/ollama.rs` | ✅ | `OllamaProvider` with blocking NDJSON streaming. `OLLAMA_HOST` env. Per-line text-delta callback. Tool-call normalization with synthesized id fallback. Saturating u64→u32 token-count cast. | **Context-budgeting is not ported.** Claudette's `history_budget_chars` / `truncate_to_budget` / `build_messages` are skipped here by design — v0.1 relies on the caller to produce a message list that fits. Sprint 3 (forge mode) will want to port those. |

### `core` stuff that does not exist yet

Not a shortcoming of Sprint 1 — these are all flagged in the design as future-sprint work, but worth surfacing in one place:

- **Concrete tool implementations.** The `Tool` trait exists; no concrete tool does. A `file_read`, `file_write`, `shell_run` starter set lives in claudette (`src/tools.rs` + `src/tool_groups.rs`, ~1866 LOC) and will need porting — probably into a new `crates/core/src/tools/` subdirectory or per-crate split. Sprint 3 candidate.
- **`enable_tools` meta-tool.** Design calls for a synthetic tool the model can call mid-session to activate a `ToolGroup`. Not in the registry yet.
- **learnings.md `--promote` flow.** Explicit graduation from session memory to durable learnings. Not built.
- **CLI parser / subcommand dispatch.** `crates/claudettes-forge/src/main.rs` is a stub — no `clap`, no `forge <mission>` / `verify <path>` / `bench <sub>` dispatch.

---

## 3. Personas

| Persona       | File                     | Role         | Voice                 | Status   | Source |
|---------------|--------------------------|--------------|------------------------|----------|--------|
| CodeX-7       | `personas/codex7.md`     | `coder`      | clipped-tactical       | ✅ Loaded | Distilled from `D:/dev/tacticode/packages/agents/src/personas/codex7.py`. 7 example moments. |
| Sentinel-9    | `personas/sentinel9.md`  | `verifier`   | auditor-formal         | ✅ Loaded | Distilled from tacticode `sentinel9.py`. 6 example moments. |
| CTO           | `personas/cto.md`        | `cto`        | strategic-authority    | ✅ Loaded | Distilled from tacticode `cto.py` (decompose / review / clarify system prompts). 6 example moments. |
| Eva           | `personas/eva.md`        | `assistant`  | warm-efficient         | ✅ Loaded | Written fresh. 7 example moments. |

**Safety net in place:** three bundled-persona smoke tests in `personas.rs` parse the real directory via `CARGO_MANIFEST_DIR/../..` and assert all four load, carry expected roles, advance past `Placeholder`, and have ≥3 examples each. Any future edit that breaks a persona fails `cargo test`.

---

## 4. Providers

| Provider                  | Status | Notes |
|---------------------------|--------|-------|
| Ollama (`/api/chat` NDJSON) | ✅   | Blocking stream. Live test gated behind `live-ollama` feature. |
| Anthropic Claude          | v0.2   | Trait supports multi-provider; concrete impl deferred per AD-3. |
| `models.toml` resolution  | ⏳     | `ModelMap` exists + `resolve()`; TOML loader not yet written. Sprint 2 when CLI wiring lands. |
| `doctor` pre-flight check | ⏳     | Design calls for `claudettes-forge doctor` to probe Ollama availability. Not built. |

---

## 5. Memory + learnings

| Feature                                              | Status | Notes |
|------------------------------------------------------|--------|-------|
| File-backed markdown default                         | 🟡     | **Read-only.** `try_load_memory_at` + `cap_memory` exist. Session autosave is not built — flagged in design, deferred. |
| `CLAUDETTES_FORGE.MD` at `~/.claudettes-forge/`      | ✅     | Loader path + env-var override wired. |
| 800-char cap (claudette's qwen3.5:9b finding)        | ✅     | `MAX_MEMORY_CHARS` const. |
| Cross-session session autosave                       | ⏳     | Not built. See §2 note. |
| Explicit `learnings.md` `--promote` graduation       | ⏳     | Not built. |
| LanceDB rich memory (`--features rich-memory`)       | v0.2   | Feature flag declared in `Cargo.toml`; no LanceDB code. |
| Self-evolving few-shots                              | dropped | User explicit decision: "never load-bearing." |
| Antipattern auto-detection                           | v0.2   | Deferred per design. |

---

## 6. Permissions + security

| Feature                                              | Status | Notes |
|------------------------------------------------------|--------|-------|
| 5-tier (`ReadOnly` / `WorkspaceWrite` / `DangerFullAccess` / `Prompt` / `Allow`) | ✅ | AD-5. |
| `PermissionPolicy` trait, `DefaultPolicy`            | ✅     | Path-boundary + `..` traversal reject + tier-gate. |
| `PermissionPrompter` trait, `TtyPrompter` + `DenyingPrompter` | ✅ | 60s stdin timeout via stdlib `IsTerminal`. |
| `Prompt`/`Allow` bypass of dispatch tier gate        | ✅     | Semantic decision — session-mode tiers, not ladder entries. Candidate for AD-6 if documenting. |
| Universal provenance-wrapping (email → web / calendar / gmail bodies) | ⏳ | Design calls for generalising claudette's `<email>` defanger. Not ported. |
| Platform sandboxing (`sandbox-exec` / `bwrap` / Windows TBD) | ⏳ | Not wired. Would wrap `DangerFullAccess` as defence-in-depth. |
| 3-layer shell security                               | dropped? | Design says "deferred"; treat as ⏳ until user re-confirms. |

---

## 7. TUI

| Feature                              | Status | Notes |
|--------------------------------------|--------|-------|
| ratatui-based TUI (always linked)    | ⏳ S2  | Stub crate. |
| Paste-to-tempfile (`paste.rs` lift)  | ⏳ S2  | — |
| Typewriter code effect               | ⏳ S2  | — |
| Just-Space-Invaders easter egg (redesigned) | ⏳ S2 | — |
| Model-registry header strip          | deferred | — |

---

## 8. Voice

| Feature                              | Status | Notes |
|--------------------------------------|--------|-------|
| Tier-1 runtime feature               | ⏳     | Lives in `integrations` crate behind `voice` flag. Not wired. |
| 192 `.wav` banks from godfather      | ⏳     | Asset-copy step; not done. |
| Platform TTS                         | ⏳     | — |

---

## 9. Integrations

| Feature                              | Status | Notes |
|--------------------------------------|--------|-------|
| Telegram (feature-flagged)           | ⏳ S6  | Stub crate. |
| Voice (feature-flagged)              | ⏳ S6  | See §8. |
| MCP client (feature-flagged)         | ⏳ S6  | — |
| MCP server                           | deferred | Design says v0.2+. |
| OAuth in `core` (Calendar + Gmail-read) | ✅  | See §2 `oauth.rs`. |
| Plugin/hook system                   | dropped | User explicit decision. |

---

## 10. Bench

| Feature                              | Status | Notes |
|--------------------------------------|--------|-------|
| `claudettes-forge bench …` subcommand | ⏳ S7 | Needs CLI + bench crate body. |
| User-defined fixtures                 | ⏳ S7 | — |
| A/B methodology (WITH/WITHOUT-QA, WITH/WITHOUT-URL, determinism reruns) | ⏳ S7 | — |
| Rich-JSON output                      | ⏳ S7 | — |
| SWE-bench runner (dev-only)           | ⏳ S7 | — |

---

## 11. Observability

| Feature                              | Status | Notes |
|--------------------------------------|--------|-------|
| `tracing` baseline                   | ⏳     | Not added yet. Small follow-up; no design decision pending. |
| Prometheus                            | deferred | Flagged in design. |
| OpenTelemetry                         | deferred | — |

---

## 12. Dev process + docs

| Feature                              | Status | Notes |
|--------------------------------------|--------|-------|
| Conventional Commits                 | ✅     | 5 commits on `main`, all compliant. |
| AD-numbered decisions                | 🟡    | AD-1 → AD-5 exist. AD-6 candidates: (a) `Prompt`/`Allow` bypass of dispatch gate, (b) stdlib `IsTerminal` instead of `atty`, (c) TOML frontmatter over YAML, (d) OAuth dep-minimalism (no chrono/url/base64/webbrowser), (e) memory read-only at v0.1. Writing AD-6 is nice-to-have; none is load-bearing. |
| Scope-guarded sprint plans           | ✅     | `docs/sprints/sprint_01_core.md`. Sprint 2 plan not yet written. |
| `docs/comparison.md`                 | 🟡    | Stub. Content empty. |
| CI workflow (GitHub Actions)         | ⏳     | No `.github/workflows/`. Waits on repo getting a GitHub remote. |
| Git identity configured              | ⚠️    | Repo has no `user.email` / `user.name` set locally or globally. Past two sessions used `git -c user.name=... -c user.email=...` to match `mrdushidush <mrdushidush@gmail.com>` from the first commit. Next session should follow the same pattern (per the "never modify git config" rule). |

---

## 13. Deferrals map — where each deferred feature lives

### Sprint 2 (TUI + CLI wiring)

- `crates/tui` body (ratatui loop, paste-to-tempfile, typewriter, space-invaders easter egg)
- `crates/claudettes-forge/src/main.rs` — clap parser, subcommand dispatch, `doctor` command
- `models.toml` loader in `core`
- CI workflow — lands whenever the repo gets a GitHub remote
- Concrete tool implementations (`file_read`, `file_write`, `shell_run`, etc.) — *could* land in Sprint 2 with CLI if the assistant loop wants real tools; otherwise Sprint 3.

### Sprint 3 (forge pipeline)

- `crates/forge` bodies — 7 pipeline stages
- Double-Context Phase-0 gambit
- Branch-per-mission + synthetic-author commit wiring
- Context-budgeting port from claudette (`truncate_to_budget` etc.)
- Session autosave + `learnings.md --promote` flow

### Sprint 4 (verifier)

- `crates/verifier` body — static analysis + LLM review
- `claudette-verify` standalone binary logic

### Sprint 5 (security hardening)

- Universal provenance-wrapping
- Platform sandboxing (`sandbox-exec` / `bwrap` / Windows TBD)
- 3-layer shell security (if un-deferred)

### Sprint 6 (integrations)

- Telegram (feature-flagged)
- Voice (feature-flagged, 192 `.wav` banks, platform TTS)
- MCP client (feature-flagged)

### Sprint 7 (bench)

- `claudettes-forge bench` subcommand
- A/B harness, determinism reruns
- SWE-bench runner (dev-only)

### v0.2 (post-v0.1 ship)

- Anthropic Claude provider
- LanceDB / rich memory (`--features rich-memory`)
- Antipattern auto-detection
- MCP server

### Dropped permanently (user decision)

- Self-evolving few-shots
- Plugin / hook system
- stealthsambaV2's 10-stage pipeline (7-stage replaces it)

---

## 14. Dependency inventory

### Shipped with Sprint 1 core

```toml
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"
reqwest = { version = "0.12", features = ["blocking", "json"] }
thiserror = "1.0"
# dev-dep:
serde_json = "1.0"  # integration-test scope
```

### Expected to arrive next

- `clap` (Sprint 2 CLI parser)
- `ratatui` + `crossterm` (Sprint 2 TUI)
- `tracing` + `tracing-subscriber` (observability baseline, minor)
- Sprint 3+ as each feature lands

### Deliberately *not* pulled in

- `chrono` — std `SystemTime` suffices.
- `url` — hand-rolled percent codec in `oauth.rs`.
- `base64` — no PKCE in OAuth flow.
- `webbrowser` — native `cmd /C start` / `open` / `xdg-open` dispatch.
- `atty` — stdlib `IsTerminal` (stable since 1.70; workspace MSRV 1.75).

---

## 15. Next-session starting points

A fresh session can orient itself from:

1. This file (`docs/sprint_01_audit.md`) — the feature-map.
2. `docs/sprints/sprint_01_core.md` — the completed sprint plan.
3. `memory/claudettes_forge_decisions.md` — durable design record.
4. `git log --oneline` — the five landed commits.

**Likely first questions for the user to confirm before Sprint 2:**

1. Take the ship-claudette pause (step 9) before Sprint 2, or skip it?
2. Write AD-6 capturing the sprint's non-load-bearing decisions, or skip?
3. Add CI workflow now (the repo has no remote yet), or wait?
4. Sprint 2 target: TUI + CLI wiring + concrete tools *together*, or TUI first and tools later?

**Checks the next session should run before touching code:**

```bash
cargo test -p claudettes-forge-core   # expect 103 lib + 2 integration = 105 green
cargo clippy -p claudettes-forge-core --all-targets -- -D warnings  # expect clean
cargo check --workspace                # expect clean
git status                              # expect "working tree clean"
git log --oneline -6                    # expect 519031f → c245776
```
