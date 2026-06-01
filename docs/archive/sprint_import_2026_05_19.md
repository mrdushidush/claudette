# Sprint plan — `import_2026_05_19`

**Goal:** Land the Tier-1 import set from `docs/archive/import_sweep_2026_05_19.md` into claudette, then fold the `claudettes-forge` scaffold and archive it. After this sprint sequence, only claudette ships.

**Out of scope:** Tier-2 items not on the user's list (LanceDB rich memory considered tentatively in Phase 8; Independence reviewer / voice banks / MCP / swarm / multi-tenant RBAC explicitly deferred per `import_sweep_2026_05_19.md` §3).

**Velocity assumption:** Each phase ships as 1-3 commits in its own slot. Phases 1-2 are days of work; 3-7 are weeks. The plan optimizes for landable-in-pieces over big-bang.

---

## Phase ordering rationale

| Phase | Risk | Blast radius | Dependencies |
|---|---|---|---|
| 1. TUI lifts | Low | TUI input + chat render | None |
| 2. Personas + Eva | Low-medium | Prompt assembly | None |
| 3. Best-round restore + smart stopping | Medium | `forge/mod.rs` fix-loop | None |
| 4. 5-tier perms upgrade | Low (re-scoped) | `runtime/permissions.rs` policy only | None |
| 5. CTO chat agent | Medium | New layer above forge | Phase 2 |
| 6. SWE-bench + A/B bench | Medium-high | New `bench` crate | Phase 3 (best-round) for fair scoring |
| 7. Antipattern auto-detection | Medium | `forge/` + `memory.rs` | Phase 6 (needs failure corpus) |
| 8. LanceDB rich memory (decision) | — | TBD | Phase 7 evidence |
| 9. Fold claudettes-forge + archive | Low | Cleanup | All prior phases |

Phases 1-4 can interleave with normal claudette work; 5-7 each warrant a dedicated focus block.

---

## Phase 1 — TUI lifts (paste / typewriter / Space Invaders)

**Scope:**

- Copy `D:\dev\claudettes-forge\crates\tui\src\paste.rs` (146 LOC, tested) → `crates/claudette/src/tui/paste.rs`. Replace `claudettes-forge` temp-dir prefix with `claudette`.
- Copy `claudettes-forge/crates/tui/src/typewriter.rs` (163 LOC, tested) → `crates/claudette/src/tui/typewriter.rs`.
- Copy `claudettes-forge/crates/tui/src/space.rs` (388 LOC) → `crates/claudette/src/tui/space.rs`.

**Wire-up:**

- `tui.rs` input handler: on bracketed-paste event with len > 500, route to `PasteFile::try_store`; show preview in the input bar; on submit, replace with `retrieve()`.
- `tui/render.rs`: optional typewriter renderer for streamed code-fenced blocks — feature-flag via `--tui-typewriter` flag at first, default-on after one shipping cycle.
- New slash `/space` (and possibly `/play`) launches the Space Invaders modal — handled in `commands.rs`, dispatched as a `TuiEvent::Game(SpaceGame::new())`.

**Touch list:**

- New: `crates/claudette/src/tui/paste.rs`, `crates/claudette/src/tui/typewriter.rs`, `crates/claudette/src/tui/space.rs`.
- Edit: `crates/claudette/src/tui.rs` (mod declarations, input handler, key dispatch for game modal).
- Edit: `crates/claudette/src/tui/render.rs` (typewriter integration).
- Edit: `crates/claudette/src/commands.rs` (`/space` registration).
- Edit: `crates/claudette/src/tui_events.rs` if needed for `Game` event variant.

**Success criteria:**

- Existing tui_test_prompts.md scenarios still pass (no regressions).
- `cargo test -p claudette tui::paste tui::typewriter` green.
- Manual: paste >500 chars shows preview; submitting sends the full text. Typewriter visibly animates a code fence. `/space` opens the game; `q` or `Esc` closes it back to chat.

**Out of scope:** Snake easter egg (explicitly dropped per `import_sweep_2026_05_19.md` §5). Model-registry header strip (Tier-2 deferred). Tacticode 3-panel layout swap (Phase 9 or later — claudette's 5-tab layout is the current target).

**Rough effort:** Half a day.

---

## Phase 2 — Personas system + Eva

**Scope:**

- Copy `D:\dev\claudettes-forge\personas\{codex7,sentinel9,cto,eva}.md` → `personas/` at workspace root of claudette (i.e. `D:\dev\claudette\personas\`).
- Lift `claudettes-forge/crates/core/src/personas.rs` (500 LOC, 17 tests) → `crates/claudette/src/personas.rs`. Adjust imports (`crate::types::Role` → claudette's role enum or a new module).
- Add `Role` enum mirroring the personas file's role values (`assistant`, `planner`, `router`, `coder`, `test_coder`, `verifier`, `surgical_coder`, `cto`). Existing claudette forge has Planner/Coder/Verifier; lift adds Assistant/CTO/Router/TestCoder/SurgicalCoder as forward-compat — wire only Assistant + Coder + Verifier + CTO initially.
- Wire `Persona.backstory + examples` into system-prompt assembly:
  - In `prompt.rs`: when assembling the assistant system prompt, look up Eva, prepend backstory + first 3 examples.
  - In `forge/mod.rs`: when assembling Coder / Verifier prompts, look up CodeX-7 / Sentinel-9, prepend.
- CLI flag `--faceless` (and env `CLAUDETTE_FACELESS=1`): skips persona injection. Reason: tooling integrations / API users may not want the conversational tone.
- User-defined override loaded from `$CWD/.claudette/personas/*.md` (matches existing `.claudette/` convention).

**Touch list:**

- New: `crates/claudette/src/personas.rs`, `personas/*.md` (4 files).
- New: claudette-side `Role` enum (in `src/forge/types.rs` extended, or a new `src/roles.rs`).
- Edit: `crates/claudette/src/prompt.rs` to inject Eva for assistant turns.
- Edit: `crates/claudette/src/forge/mod.rs` to inject CodeX-7 / Sentinel-9 / CTO into role prompts.
- Edit: `crates/claudette/src/main.rs` for the `--faceless` CLI flag + env.
- Edit: `crates/claudette/src/lib.rs` (mod declaration).
- Edit: `Cargo.toml` if `toml` isn't already a dep at the claudette-crate level (it is — used by `model_config.rs`).

**Success criteria:**

- Eva backstory shows up in the system prompt for a default assistant turn (verifiable by `--show-prompt` flag if it exists, otherwise a unit test asserting the prompt builder includes "warm-efficient").
- CodeX-7 shows up in the forge Coder role prompt.
- 17 lifted tests pass; one new test exercises Eva persona being found.
- `CLAUDETTE_FACELESS=1` strips persona content from prompts (verified by test).
- `cargo build` clean. `cargo clippy -- -D warnings` clean. `cargo fmt --check` clean.

**Out of scope:** Hot reload of personas (§14 explicitly: restart required). Custom personas in user override beyond filename overlap (no namespacing). Voice tone modulation (separate Voice phase). Eva persona expanded — already shipped in claudettes-forge as `status = "loaded"`.

**Rough effort:** 1-2 days.

---

## Phase 3 — Forge best-round restore + smart stopping

**Scope:**

The current `forge/mod.rs` Verifier loop scores each round but doesn't track the best round's filesystem state. Add:

- After each Verifier pass that produces a score, snapshot the *files touched* (git stash-like or in-memory `HashMap<PathBuf, Vec<u8>>`) tagged with that round's score.
- Track running best-score and best-round-index.
- Smart stop: if score declines two consecutive rounds, break the loop.
- Restore: if `current_round_score < best_score`, restore best-round files to disk (and surface that decision in the run log).

**Source patterns:**

- `D:\dev\clawForge\crates\forge\src\mission.rs` (2005 LOC) — extract the round-tracking and best-round restore logic. **Solo-authored life-hacker phase, so safe to lift code per §14.5.** Verify with `git log` if unsure.
- Smart-stopping logic also documented in BCF learning #12 (regen always degrades).

**Touch list:**

- Edit: `crates/claudette/src/forge/mod.rs` — round state + score tracking + restore.
- New tests under `crates/claudette/src/forge/` exercising: (a) two declining rounds → break + restore; (b) monotonic improvement → finish on threshold; (c) flat scores → continue to max-rounds-then-stop.
- Possibly edit `forge/types.rs` for a `RoundReport` struct.

**Success criteria:**

- Unit test: a forced "score declines round 2 + round 3" path triggers restore-to-round-1 + break.
- Existing forge tests still pass.
- The forge e2e example (`examples/forge_e2e.rs`?) runs against a tiny mission and the run log surfaces "restored from best round N".

**Out of scope:** Multi-mission corpus aggregation (Phase 6 bench). Changing the Verifier score scale.

**Rough effort:** 2-3 days.

---

## Phase 4 — 5-tier permissions upgrade

**Re-scoped finding:** Claudette **already has** the 5-tier `PermissionMode` enum (`runtime/permissions.rs:3-10`) covering ReadOnly / WorkspaceWrite / DangerFullAccess / Prompt / Allow. The original report assumed claudette only had 3 tiers — that's stale.

**What `claudettes-forge` adds beyond what claudette has:**

| Surface | Claudette (today) | claudettes-forge | Verdict |
|---|---|---|---|
| 5-tier enum | ✅ Present | ✅ Present | No change |
| Tool-level requirements (`tool_requirements: BTreeMap<&str, Mode>`) | ✅ | ❌ | Keep claudette's |
| Operation-level types (`Operation::{ReadFile, WriteFile, Execute, Network, Other}`) | ❌ | ✅ Present | **Lift as v2 surface** |
| `AuthOutcome::{Allowed, Denied(reason), PromptRequired(op)}` | Has equivalent (`PermissionOutcome` + `PermissionPromptDecision`) | ✅ Cleaner naming | Consider rename |
| Tool-name suggestion heuristic (`suggest_for`, Levenshtein etc.) | ✅ | ❌ | Keep claudette's |
| `max_tier()` cap on policy | ❌ | ✅ | **Lift — cheap safety** |

**Actual work:**

- Add `Operation` enum + adapt `PermissionPolicy::authorize` to optionally take an `Operation`. Default tool-name lookup remains primary; operation-level becomes available for new tools.
- Add `max_tier()` cap to `PermissionPolicy` — refuses dispatch of a tool requiring `DangerFullAccess` when the session is configured `WorkspaceWrite`-max.
- Document in `docs/decisions.md` (or a new `docs/AD-permissions.md`).

**Touch list:**

- Edit: `crates/claudette/src/runtime/permissions.rs` — add `Operation` enum, `max_tier()` method, possibly rename outcome variants.
- Edit: every tool call site that wants the operation-level guard (file_ops / shell / web_search / etc.) — do this opportunistically, not as a blanket migration.
- Add unit tests for `max_tier` enforcement.

**Success criteria:**

- `cargo test -p claudette permissions` green with new ops-level tests.
- A tool registered as needing `DangerFullAccess` is denied at dispatch when policy `max_tier() == WorkspaceWrite` even before the prompter is consulted.
- No regression on existing 3-tool permission tests.

**Out of scope:** Wholesale rewrite of every tool's authorize path (~50 sites). Operation-level guard is opt-in per tool.

**Rough effort:** 1-2 days.

---

## Phase 5 — CTO chat agent

**Scope:**

A strategic-loop agent that sits above the forge pipeline. Distinct from CodeX-7 (the coder) — CTO frames the *mission* itself: clarifies scope, decomposes into milestones, decides when to invoke forge vs hand back to the user.

**Sources (idea-only lift; solo-or-godfather code OK):**

- `D:\dev\clawForge\crates\forge\src\cto.rs` — small file, 10-native-tools pattern, 5-iter cap.
- `D:\dev\agent-battle-command-center\packages\agents\src\agents\cto.py` — godfather, solo, intact tree.

**Surface:**

- `/cto <mission>` slash command — opens a sub-conversation framed as CTO with persona injection (Phase 2 prerequisite).
- `claudette cto <mission>` CLI subcommand for one-shot strategic decomposition (writes a milestone plan to a file).
- History persisted to `~/.claudette/cto-sessions/<id>.jsonl`.
- 10 tools allowed (subset of full tool registry): notes / todos / file_ops / web_search / forge invocation / git / github / calendar / gmail / shell with WorkspaceWrite-only.

**Touch list:**

- New: `crates/claudette/src/cto.rs` (200-400 LOC est.).
- Edit: `commands.rs` for `/cto` slash.
- Edit: `main.rs` for `cto` subcommand.
- New: persona wiring (depends on Phase 2 being live).

**Success criteria:**

- `/cto "build me a CSV-to-JSON tool"` opens a sub-session, CTO persona greets, asks one clarifying question, then proposes a 3-milestone plan with concrete forge invocations.
- Persisted session can be resumed with `/cto resume <id>`.

**Out of scope:** Multi-CTO (one per project). Persistent state across machine restarts beyond filesystem.

**Rough effort:** 3-5 days.

---

## Phase 6 — Bench harness (SWE-bench + A/B + multi-template)

**Scope (big phase, may split into 6a + 6b):**

- **6a — SWE-bench runner.** Lift `D:\dev\clawForge\crates\forge\src\{swebench.rs (725), swebench_eval.rs (87), swebench_tools.rs (293)}`. Wire as `claudette bench swe --fixture <path>` subcommand. Outputs JSON.
- **6b — Multi-template + A/B.** Build 10-template fixture set (storefront / arcade / portfolio / restaurant / dashboard / csv-analytics / log-parser / rms-scheduler / dns-parser / markdown-converter / task-queue / config-merger). A/B knobs:
  - `--ab qa` runs each mission twice: WITH-QA verifier, WITHOUT-QA.
  - `--ab url` runs each mission twice: WITH-URL reference, WITHOUT.
  - `--ab determinism` runs each mission twice on the same model + temp, compares outputs.
- Output: `results.json` per run + a summary CSV for cross-run comparison.

**Crate layout decision:**

- Option A: new `crates/claudette-bench/` workspace member (matches `claudettes-forge` plan).
- Option B: under `crates/claudette/src/bench/` as a module.

Recommend **Option B** initially — keeps claudette a single crate per its current shape. Promote to a workspace member only if bench grows beyond ~1500 LOC.

**Touch list:**

- New: `crates/claudette/src/bench/{mod,swebench,templates,ab}.rs`.
- New: fixture corpus under `bench/fixtures/`.
- Edit: `main.rs` for `bench` subcommand tree.
- Edit: `Cargo.toml` for any new bench-only deps (probably none — reuse existing).

**Success criteria:**

- `claudette bench swe --fixture bench/fixtures/swe-mini.json` runs end-to-end and writes `results.json`.
- `claudette bench ab qa --templates 3` runs 3 templates × 2 modes and writes a comparison summary.
- Round-3-e2e methodology (`project_e2e_sweep_2026_05_16_round3.md`) is reproducible via this harness.

**Out of scope:** Cross-machine result aggregation. CI-runnable benches (separate effort — needs runner sizing).

**Rough effort:** 1-2 weeks (this is the biggest phase).

---

## Phase 7 — Antipattern auto-detection

**Scope:**

Closes the godfather "self-evolving few-shots" aspirational loop with a corpus-grounded design:

- Failed forge missions write a structured `failure.json` into `~/.claudette/failures/<mission-id>/`.
- Field: `{pattern: <hashed root cause>, count: <occurrences>, examples: [...], graduated: bool, graduation_rule: Option<String>}`.
- On each new failure, similarity-search the corpus (use existing `recall.rs` embeddings — already in claudette).
- When ≥3 failures within a similarity threshold (e.g. cosine ≥ 0.85) and `graduated == false`: auto-graduate a hard rule into the Engineer prompt overlay (the prompt assembly checks `~/.claudette/antipatterns/active.toml` and appends graduated rules).
- Graduated rules can be reviewed / demoted via `/antipattern list` and `/antipattern demote <id>` slashes.

**Depends on Phase 6** because we need a way to test that "X causes failures, Y rule prevents them" — bench is the controlled environment.

**Touch list:**

- New: `crates/claudette/src/antipatterns.rs`.
- Edit: `forge/mod.rs` for failure capture hook.
- Edit: `prompt.rs` or `forge/mod.rs` for graduated-rule injection.
- Edit: `commands.rs` for `/antipattern` slashes.

**Success criteria:**

- Synthetic test: induce 3 similar failures → assert rule graduated → assert next prompt includes graduated text.
- Bench harness shows pass-rate improvement on a known-bad mission template after antipattern graduates.

**Out of scope:** Multi-user shared antipattern corpora. Auto-demotion based on rule efficacy (Phase 7.5 idea).

**Rough effort:** 1 week.

---

## Phase 8 — LanceDB rich memory (decision)

Not a build phase yet — a **decision phase** triggered by Phase 6/7 evidence.

**Decision criteria (revisit after Phase 7 lands):**

- Does the failure corpus + recall combo strain claudette's flat-file + embeddings approach?
- Are forge missions retrieving "near-miss" precedents reliably?
- If embedding recall miss-rate < 10% on the bench corpus, **stay file-backed** (claudette's principle).
- If miss-rate > 20%, prototype LanceDB behind `--memory=rich` feature flag.

**Status:** Defer until data exists. Not in this sprint's commit budget.

---

## Phase 9 — Fold claudettes-forge → claudette + archive

**Scope:**

Once Phases 1-7 land, claudettes-forge has no remaining unique value (verified by §1 of `import_sweep_2026_05_19.md` plus the phase deltas). Archive it.

**Steps:**

1. **Audit gaps.** Diff `D:\dev\claudettes-forge\crates\*` against the new modules in claudette. Flag anything not yet lifted.
2. **Lift docs.** Copy `claudettes-forge/docs/{architecture.md, decisions.md, sprints/*}` into `claudette/docs/` — adapt naming as needed. Preserve the sprint history.
3. **Tag the scaffold.** `cd D:\dev\claudettes-forge && git tag archive-2026-XX-XX && git push --tags`.
4. **README** the scaffold: short note pointing at claudette as the live product.
5. **Memory update.** Mark `claudettes-forge` as superseded in claudette's MEMORY.md.

**Touch list:**

- New: `claudette/docs/architecture.md` (may already exist — diff first).
- Edit: `claudettes-forge/README.md` to point at claudette.
- New git tag in claudettes-forge.

**Success criteria:**

- All claudettes-forge tests pass on the tagged commit.
- README points at claudette.
- Memory index notes the supersession.
- No code is deleted from claudettes-forge in-place — it stays as a frozen reference.

**Rough effort:** Half a day after Phase 7.

---

## Cross-cutting checks

Every phase ships with:

- `cargo fmt --all` (per [[feedback-pre-commit-checks]]).
- `cargo clippy --workspace -- -D warnings`.
- `cargo test -p claudette`.
- Manual TUI smoke test if the phase touches `tui.rs` (Phases 1, 5).
- A CHANGELOG entry under the next semver bump.
- A short follow-up memory note in `C:\Users\david\.claude\projects\D--dev-claudette\memory\`.

---

## What can ship in parallel

- Phase 1 + Phase 4 are independent — can be one commit each in the same session.
- Phase 2 must land before Phase 5.
- Phase 3 must land before Phase 6 (we want best-round restore active when bench runs A/B).
- Phase 6 must land before Phase 7 (need failure corpus).
- Phase 9 is last.

**Recommended sprint 1 commit budget:** Phases 1 + 2 + 4 + 3 in roughly that order. Phases 5-7 each warrant their own dedicated sprint after sprint 1 ships.

---

## Open decision points (block-cutting these before Phase 5+ starts)

1. **CTO persona vs CTO agent distinction.** Phase 2 lifts the CTO *persona* (markdown file). Phase 5 lifts the CTO *agent* (chat loop). Are they the same surface? Recommend: persona supplies the system prompt; the agent supplies the tool budget + iteration cap. Phase 5 confirms.
2. **`/space` vs `/play space`.** §14 I5 said "Just Space Invaders easter egg — redesign." Single command `/space` recommended for discoverability.
3. **Bench fixtures shipped in-repo or downloaded on demand?** §11 corpora are large. Recommend: in-repo for the 10-template baseline (small); SWE-bench fixtures downloaded on first run.
4. **claudettes-forge final disposition.** Tag + freeze (Phase 9 plan), or delete the GitHub repo entirely? Recommend tag + freeze for history.

---

## Phase status (live)

- [ ] Phase 1 — TUI lifts
- [ ] Phase 2 — Personas + Eva
- [ ] Phase 3 — Best-round restore + smart stopping
- [ ] Phase 4 — 5-tier perms upgrade (re-scoped: claudette already has 5-tier; only Operation enum + max_tier remain)
- [ ] Phase 5 — CTO chat agent
- [ ] Phase 6 — Bench harness (SWE-bench + A/B + multi-template)
- [ ] Phase 7 — Antipattern auto-detection
- [ ] Phase 8 — LanceDB decision (deferred to after Phase 7)
- [ ] Phase 9 — Fold claudettes-forge + archive
