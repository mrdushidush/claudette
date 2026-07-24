# Essence from the Ancestors — Salvage Audit, 2026-07-24

Final idea-mining pass over the archived project family (`D:\dev\_archive\`) before
letting it rest. Every idea below was diffed against claudette's current state
(v0.17.0, branch `battery/q50-quality-corpus`). Sources: Beast's own
`DECISION-beast-vs-claudette.md` + roast series, the 210 KB `PROJECT_REVIEW.md`,
`battle-command-forge/CLAUDE.md` (29 numbered learnings), tacticode, clawForge's
life-hacker crate, battleclaw-forge `CLAUDE.md`/`MVP-READINESS.md`, stealthsambaV2,
and independencev1.

---

## 1. Already inherited — verified present, ancestors can rest

- **On-demand tool groups** (life-hacker Sprint 8) → `tool_groups.rs`, now ~80 tools / 20 groups.
- **Tiered brain fallback with strict stuck-signals** (life-hacker `brain_selector.rs`) → present, 4B→9B.
- **Sessions, 3-tier permissions, CLAUDETTE.MD memory, auto-compaction** → all graduated from Claudet.
- **Best-round restore in fix loops** (forge learning #19) → forge restores best-scoring round before submit.
- **Real build+test verifier gate, fail-closed** → forge Verifier runs cargo/go/pytest/npm for real.
- **Red-at-base test validation** (Beast C3) → battery's `gate_q50.sh` authoring gate (untouched fixture must FAIL).
- **No-LLM-self-grading benchmark** → the Q50 battery's hidden-verification design already embodies the
  family's hardest-won lesson (see §4 below) — deterministic verify scripts, no judge model.
- **Brownfield missions** (clone → route → branch → PR) → present.
- **VRAM/hardware awareness** → `hw.rs`/`doctor.rs` recommend the fitting model.
- **Secretary features** (Telegram, voice, Gmail/Calendar, briefing) → graduated behind the `integrations` flag.
- **Prompt-injection provenance wrapping, deterministic-Rust-validates-LLM-suggests, AD-numbered docs** —
  claudette-native patterns the PROJECT_REVIEW names the family's high-water mark. Keep doing them.

## 2. Recommended imports — genuinely new, evidence-backed

Ranked by expected value.

### 2.1 Grounded-feedback fix loop  *(Beast C1 — designed, never built; highest upside)*
Forge fix rounds should receive the **actual test failure** — failing test id, `E` assertion
lines, traceback tail — not (only) the Verifier brain's prose feedback. Beast's sprint plan
(`_archive/Beast/runs/SPRINT-PLAN-grounded-feedback.md`) diagnosed "plausible-but-wrong patch
rubber-stamped by LLM judge" as the dominant failure and targeted 3/10 → ≥6/10 with real error
text injection. Claudette already runs the real tests in the Verifier — the missing piece is
piping their raw output into the fix-round prompt.

### 2.2 Agentic localizing Planner  *(Beast A1 — the one proven, repeatable lift)*
Beast's only reproducible win (+1 on Sonnet mini-10, per the user's own DECISION doc) came from
a Planner that *researches* first: repo map → targeted searches → hands the Coder the correct
files. Claudette's forge Planner is read-only-capped already; make "localize before plan" an
explicit phase-0 with `repo_map` + `grep` budget. Reference: `_archive/Beast/crates/beast-graph/src/live_planner.rs`.

### 2.3 Static clamp on the forge brain-score  *(discovered independently 3× in the family)*
The family measured LLM-judge inflation three times: +1.3 (qwen80b self-score), +3.40 mean
(stealthsambaV2 critic vs independent scorer), ~+50pts (Beast exec-oracle vs official grader),
plus the **7.0 floor** (judge collapses to a safe default under uncertainty — independencev1
died of it). The battery is immune (no judge). The forge gate is not fully: the brain emits
`{score,pass}` and best-round restore trusts the score. Import the clamp rules:
- build fails → score ceiling 4.0; tests fail N of M → ceiling `min(7.0, 10×(M−N)/M)`
- a second opinion may only **lower** a score, never raise (`final = min(...)`)
- instrument exact-default scores (how often exactly 7.0?) to detect judge collapse.
Sources: `_archive/abcc_projects/abcc_projects/independencev1/CLAUDE.md:85-209`.

### 2.4 Fix-round discipline guardrails  *(forge learnings #8/#12/#20 + Beast C2)*
Three cheap prompt/mechanical rules for forge fix rounds:
- **"Fix ONLY bugs — do not add features"** (reviewers ask for auth/rate-limiting; coder adds
  them and breaks working code — documented failure).
- **No editing test files** during fix rounds; flag over-scoped diffs (Beast's astropy-13033
  broke 3 pre-existing tests with an unasked-for extra override).
- **Deterministic cleanup pass before any LLM fix**: pattern-matchable defects (missing
  `pub mod`, missing derives, duplicate `_N.rs` files) fixed with zero model calls.
  stealthsambaV2's Hotfix #6 "eliminated almost all mechanical failures."

### 2.5 Generation watchdog timeout  *(Beast C5 — its most expensive operational failure)*
A single hung local-model generation killed a whole 10-instance Beast run (8 of 10 never ran);
the roast found **zero timeout on any of its 5 HTTP clients**. Claudette has loop-breakers and
iteration caps, but verify every model-call path has a hard generation timeout + abort. Cheap
insurance for exactly the local-model-stall failure mode claudette lives with.

### 2.6 Local-model behavior catalog  *(pure reference import — feeds the champion hunt)*
Hard-won empirical findings worth keeping at hand during the Q50 hunt
(source: `_archive/abcc_projects/abcc_projects/battle-command-forge/CLAUDE.md:175-205`):
- **MoE models are unreliable for surgical edits** (return empty on targeted-edit prompts) — #25
- 80B coder > 32B in multi-file coherence despite worse isolated benchmarks — #14
- 32B is the best architect (concise specs, no overengineering) — #15
- `qwen3-coder:30b` was the most *honest* critic (no inflation) — #3
- Temperature 0 for codegen — non-zero "amplifies small bugs into inconsistent outputs"
- VRAM efficiency > parameter count (30B MoE beat 123B on speed+reliability) — #4
- Decompose quality cascades: weak architect spec → toy code downstream — #1

### 2.7 Eval-methodology compendium  *(Beast B-tier — for any future harness-change evaluation)*
The battery grades models. When evaluating *harness* changes (forge tweaks, prompt changes):
- **Measure lift, not absolutes**: run a raw-model/single-shot baseline lane on the identical
  model + tasks; the harness's contribution is the delta (Beast's 7/16-vs-7/16 tie was only
  visible because both lanes existed).
- **Coverage telemetry on grounded gates**: tally grounded-vs-fallback per run with reasons —
  Beast's oracle silently fell back to the biased judge and "you cannot tell the gate from a no-op."
- **Delta-gates must refuse to compare non-identical instance sets** (Beast's compare silently
  dropped non-overlapping instances → false promotions).
- **Count skips in the denominator** (emit empty patch for unattempted, never omit).
- **Reasoning-trajectory triage**: taxonomy of waste patterns (search-ramp, tool-call-as-content
  loss, edit-but-ineffective loops) as a repeatable QA lens — `_archive/Beast/runs/reasoning-v23-report.md`.

### 2.8 Edit-tool consolidation input  *(Beast A2 — feeds the existing roadmap item)*
README roadmap already plans folding overlapping edit tools into one canonical `edit_file`.
Beast's data says the winning semantics are **fuzzy search/replace** (trimmed-line similarity
match), with one known landmine to fix in the port: the trim-match/verbatim-insert writes
mis-indented Python and reports success (`_archive/Beast/crates/beast-tools/src/fuzzy_patch.rs:78`
+ roast #3). Add a re-indent-`after`-to-match step — critical for indentation-significant languages.

### 2.9 Small extras (cheap, on-brand)
- **Model scorecard lite**: log per-model outcomes (pass/fail, latency, escalations) to a local
  JSONL; surface promote/demote hints. Lightweight fusion of tacticode's `selfEvolution.ts` and
  life-hacker's `fallback.jsonl`. Aligns with the "Claudette Certified" coverage roadmap.
- **Inline-code validation layer**: tacticode's shell layer-3 validated `python -c` / `node -e`
  payloads specifically — a gap between claudette's permission tiers and its egress guard.
- **"Saved vs cloud" status line**: `battleclawclean/rust/src/state/costs.rs` — even at $0,
  `estimate_cloud_equivalent()` ("Saved: $X") is a motivating one-liner for an air-gapped agent.
- **Venv-per-project test isolation** when the Verifier runs Python (forge learning #6:
  Pydantic v1/v2 system conflicts create phantom failures).

## 3. Deliberately cut — revive only with a new reason (owner's call)

These were **consciously removed** by claudette's legacy audits; the archives argue for some of
them, so recording the tension rather than re-importing silently:

| Idea | Cut | The counter-argument from the archives |
|---|---|---|
| Codet validator sidecar | v0.16.0 (~3.8k LOC) | Its *context isolation* (fix conversation never pollutes main context; one-line JSON summary back) is still distinctive; forge Verifier covers the rest. |
| `spawn_agent` sub-agents | legacy-audit PR-E | life-hacker's `FilteredToolExecutor` (per-agent tool allowlist + scoped permissions) was the clean part of the design. |
| Sentinel-9 persona wiring | legacy-audit PR-F | PROJECT_REVIEW's *locked rewrite decision* (§14.1-C) wanted CodeX-7 **and** Sentinel-9 from day 1 with a `--faceless` toggle — the personas "silently died" in two successors and the review called their return a "big bring-back." `sentinel9.md` is now restored to `crates/claudette/personas/`; wiring it stays a choice. |
| Swarm / best-of-N | never ported | Only defensible as *sequential* best-of-N (VRAM discipline) for rare hardest tasks; parallel form conflicts with one-model-resident. |
| Antipattern auto-detection, LanceDB graph memory | deferred by §14.1-H | The review rates them the family's most sophisticated memory work — but the owner already chose file-backed markdown. Deferred, not forgotten. |
| Self-evolving few-shots | dropped §14.1-H | Was documented-as-done for a year but never implemented ("never load-bearing"). Correctly retired — do not resurrect. |

## 4. The one lesson above all others

Four independent projects (battle-command-forge, stealthsambaV2, independencev1, Beast) each
paid separately to learn the same thing: **an LLM grading its own family's output inflates, and
under uncertainty collapses to a safe default.** The only fixes that ever worked were
deterministic ground truth (real tests, static analysis, official graders) with the LLM allowed
only to *lower* the resulting ceiling. The Q50 battery's hidden-verification design is the
mature expression of this. Any future feature that reintroduces a judge model must inherit the
clamp (§2.3) — that is the estate's binding covenant.

## 5. Correctly left behind (no action, recorded for peace of mind)

Enterprise layer (RBAC, multi-tenant Postgres, Prometheus, audit-log service), the TypeScript
middle tier ("single Rust binary can handle orchestration" — pitfall #6), cloud-judge mixes
(Gemini+Grok), the exec-oracle SHIP/NOSHIP verdict, CrewAI, Redis, and the RTS/voice-pack
theater (lives on in ABCC, which remains active).

---

*Archive locations referenced above are under `D:\dev\_archive\`. The lineage bible is
`_archive/abcc_projects/abcc_projects/PROJECT_REVIEW.md` (§12 cherry-pick catalog, §14 locked
decisions) with `TIMELINE.md` beside it.*
