# Claudette v0.8.0 daily-driver eval — 50-task interactive battery

**Date:** 2026-05-29 → 2026-05-30
**Model:** `qwen3.6-35b-a3b@q3_k_xl` (18.6 GB), LM Studio, 32k ctx, RTX 5060 Ti
**Config:** `CLAUDETTE_AUTO_APPROVE=1`, `CLAUDETTE_WORKSPACE=<per-task>`, OpenAI-compat backend
**Question:** Is claudette good enough on the local q3 brain to be an ~80% daily coding driver?

## Verdict: **YES — 88% (44/50) clean pass, 92% (46/50) task-accomplished.** Goal (≥80%) met.

The battery runs every task through the **real interactive one-shot path** (`claudette "<prompt>"`)
against a fresh copy of a per-task fixture, then verifies the outcome **objectively** (build/test
passes, file exists with correct behavior, or the transcript contains ground-truth tokens). No
self-grading by the model.

| Run | Score |
|---|---|
| Baseline (pre-fix, 36 tasks completed) | 26/36 = **72.2%** |
| Post-fix, same 36 tasks | 32/36 = **88.9%** (+16.7 pts) |
| Post-fix, full 50-task battery (clean pass) | 44/50 = **88.0%** |
| Post-fix, full 50 (task-accomplished*) | 46/50 = **92.0%** |

\* adds B4 (file created + test passes, but process didn't exit before the cap) and C6 (a verifier
false-negative — the model's explanation was correct; the verifier regex was too strict — corrected
and re-verified to PASS against the unchanged transcript).

7 tasks flipped FAIL→PASS after the fix: **A1, B1, B2, B5, C1, C5, E2**.

## Coverage (the battery)
- **11 languages/surfaces:** Rust, Python, JS, TypeScript, Go, shell, HTML, CSS, SQL, a large
  real-repo (claudette's own `src`+`docs`), and a git repo.
- **12 task types:** bugfix, add-feature, multi-file edit, refactor/rename, create-file, explain,
  locate-symbol, enumerate-all-X, run-tests/build, debug-from-error-message, git-workflow,
  answer-from-codebase.

### By task type (post-fix, clean pass)
| type | score | | type | score |
|---|---|---|---|---|
| bugfix | 6/6 | | locate | 3/4 |
| add-feature | 5/5 | | enumerate | 2/3 |
| multi-file | 4/4 | | explain | 3/4 (→4/4 corrected) |
| refactor | 4/4 | | create-file | 1/4 (→2/4 w/ B4) |
| run-tests | 4/4 | | git-workflow | 4/4 |
| debug-error | 4/4 | | answer-codebase | 4/4 |

### By language (post-fix, clean pass)
rust 7/7 · ts 4/4 · go 4/4 · sql 3/3 · git 4/4 · html 1/1 · css 1/1 · python 6/7 · js 5/7 · shell 3/4 · bigrepo 6/8

## Root-cause analysis (from `lms log stream --source model --stats`)

The baseline's failures all traced to **two harness gaps**, both fixed:

### Gap 1 — `enable_tools` spiral (the #1 failure source)
Claudette gates its actuation tools (edit/search/run/git) behind an `enable_tools(group)` meta-tool
to keep the base schema tiny (~210 tok). But q3 routinely emits the call with the required `group`
arg **dropped entirely** — `<function=enable_tools></function>` — **415 such errors** in the
baseline capture. With no tools enabled it either gave up and explained (A1) or retried the empty
call until the 150s timeout (B1, B2, B5).

**Fix** (`tool_groups.rs`, `run.rs`, `executor.rs`): workspace-gated pre-enable of a lean **coding
core** — Files + Search + Advanced + Quality (~2.2k tok) — when `CLAUDETTE_WORKSPACE` is set, **plus**
a forgiving `enable_tools` (empty/missing group → enable the coding core) for secretary mode. Result:
`enable_tools: missing` errors dropped to **0**, and tasks got *faster* (no wasted round-trips):
B1 150s-timeout → 42s, B2 160s-timeout → 31s.

### Gap 2 — `glob_search` rooted at `$HOME`, not the workspace
The model called `glob_search("**/stats.py")`; it searched `C:\Users\david\**` and matched 13 decoy
`stats.py` files from old scikit-learn checkouts, read the **wrong file**, and stalled. The fixture
on `D:\` was never in scope. `grep_search` already got workspace-rooting in v0.8.0; `glob_search`
was missed and its sandbox even *hardcoded* `starts_with($HOME)`, rejecting other-drive workspaces.

**Fix** (`tools/search.rs`): glob now uses the same root priority (mission → `CLAUDETTE_WORKSPACE`
root → `$HOME`) and the same `validate_read_path` envelope as grep. Fixed B1, B5.

All fixes shipped with unit tests; `cargo fmt`, `clippy -D warnings`, and the full lib suite
(1053 tests) are green.

## The remaining ~20% (honest triage)

**Model-bound (the "keep Claude Code for this" 20%) — as predicted in the goal:**
- **I1 enumerate** — listed 2/6 `CLAUDETTE_FORGE_*` vars. q3 under-enumerates in a large repo.
- **I3 deep-locate w/ conflicting docs** — asked for the fix-loop default; the source says
  `DEFAULT_MAX_FIX_ROUNDS = 3` but four doc files say `2`. The model trusted the stale docs / timed
  out. This is the exact "deep localization with conflicting docs" weak spot the goal said to stress
  hardest. **Both are genuine model limits, not harness bugs.**

**Fixable follow-up (deferred for review, not a model limit):**
- **C4 / F2 create-file timeouts** — "create a new file" routes through `generate_code` (a *second*
  q3 coder-model pass). The capture shows it generating **correct** code, but q3 over-thinks a
  trivial function (reasoned through `slugify('!!!')` edge cases) and raced past the 150s cap. B4
  (python) won the same race; C4/F2 lost. **Recommended fix:** route simple, signature-known file
  creation to a direct `write_file` instead of the heavyweight codet path (or stream/cap codet
  reasoning). Left for review rather than changed unsupervised.

**Eval-harness note:** C6 was a verifier false-negative (corrected). B4 accomplished its task but the
process didn't terminate before the cap — a real UX wart worth a follow-up (clean-exit-on-done).

## Reproduce
```
bash runs/eval-2026-05-29/battery/run_battery.sh          # all 50
bash runs/eval-2026-05-29/battery/run_battery.sh A        # just the Rust tasks
bash runs/eval-2026-05-29/battery/analyze.sh              # aggregate + breakdown
```
Captures (model reasoning) live in `battery/captures/` — **outside** every task workspace.
