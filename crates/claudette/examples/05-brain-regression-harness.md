# 06 — The brain100 regression harness

Claudette ships a 100-prompt regression harness at
`tests/brain100_test.sh` that exercises real tool calls across five
tiers of increasing complexity. Run it before you pin a new brain
model, before a release, or after tuning any prompt.

## What it tests

100 prompts across 5 tiers:

| Tier | Range | What it exercises |
|------|-------|-------------------|
| 1 | 1-20 | Basic tool calling (time, notes, todos, simple file ops, no-tool Q&A) |
| 2 | 21-40 | Parameter passing (correct args to multi-param tools, path handling) |
| 3 | 41-60 | Multi-step reasoning (read-then-summarise, search-then-act) |
| 4 | 61-80 | Edge cases (ambiguous prompts, error recovery, odd units) |
| 5 | 81-100 | Complex scenarios (spawn_agent delegation, long chains) |

Each prompt has an expected regex pattern and an expected tool name.
Pass = (exit 0) AND (non-empty output) AND (output matches pattern).

## Running it

```bash
bash tests/brain100_test.sh
```

That runs against the default `qwen3.5:4b` (the Auto-preset brain) and
writes per-prompt captures to `tests/results_brain100/`. Takes about
**25-30 minutes** on a 3060 Ti.

Pass a different model:

```bash
bash tests/brain100_test.sh qwen3.5:9b
bash tests/brain100_test.sh qwen3.5:9b tests/results_9b
```

The harness skips prompts that don't match the pattern — it doesn't
stop on failure. You get the full 100-prompt score every time.

## Reading the output

The terminal summary looks like:

```
==============================================
  RESULTS: qwen3.5:4b
==============================================

  Total:  100
  Pass:   94
  Fail:   6
  Score:  94%

  Per-tier breakdown:
    T1: Basic (1-20)          19/20  (95%)
    T2: Params (21-40)        19/20  (95%)
    T3: Multi-step (41-60)    17/20  (85%)
    T4: Edge cases (61-80)    19/20  (95%)
    T5: Complex (81-100)      20/20  (100%)

  Wall time: 1631s
```

That is an actual run from the 2026-04-19 regression suite on
`qwen3.5:4b`. Interpretation:

- **94% overall** — solid for a 3.4 GB model. Most real Claudette
  usage hits T1-T2; T3 is the first tier that stresses multi-step
  reasoning.
- **T5 at 100%** — T5 prompts delegate to sub-agents (researcher,
  gitops, reviewer), which use the same brain but in a narrower
  context. Delegation keeps complex tasks from overflowing the 4b
  brain's working memory.
- **Wall time 1631s = 27 minutes** — about 16s per prompt average.
  `qwen3.5:9b` roughly doubles that.

## Per-prompt drill-down

Each prompt produces `tests/results_brain100/NNN.txt` with:

```
NUM: 13
PROMPT: What files are in D:/dev/claudette/src?
MODEL: qwen3.5:4b
EXPECTED_PATTERN: agents|tools|run
EXPECTED_TOOL: list_dir
EXIT_CODE: 0
ELAPSED_MS: 9250
---OUTPUT---
Here are the files in `D:/dev/claudette/src`:

**Rust source files:**
- agents.rs
...
```

Failures land in `tests/results_brain100/failures.txt` with the
prompt, expected pattern, exit code, and a 3-line output preview.
Useful for spotting regressions at a glance.

## Adding your own prompts

`tests/brain100_prompts.txt` is pipe-delimited: `NUM|||PROMPT|||PATTERN|||TOOL`.
`PATTERN` is a case-insensitive regex (grep `-iE`) matched against
stdout+stderr. `TOOL` is the expected tool name, shown only for
documentation — it doesn't affect scoring.

Append a new tier with `TIER=0` adjusted; the harness auto-computes
tier buckets from the NUM field (1-20=T1, 21-40=T2, etc).

## Comparing models

Run the harness with each model, then diff:

```bash
bash tests/brain100_test.sh qwen3.5:4b  tests/results_4b
bash tests/brain100_test.sh qwen3.5:9b  tests/results_9b

diff <(tail -20 tests/results_4b/summary.txt) <(tail -20 tests/results_9b/summary.txt)
```

Expected: 9b wins T3-T5 by 2-5 prompts, 4b wins on wall time by 2x.
The Auto-preset's 4b→9b fallback on stuck signals is designed for
exactly this trade-off — fast by default, escalate only when needed.

## In CI

The harness is intentionally NOT in the GitHub Actions CI workflow —
it's slow (27+ minutes) and non-deterministic (model temperature,
network for `web_search`). Run it locally before releases.

## Honest positioning

Brain100 pass rate is **not the same** as SWE-bench. The harness is
pattern-matching on short-horizon tool-using prompts; SWE-bench is
real task resolution on real repos. Claudette's 94% here doesn't
translate to a 94% SWE-bench number (and we haven't run SWE-bench
yet). See [`../docs/comparison.md`](../docs/comparison.md) for how
Claudette positions against SWE-agent tooling.
