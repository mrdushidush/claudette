# Sprint 1 — `core` crate

**Target version:** Ships as part of v0.0.2+. Sprint ends when exit criteria green.
**Duration estimate:** 1-2 weeks of focused work.
**Cadence:** Per user, pause partway to ship existing claudette as-is; resume after.
**Outcome:** `core` crate compiles clean, passes unit tests, and a hand-written smoke test drives a full mission through the public API without touching private internals.

## Current status (2026-04-22, updated after step 13 — Sprint 1 core work done)

- API surface drafted in `crates/core/src/`: `types.rs`, `tool.rs`, `memory.rs`, `permissions.rs`, `personas.rs`, `oauth.rs`, `secrets.rs`, `providers/{mod.rs, ollama.rs}`.
- Steps 2–8 complete: bodies written for `memory.rs`, `secrets.rs`, `types.rs::ModelMap`, `permissions.rs::DefaultPolicy` + `TtyPrompter`, `tool.rs::ToolRegistry`, `personas.rs`.
- Step 10 complete: `OllamaProvider` NDJSON streaming `chat()` ported from claudette.
- Step 11 complete: Google OAuth 2.0 loopback flow ported. `AuthContext::min_tier()` wired into 5-tier permissions.
- Step 12 complete: four bundled personas fleshed out — CodeX-7, Sentinel-9, CTO, Eva. All four parse cleanly, status `Loaded`, ≥3 `## Example moments` each.
- Step 13 complete: integration smoke test at `crates/core/tests/smoke.rs` composes personas + mission + policy + registry + two mock tools (ReadOnly + WorkspaceWrite) + mock Provider. Drives one full turn end-to-end: canned chat → tool call dispatch → real file write → file read-back through a second tool. A second `policy_denies_write_outside_workspace` test guards the workspace boundary.
- 103 lib unit tests + 2 integration smoke tests = **105 tests green**.
- Clippy pedantic green (`cargo clippy -p claudettes-forge-core --all-targets -- -D warnings`).
- Workspace check clean. Deps added this sprint: `serde`, `serde_json`, `toml`, `reqwest` (blocking+json), `thiserror`. Dev-dep: `serde_json` (for integration-test scope).

**Remaining for formal sprint close** (not part of step 13 proper):
1. CI workflow — `.github/workflows/ci.yml` running `cargo test`, `cargo clippy`, `cargo fmt --check` on push. Separate concern from the core bodies; can land in a follow-up.
2. AD-6 if desired — candidates that accumulated during the sprint: (a) `Prompt`/`Allow` tier bypassing the dispatch gate, (b) `IsTerminal` instead of `atty`, (c) TOML frontmatter instead of YAML for personas, (d) OAuth flow deliberately not pulling `chrono`/`url`/`base64`/`webbrowser`. Each is noteworthy but none is load-bearing architecturally.
3. Ship-claudette pause (step 9) — the user's original sequential cadence: cut claudette v0.2 before Sprint 2 starts.

## Per-module lift plan

**Verbatim lift** means: copy the module body, rename paths/env-var prefixes (`.claudette/` → `.claudettes-forge/`, `CLAUDETTE_` → `CLAUDETTES_FORGE_`, constant names like `MAX_MEMORY_CHARS`). Unit tests port with trivial rename.

**Refactor** means: design choices change between claudette and claudettes-forge; structural changes required.

**Rewrite** means: claudette has no equivalent or the shape doesn't port; new code from scratch.

| Module (new) | Source (claudette) | LOC (src) | Porting strategy | Notes |
|---|---|:---:|---|---|
| `memory.rs` | `src/memory.rs` | 152 | **Verbatim lift** | Already stubbed with doc comments + `MAX_MEMORY_CHARS` const. Sprint 1: fill `try_load_memory_at` body + port all 7 unit tests. |
| `secrets.rs` | `src/secrets.rs` | 258 | **Verbatim lift** | Chat-ID persistence may be deferred (Telegram-specific; can live in `integrations/telegram` instead). Sprint 1: fill `read_secret` body + `secret_file_path`; port 8 unit tests minus chat-ID ones. |
| `oauth.rs` | `src/google_auth.rs` | 714 | **Refactor** | Structural port — `AuthContext` now has `min_tier()` mapping into 5-tier permissions. Token-file paths move to `~/.claudettes-forge/secrets/`. Client-ID/secret env vars rename. Core loop (TcpListener loopback → browser open → code exchange → token persist → refresh) stays the same. Sprint 1: fill `access_token`, `run_auth_flow`, `revoke`. |
| `providers/ollama.rs` | `src/api.rs::OllamaApiClient` | ~1099 | **Refactor** | claudette had a concrete struct; we have a `Provider` trait with `OllamaProvider` as impl. Stream parser (line-buffered NDJSON, per-line delta callbacks, tool-call extraction on `done:true` chunk) ports verbatim. Context-budgeting is deferred to Sprint 3 when forge-mode lands. Sprint 1: blocking `reqwest` HTTP, NDJSON parse loop, tool-call normalization. |
| `providers/mod.rs` (`Provider` trait) | — | — | **New** | No trait in claudette (concrete struct). Sprint 1: trait definition (done in API draft), blocking `chat()` method, `ProviderError` enum. |
| `tool.rs` (`Tool` trait + `ToolRegistry`) | `src/tools.rs` + `src/tool_groups.rs` | 1199 + 667 | **Refactor** | claudette uses `fn dispatch_tool(name, input) -> Result<String, String>` — no trait. We have a `Tool` trait with `required_tier()` + permission-aware dispatch. Keep the `ToolGroup` enum + on-demand enabling pattern (verbatim-ish lift). Sprint 1: `current_schemas()`, `dispatch()` with policy gate. |
| `permissions.rs` | `src/agents.rs::build_agent_permission_policy` | partial | **Refactor + extend** | claudette has 3-tier; we have 5-tier (+ `Prompt`, `Allow`). `DefaultPolicy::authorize` is the big body — path-boundary checks for Read/Write, tier-elevation for Execute/Network, prompt delegation for `Prompt` tier. `TtyPrompter::ask` is ~30 LOC (stdin-with-timeout). Sprint 1: `DefaultPolicy::authorize`, `TtyPrompter::ask`, `DenyingPrompter::ask` (already trivial). |
| `personas.rs` | — | — | **New** (reference: `Archive/tacticode/packages/agents/src/personas/*.py`) | No claudette equivalent. Sprint 1 target: TOML-frontmatter parser for `personas/*.md`, merge with user overrides from `$PROJECT/.claudettes-forge/personas/`. Body content (CodeX-7 / Sentinel-9 backstories) is *ported content*, not *ported code* — each `.md` file in `personas/` gets filled with content translated from the Python persona files. |
| `types.rs` | scattered (`types.rs` doesn't exist in claudette) | — | **New** | Gathers what was inline in claudette (`Mission`, `Subtask`, `Role`, `Complexity`, `ProviderKind`, `ToolCall`, `ToolResult`). API already drafted. Sprint 1: fill `ModelMap::resolve` and add `Display`/`FromStr` derives where useful. |

## Sprint 1 concrete checklist

In order. Each step should produce a passing test before moving on.

1. **[x]** `crates/core/src/*` API surface (done 2026-04-22).
2. **[x]** `memory.rs` — filled `try_load_memory_at`, ported all 7 tests. ✅ 7 tests.
3. **[x]** `secrets.rs` — filled `read_secret`, ported env-var tests (chat-id deferred to integrations/telegram). ✅ 6 tests.
4. **[x]** `types.rs::ModelMap` — round-trip tests (body already existed from API draft). ✅ 4 tests.
5. **[x]** `permissions.rs::DefaultPolicy` — path-boundary + tier-gate + `..` traversal rejection. ✅ 11 tests (incl. prompter sanity).
6. **[x]** `permissions.rs::TtyPrompter` — `std::io::IsTerminal` + thread + `mpsc::recv_timeout`; falls back to deny in non-TTY envs. (No `atty` dep — MSRV 1.75 covers stdlib `IsTerminal`.) ✅ 2 tests (plus denying-prompter test under step 5).
7. **[x]** `tool.rs::ToolRegistry` — `current_schemas()` + `dispatch()` with tier gate; `Prompt`/`Allow` short-circuit the gate so per-op authorization is the tool's job. ✅ 9 tests.
8. **[x]** `personas.rs` — TOML-frontmatter parser, `load_personas`, `parse_persona_file`, body-splitter for `## Example moments`. Added `toml = "0.8"` dep. ✅ 10 tests.
9. **[ ]** **Pause here to ship claudette v0.2 as-is** (user's sequential cadence; see `memory/claudettes_forge_decisions.md`).
10. **[x]** `providers/ollama.rs` — NDJSON streaming `chat()` impl + 20 parser unit tests. Live integration test behind `#[cfg(feature = "live-ollama")]`. ✅ Added deps: `reqwest` (blocking+json), `thiserror`.
11. **[x]** `oauth.rs` — Google OAuth 2.0 loopback flow ported. `AuthContext` with `min_tier()` mapping to 5-tier permissions. Env renamed to `CLAUDETTES_FORGE_GOOGLE_*` (fallback `GOOGLE_*`). Token paths under `~/.claudettes-forge/secrets/`. 30 unit tests (AuthContext parse/tier, url codec, callback-query parse, env lookup, token serde roundtrip). Live browser-opening test behind `#[cfg(feature = "live-oauth")]`. ✅ Added dep: `serde` (derive).
12. **[x]** `personas/*.md` — all four fleshed out with TOML frontmatter (YAML-style would have failed the parser), correct role names, and ≥3 `### Example N` subheadings under `## Example moments`. 3 bundled-persona smoke tests verify the real files parse as `Loaded`. CodeX-7 + Sentinel-9 distilled from tacticode `D:/dev/tacticode/packages/agents/src/personas/*.py`; CTO distilled from tacticode's decompose / review system prompts; Eva written fresh.
13. **[x]** Smoke test at `crates/core/tests/smoke.rs`. Composes personas + mission + policy + registry + two mock tools (`MockReadFileTool` at `ReadOnly`, `MockWriteFileTool` at `WorkspaceWrite`) + `MockProvider`. One full turn end-to-end: canned chat → tool call dispatch → real file write at workspace-scoped path → file read-back through the second tool. A second test (`policy_denies_write_outside_workspace`) guards the workspace boundary. Memory autosave was deferred (the `memory` module is read-only at v0.1; memory-file round-trip via `try_load_memory_at` is tested instead, matching what actually ships). ← current landing zone.

## Dependencies to add during Sprint 1

Current state of `crates/core/Cargo.toml` dependencies after step 11:

```toml
serde = { version = "1.0", features = ["derive"] }                # OAuth GoogleTokens serde derive
serde_json = "1.0"                                                 # ToolCall::arguments, ToolRegistry schemas
toml = "0.8"                                                       # persona frontmatter
reqwest = { version = "0.12", features = ["blocking", "json"] }   # Ollama HTTP + OAuth loopback
thiserror = "1.0"                                                  # ProviderError derive
```

Step 11 deliberately **did not** pull in `chrono`, `url`, `base64`, `webbrowser`:
- `chrono` — `std::time::SystemTime` gets unix seconds for token expiry math; no chrono needed.
- `url` — hand-rolled `url_encode`/`url_decode` (percent + hex nibble) stays in-file; simpler than a crate dep for ~40 LOC.
- `base64` — not using PKCE, just CSRF `state` param.
- `webbrowser` — native `cmd /C start` / `open` / `xdg-open` dispatch inline, matching claudette.

`atty` was dropped — `std::io::IsTerminal` (stable since Rust 1.70; workspace MSRV is 1.75) handles the non-TTY check without an external crate.

Anchor versions in `workspace.dependencies` (workspace `Cargo.toml`) when a second crate wants the same dep.

## Out of scope (deferred)

- TUI (Sprint 2 — `crates/tui`).
- Forge pipeline stages beyond types (Sprint 3 — `crates/forge`).
- Verifier logic (Sprint 4 — `crates/verifier`).
- Anthropic Claude provider (v0.2; scaffold trait supports multi-provider).
- LanceDB / rich memory (v0.2 with `--features rich-memory`).
- Telegram / voice / MCP (Sprint 6+ — `crates/integrations`).
- Bench harness (Sprint 7 — `crates/bench`).
- Plugin / hook system — **deferred entirely** per user.
- Antipattern auto-detection (v0.2).
- Self-evolving few-shots — **dropped** per user (never load-bearing).

## Exit criteria

- [x] `cargo build -p claudettes-forge-core` clean, zero warnings. (verified after step 13)
- [x] `cargo test -p claudettes-forge-core` green. (103 lib + 2 integration = 105 tests after step 13)
- [x] `cargo clippy --all-targets -p claudettes-forge-core -- -D warnings` green. (verified after step 13)
- [x] Integration smoke test at `crates/core/tests/smoke.rs` runs and passes. (step 13)
- [ ] CI green on push (GitHub Actions workflow — still to be added; not blocking sprint close on the code side).
- [ ] AD-6 written if any new load-bearing architecture decisions land during the sprint. (candidates: `Prompt`/`Allow` bypass of dispatch tier gate; `IsTerminal`-based prompter instead of `atty`; toml-only frontmatter instead of YAML; OAuth flow deliberately dep-free of `chrono`/`url`/`base64`/`webbrowser` — each noteworthy but none load-bearing.)

## Known risks

- **OAuth lift may hit `webbrowser` crate cross-platform gotchas on Windows.** If so, fall back to `xdg-open`-style platform-dispatch inline (claudette already handles this in `google_auth.rs`).
- **Personas markdown parser choice.** TOML frontmatter chosen here — simple, widely-understood. Alternative: YAML frontmatter (more common in static-site generators) but adds a heavier YAML dep.
- **5-tier `Prompt` tier semantics in non-TTY environments.** `DenyingPrompter` is the safe default, but a CI that legitimately wants to run through `Prompt` tier needs an env-var escape hatch (e.g. `CLAUDETTES_FORGE_AUTO_ACCEPT=1`). Add in Sprint 1 when `TtyPrompter` lands.
- **`serde` derive cascades.** Adding `Serialize`/`Deserialize` to `Mission`/`Subtask`/`ModelMap` pulls a lot of trait impls into scope; worth auditing when it lands to avoid accidental derives on private fields.

## After this sprint

**Sequential pause** (user's plan): ship existing claudette v0.2 (as-is) + maybe BCF + maybe ABCC godfather. Resume on Sprint 2 (TUI crate).

## Next-session kickoff checklist

Sprint 1 core bodies (steps 2–8, 10, 11, 12, 13) are all done. Remaining for a formal sprint close: CI workflow (optional), AD-6 (optional), step 9 ship-claudette pause.

**To wrap sprint 1 housekeeping:**

1. **CI workflow** — `.github/workflows/ci.yml` running `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` on push. The repo doesn't have a remote yet (or a `.github/` dir); this lands when the repo gets pushed to GitHub.
2. **AD-6** — if the user wants a decision record capturing the non-load-bearing choices (tier bypass semantics, stdlib `IsTerminal`, TOML frontmatter, OAuth dep-minimalism), this is the place. Otherwise skip.

**To take the ship-claudette pause (step 9):**

Leave `claudettes-forge` where it is (compiles, 105 tests pass, clippy green). Switch to `D:/dev/claudette/` and cut v0.2. Resume at **Sprint 2 (TUI crate)** after. See `memory/claudettes_forge_decisions.md` for the sequential cadence decision.

**To start Sprint 2 directly (skip the ship-claudette pause):**

1. Open `crates/tui/src/lib.rs` — currently a stub.
2. Reference `D:/dev/claudette/src/tui/` for the current TUI patterns (paste-to-tempfile from `paste.rs`, typewriter code effect, just-space-invaders easter egg). Design fresh per user's decision; don't direct-lift from tacticode.
3. Sprint 2 plan doc doesn't exist yet — write `docs/sprints/sprint_02_tui.md` first with the same shape as sprint 01.
