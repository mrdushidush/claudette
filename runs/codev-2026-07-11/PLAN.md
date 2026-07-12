# Co-dev backlog sprint — tracker (started 2026-07-11)

Goal doc: `launch-drafts/goal_codev_backlog_2026_07_11.md`. Protocol: Claude specs
task cards → **Claudette (forge, byteshape champion brain) implements + opens PR** →
Claude reviews → David merges (`--rebase`). One card in flight. Escalation: 2 failed
forge attempts → Claude solo (failure mode recorded first).

Grounding verified 2026-07-11: main = `77a8883`, clean tree, 0 open PRs, issues
#137/#138/#139 open by design.

## Status board

| item | what | mode | status |
|---|---|---|---|
| W0 | Ground + rig co-dev bench | Claude | **DONE** — see rig notes |
| W1 | claudette100_test.sh default repoint | Claudette card | **PR #183 OPEN, Claude-reviewed LGTM — awaiting David merge** |
| W2 | hw.rs recommendation refresh | **GATE ANSWERED: Option A (qwen3.5:4b)** | **DONE — PR #185 MERGED `17d11b4`** |
| W3 | Repo tidy sweep | Claude (+small cards) | **DONE** — 22 branches deleted, PR #184 merged, ledger below |
| W4 | Post-edit check loop | **GATE ANSWERED: PROCEED** | W4a **MERGED `02cb673`** (PR #186, escalation-finished); **W4b mission IN FLIGHT** (behavioral when ON → A/B before merge) |
| W5 | Context eviction | **GATE ANSWERED: PROCEED (60%)** | design `design-context-eviction.md` signed off; card `cards/W5a.md` ready; W5b = Claude-led after W5a |
| W6 | Wave B / Wave E | PARKED unless David joins | parked |
| W7 | TUI doubled-text (timeboxed) | optional | **PARKED with probe evidence** — TUI renders clean non-interactively (alt-screen + tab bar OK); the doubled-text artifact needs a human at a live terminal to reproduce; no public issue filed without fresh repro |

## David gates (answered 2026-07-11 via AskUserQuestion)

- **W2 Ollama ≥16 GB slot → Option A: `qwen3.5:4b`** (45/50 + K 8/8; tier collapses
  to the 4b with the LMS-switch pointer kept in alternatives).
- **W4 post-edit check → PROCEED** as designed (opt-in OFF, 2 cards).
- **W5 context eviction → PROCEED** with the 60% trigger (W5a Claudette card,
  W5b Claude-led).

## A/B baseline state

- `git log 028098f..main` = docs/battery/test-only (incl. 9486976 test-only) →
  **existing 4b + 35b(q3_k_xl) baselines STAND** (runs/issues-2026-07-09/).
- **byteshape brain100 baseline on main `77a8883`: DONE — 90/100, wall 1535 s
  (25.6 min)** (`runs/codev-2026-07-11/battery-byteshape-baseline/`). Tiers:
  T1 19/20 · T2 18/20 · T3 19/20 · T4 16/20 · T5 18/20.
  **All 10 fails are known noise classes** (failures.txt): #13/#44/#91
  `src/`→`crates/` stale-corpus paths (answers correct, pattern misses);
  #29/#33 non-interactive permission refusals (bash escalation / web fetch);
  #65/#67/#80 phrasing ("doesn't exist" doesn't match `not exist`); #75 counted
  tool GROUPS (20) vs tools; #87 corpus expects `refactor|feat|fix` in last-5
  commits but post-merge history is all `docs(...)`/`test(...)` — a
  moment-in-time artifact. Adjusted ≈ high-90s, consistent with the 50/50 core
  battery. A/B judgments use per-task deltas vs THIS run under identical
  conditions (same non-interactive harness), so the classes cancel.

## PR ledger

| PR | title | author | behavioral | rounds | state |
|---|---|---|---|---|---|
| #183 | test(claudette100): default harness model to byteshape champion | **Claudette-forge** (round 0, attempt 1) | no (harness-only) | Verifier pass round 0; Claude review: LGTM, 0 changes requested | **MERGED `0c22e85`** (rebase, 2026-07-11) |
| #184 | docs(battery): publish v0.16.0 model-sweep score evidence | Claude (W3.2 direct docs-commit) | no (data-only) | — | **MERGED `8762d29`** (rebase, 2026-07-11) |
| #185 | fix(doctor): refresh brain recommendations from the 2026-07 batteries | **Claudette-forge** (attempt 1; Verifier 10/10 at round 1 — round-0 fail was the no-committed-diff pipeline gap, not code) | no (doctor output only) | Claude review: LGTM, 0 changes | **MERGED `17d11b4`** (rebase, 2026-07-11) |
| #186 | feat(tools): pure post-edit-check module | **Claudette-forge code (100% of module + 14 tests), Claude escalation-finish** (2 attempts died on harness limits; Claude: +6-line ALLOW block, gate, plumbing) | no (dead code until wired) | Claude full code review: LGTM | **MERGED `02cb673`** (rebase, 2026-07-11) |

## W0 rig notes (2026-07-11)

- Champion loaded: `lms load "qwen3.6-35b-a3b-mtp" -c 65536 --parallel 1
  --speculative-draft-mtp --speculative-draft-max-tokens 2 -y` → 13.61 GB,
  ctx 65536, parallel 1, loaded in 13.5 s.
- **Identifier correction (affects docs + cards):** LM Studio registers the local
  copy of the byteshape model under the bare key **`qwen3.6-35b-a3b-mtp`** — NO
  `byteshape/` prefix (catalog downloads would carry the prefix; this copy was
  side-loaded). `lms load byteshape/...` (the champion-launch.md command) FAILS;
  the chat API *does* fuzzy-match the prefixed form, but `--doctor`'s exact-match
  brain probe flags it. Fixed: `~/.claudette/.env` repointed to the exact id
  (doctor now ✓); W1 card used the exact id. Public docs keep the prefixed form
  (correct for catalog installs).
- `~/.claudettes-forge/models.toml` was STALE (all 3 roles → old `qwen3.6-35b-a3b`
  bare id, ambiguous across 3 installed variants) → repointed all 3 roles to
  `qwen3.6-35b-a3b-mtp`.
- Scratch clone: `D:/dev/claudette-forge` @ `77a8883`. `gh auth status` ✓.
  `CLAUDETTE_OFFLINE` unset ✓.
- Doctor confirmed the stale W2 target live pre-fix: "recommended brain:
  qwen3.6-35b-a3b@q3_k_xl — 92% on the 50-task battery" (hw.rs:103).

## W1 forge dogfood record (card: `cards/W1.md`)

- Wall: mission fired 18:00-ish, PR #183 open 18:03 — **~4 min card-to-PR.**
- Planner: correct 4-step plan, zero drift. Coder: 2 apply_diff calls (second a
  small self-correction), ran all 4 gates itself (1008 + 1095 tests), committed,
  opened the PR. Verifier: score 8 pass, build+1042 tests re-run.
- Diff quality: **byte-exact to spec.** Commit message exact, no trailer.
- Claude review: LGTM round 0; gates re-run locally in D:\dev\claudette — all green.

## W3 tidy ledger

1. **Local branches:** `git branch --merged` is useless under rebase-merges (0 hits).
   Used `git cherry main <b>` patch-equivalence instead: **22 branches fully
   equivalent to main** (safe delete): c2-recall-index, c3-cli-prompter,
   c4-runtime-build, c5-forge-run, c6-repl, c-final-clippy-allows,
   chore/release-prep-v0.16.0, docs/bash-windows-guidance, docs/truth-pass-2026-07,
   feat/cli-prompt-single-key, feat/edit-near-miss-diagnostics,
   feat/graceful-iteration-cap, feat/grep-search-glob, feat/repo-map-csharp,
   feat/semantic-grep-stopwords, feat/shell-guard-destructive-git,
   fix/offline-guard-registry-tools, fix/undo-path-revalidation, persona-take2,
   refactor/retire-codet-coder-subsystem, release/v0.10.0, scratch/ab-w1-w31.
   **NOT deleted yet** (doing it in one batch at wrap). **3 branches have unmerged
   commits — DAVID's call:** `perf/elide-stale-repo-map` (1: elide stale repo_map
   from wire — relates to deferred history-eviction/W5!), `feat/repl-activity-indicator`
   (2: cursor-hide + newline-swallow spinner fixes — post-#60 follow-ups never PR'd),
   `fix/write-file-test-env-leak` (2: test env pinning — possibly superseded by #178).
   **Remote branches for David:** `origin/chore/cleanup-v0.6-aliases` (merged),
   `origin/docs/model-recs-2026-07` (#182, merged), `origin/perf/elide-stale-repo-map`
   (unmerged twin of the local one).
2. **Evidence audit:** tracked runs/ (269 files) all committed+pushed ✓. Found the
   v0.16.0 sweep SCORES (23 files incl. screeners + BROKEN-template documentary)
   sat untracked → **PR #184**. Still local-only (listed for David, deliberate?):
   June-era recheck/probe/nemotron/hardening tsvs, `runs/issues-2026-07-09/`
   (incl. the A/B baselines battery-{4b,35b}-new), older run dirs, logs/work/fixtures
   (gitignored by design). `runs/codev-2026-07-11/` gets committed at sprint end.
3. **CHANGELOG:** `[Unreleased]` heading present and correctly holds the #182 docs
   entry. Nothing to do.
4. **deny.toml:** `cargo deny check` → advisories/bans/licenses/sources ALL OK; the
   flagged getrandom/wit-bindgen skips are already gone. Nothing to do, no card.
5. **Stale-claim grep:** README/docs clean — all q3_k_xl/q4_k_xl/47-50 mentions are
   legitimate rollback/previous-default phrasing. No stragglers.

## W4a forge record (escalation-rule bookkeeping)

- **Attempt 1 (card `cards/W4a.md`): FAILED — iteration cap, NOT comprehension.**
  Round 0: Coder wrote the full ~430-line module + registration in one
  write_file, then iterated the gate: fixed a vec! type mix, correctly
  diagnosed `Path::new("main.go").parent() == Some("")` (subtle Rust gotcha —
  impressive), reworked truncation-marker logic + its own test expectations,
  hit 2 apply_diff misses (block-not-found, ambiguous-match) along the way,
  and died on "conversation loop exceeded the maximum number of iterations"
  BEFORE the Verifier ever ran. Autopsy: WIP left uncommitted on the clone;
  Claude's gate re-run showed **13/14 tests passing, clippy clean** — sole
  defect = one assertion omitting the marker's leading `\n` in its byte-count
  allowance. Card-design lesson: a whole-module card with 14 tests exceeds the
  per-turn iteration budget when several fix cycles hit; either raise
  CLAUDETTE_MAX_ITERATIONS for module-scale cards or split module/tests.
- **Attempt 2 (card `cards/W4a-attempt2.md`): FAILED — empty-turn flake, not
  comprehension.** Planner: perfect one-line plan. Coder: applied the exact
  +2-byte fix (`\n` added to the assertion allowance, verified in tree), then
  the mission died on "assistant stream produced no content" (round 0) before
  gate/commit — the known empty-turn class (one enable_tools-hint retry logged
  earlier in the same turn). The CODE never failed; the harness turn did.
- **ESCALATION (2 failed attempts): Claude finished W4a → PR #186.** Claude's
  gate run surfaced ONE issue neither attempt reached: the
  `every_env_var_is_documented` doc-drift guard flags the three new knob env
  vars — a constraint the W4a CARD missed (card-design gap, charged to Claude,
  not the model). Resolution: ALLOW-listed with "inert until W4b wires them"
  reason; W4b card must move them to configuration.md and drop the allows.
  Final: Claudette wrote 100% of the module + 14 tests; Claude's code
  contribution = the 6-line ALLOW-list block. Full gate green (1022 + 1109).

## W4b forge record

- **Attempt 1 (card `cards/W4b.md`): FAILED — iteration cap (40) again.**
  Planner grounding was outstanding (exact line numbers for every insertion
  across 5 files). Coder cleanly edited 4/5 files (8 apply_diffs, zero misses)
  then hit the cap before docs/configuration.md + gate + commit. Confirmed
  pattern: DEFAULT_MAX_ITERATIONS=40 is too small for multi-file cards once
  reads are counted. Not a comprehension failure.
- **Attempt 2 (card `cards/W4b-attempt2.md`): fired with
  CLAUDETTE_MAX_ITERATIONS=100** (the knob exists for exactly this) +
  ALLOW_DIRTY continuation from WIP.
- **Rig lesson for future runs:** module-scale cards need
  CLAUDETTE_MAX_ITERATIONS≈100 from the start; 40 suits single-file cards.
- **Attempt 2: SUCCESS** (Verifier 10/10, gate green, clean commit `224afec`,
  Claude pushed → **PR #187**). BUT Claude review found the 3 conversation.rs
  integration tests MISSING — attempt 1 never wrote them and Claude's attempt-2
  card wrongly asserted they existed ("verify, don't redo"), so the Verifier
  passed against a false premise. **Charged to Claude's card, not the model.**
  Both attempts spent → Claude wrote the 3 tests (`386c08e`): knob-off
  byte-identity, failure-append, round-cap — plus hoisted the module's test
  ENV_LOCK to crate visibility (both test mods mutate the same env vars in one
  parallel test binary; two locks = flake risk). All 19 feature tests green;
  full suites 1027 + 1114.
- **A/B gate evidence (behavioral when ON):**
  - **byteshape knob-ON: 90/100, wall 1570 s** (`battery-byteshape-postedit-on/`)
    vs baseline 90/100 @ 1535 s (+2% wall, noise). Per-task: 8 shared fails
    (all known-noise); baseline-fails #13/#87 now PASS (#87 = the corpus
    commit-prefix artifact self-resolving after the sprint's feat/fix merges);
    new fails #30 ("6,912" thousands-separator vs pattern `6912`) + #32
    ("+03:00" phrasing) — **neither task invokes write tools → knob cannot be
    causal**; both inside the ±3 stochastic band. **VERDICT: no regression.**
  - Feature never fired during the battery (silence-on-success — battery
    writes are valid code), so the failure path was proven separately: **live
    one-shot probe** (champion brain, forced-fail CHECK_CMD) delivered
    `[post_edit_check] … git: 'definitely-not-a-subcommand' …` inside the
    write_file tool result, model echoed it verbatim. End-to-end confirmed.
  - **4b knob-ON, run 1: KILLED at 44/100** (background task stopped, cause
    unknown; orphaned claudette generation left LMS's single slot busy).
    **Run 2: CONTAMINATED** (started while the zombie generation still held
    the slot) — 90/100 headline but 6 "new" fails ALL of the empty-turn-on-
    read-task signature; preserved at `battery-4b-postedit-on-CONTAMINATED/`.
  - **4b knob-ON, run 3 (clean reload): 90/100 @ 1925 s** vs baseline 93/100 —
    inside the documented ±3 band. Decomposition: +2 recovered (35, 48);
    −5 new = 3× empty-turn stream-death on SEARCH/READ tasks (25/45/47) + 1
    notes-fixture miss (46) + 1 stale-corpus phrasing (91). **No write tools in
    any delta task; the hook short-circuits on tool name before reading env →
    no causal mechanism. Determinism probes: the empty-turn prompts re-ran
    clean both knob-OFF and knob-ON with identical token counts** → class =
    server-state (today's LMS churn), same empty-turn family as runtime
    2.24.0's known behavior. Zero `[post_edit_check]` markers fired in either
    battery (silence-on-success; failure path proven by the live probe).
  - **VERDICT: A/B GATE PASSED** (flag: a knob-OFF 4b re-run on a fresh LMS
    boot would fully close the empty-turn confound if David wants
    belt-and-braces). Evidence comment on #187; merge recommended.

## WRAP PACKET (2026-07-12, session ended at David's request — machine needed)

### 1. PRs

| PR | title | author | behavioral | rounds/attempts | state |
|---|---|---|---|---|---|
| #183 | test(claudette100): harness default → champion | **Claudette-forge** | no | 1 attempt, Verifier round 0, review LGTM | MERGED `0c22e85` |
| #184 | docs(battery): v0.16.0 sweep SCORES evidence | Claude (W3 direct) | no | — | MERGED `8762d29` |
| #185 | fix(doctor): brain recs ← 2026-07 batteries | **Claudette-forge** | no | 1 attempt; Verifier 10/10 @round 1 (round-0 fail = pipeline gap) | MERGED `17d11b4` |
| #186 | feat(tools): post-edit-check pure module | **Claudette code 100%**, Claude escalation-finish | no (dead code) | 2 attempts (harness limits), Claude +6 lines | MERGED `02cb673` |
| #187 | feat(runtime): wire post-edit checks | **Claudette wiring**, Claude tests commit | **YES when ON** (default OFF) | 2 attempts (iter-cap → raised to 100 → 10/10) | **OPEN, A/B PASSED, merge recommended** |
| (pending) | evidence commit of this sprint dir | Claude | no | — | see below |

### 2. Forge dogfood scorecard (champion as coder)
- **Code quality: excellent.** Byte-exact diffs on W1/W2; full 461-line module +
  14 tests on W4a; correct multi-file wiring on W4b. Self-diagnosed the
  `Path::parent()==Some("")` Rust edge unaided. Zero wrong-file touches across
  all missions; zero Co-Authored-By violations; commit messages exact.
- **Harness, not model, was every failure:** iteration cap 40 (×2), empty-turn
  stream flake (×1), dirty-tree guard from test litter (×1). Verifier value:
  caught nothing Claude's review didn't; passed one false premise (missing
  tests) and hallucinated one defect while passing (W1 `-a3b`). Claude review
  caught: missing integration tests (#187), env-var doc-guard (card gap),
  cross-module ENV_LOCK race.
- **Speed:** card→PR ≈4 min (W1), ≈8 min (W2); W4a/W4b ≈15–40 min incl. retries.
- Full friction log below (10 findings) — raw material for the launch-story
  draft (NOTHING posts; David fires).

### 3. Battery evidence
- `battery-byteshape-baseline/` 90/100 (main `77a8883`; all fails known-noise).
- `battery-byteshape-postedit-on/` 90/100 — parity, no write-path deltas.
- `battery-4b-postedit-on/` 90/100 vs 93 baseline — in-band; deltas = server-state
  empty-turn + fixture/corpus noise; determinism probes clean both knob states.
- `battery-4b-postedit-on-CONTAMINATED/` quarantined (zombie-slot run).
- Live failure-path probe: `[post_edit_check]` delivered + echoed by champion.

### 4. Tidy ledger
- 22 stale local branches DELETED (cherry-verified). 3 kept for David:
  `perf/elide-stale-repo-map` (W5-adjacent), `feat/repl-activity-indicator`
  (2 unshipped spinner fixes), `fix/write-file-test-env-leak` (**now
  load-bearing — fixes the forge litter trap; recommend PR'ing it**).
- Remote branches for David's ack (list-only, none deleted):
  `origin/chore/cleanup-v0.6-aliases` (merged), `origin/perf/elide-stale-repo-map`
  (unmerged twin). `origin/docs/model-recs-2026-07` no longer exists.
- Evidence: sweep SCORES published (#184); tracked runs/ verified pushed;
  deny.toml clean (no getrandom/wit-bindgen skips); CHANGELOG `[Unreleased]`
  correct; stale-claim grep clean.
- Local-only leftovers (deliberate, David's call): June-era recheck/probe/
  nemotron tsvs, `runs/issues-2026-07-09/` baselines, older run dirs.

### 5. Parked / not done — and why
- **W5 (context eviction): design SIGNED OFF (60%) + W5a card READY — parked
  ONLY because the session ended;** fire `cards/W5a.md` after #187 merges
  (needs `CLAUDETTE_MAX_ITERATIONS=100`). W5b = Claude-led (decided at spec).
- W4 default-ON: separate David decision after dogfood time.
- W6 Wave B/E: parked per goal doc (David never joined for them).
- W7 TUI doubled-text: parked — non-interactive render is clean; repro needs
  David at a live terminal. No public issue filed (no fresh repro).
- #187 merge + the optional knob-OFF 4b belt-and-braces re-run: David.

### 6. Honest flags
- The 4b empty-turn cluster is attributed to LMS server-state on mechanism +
  determinism evidence, but a fresh-boot knob-OFF battery would close it
  conclusively; I chose not to spend the extra 32 min against David's wrap
  request. Default ships OFF regardless.
- The A/B corpus has known blind spots for this feature (batteries write
  mostly-valid code, so the check's failure path went unexercised in-battery;
  covered by unit/integration tests + live probe instead).
- `claudette --doctor` was verified green post-repoint, but the champion sat
  unloaded overnight (TTL) — JIT reload works, still worth knowing.
- Background-task kill of 4b run 1: cause never identified (not reproduced).

## Friction log (co-dev dogfood findings — first-class deliverables)

1. **champion-launch.md load command fails verbatim** (W0): `lms load
   "byteshape/qwen3.6-35b-a3b-mtp"` → not found; local model key is bare
   `qwen3.6-35b-a3b-mtp`. Crib-sheet correction is a runs/ edit — David's call.
2. **`lms load` failure exits 0 through a pipe** (W0): `lms load ... | tail`
   masked the failure. Always confirm with `lms ps` after loads.
3. **models.toml stale-id trap** (W0): forge role-routing predated the crown and
   pointed at an ambiguous id; nothing in `--doctor` checks forge role-routing
   health. Doctor-check candidate.
4. **Card-vs-pipeline mismatch** (W1): the D1a house-style card ends "Open the PR;
   STOP" — correct for REPL co-dev, but under forge the COD ER obeyed it (gates +
   commit + `gh` PR itself), so the Submitter found a clean tree and closed the
   mission on the "ephemeral/local mission — no GitHub PR target" path even though
   PR #183 was open. Harmless here, but forge cards should say "STOP after the
   gate; the pipeline submits" — OR forge should detect an already-open PR.
   → W4/W5 cards will drop the "open the PR" line.
5. **Verifier hallucinated a defect while passing** (W1): score 8 + "diff omits
   the `-a3b` segment" (false — string complete). Human/Claude review still
   earns its keep.
6. **Same-account approval refused** (W1 review): GitHub rejects `gh pr review
   --approve` on a PR authored by the same account the forge pushes with —
   co-dev reviews must be `--comment`.
7. **Forge telemetry line reads `iter=0 in=0 out=0`** (W1): token/iteration
   counters didn't record. Metering bug candidate.
8. *(supersedes the #4 interpretation)* **Ephemeral missions never self-submit:**
   auto-bootstrapped forge missions ("NO active brownfield mission") have no
   GitHub PR target BY DESIGN — `mission_submit`'s push+PR path needs a
   configured mission. W1's PR happened only because the card told the Coder to
   open it. Adopted protocol from W2 on: card says commit-but-don't-push;
   **Claude pushes the mission branch + opens the PR** (same account either way).
9. **The Verifier reads only COMMITTED diffs** (W2 round 0): the card's
   "don't commit — the pipeline does it" instruction (per the old Submitter
   contract) left the Verifier with no diff → score 0 fail → the Coder burned
   fix-round 1 committing to recover (then 10/10). Cards must instruct the
   Coder to COMMIT (not push). W4a/W5a cards corrected. Pipeline-design note:
   Verifier could diff the working tree instead — improvement candidate.
10. **`cargo test` litters the tree when CLAUDETTE_WORKSPACE points at it**
   (W2 attempt 1): the W1 mission's gate run left
   `crates/claudette/claudette-writecode-{test,big}.sh` untracked in the forge
   clone → the W2 mission died on the dirty-tree guard (not a model failure;
   attempt counter NOT consumed). Root cause = the write_file test env leak
   that David's dormant branch `fix/write-file-test-env-leak` (2 commits,
   never PR'd) pins. Every forge mission re-litters the clone until fixed —
   Claude cleans between missions this run. **Recommend: PR that branch.**
