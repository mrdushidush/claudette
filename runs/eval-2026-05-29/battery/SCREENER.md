# SCREEN-10 — the model screener

A cheap, **discriminating** 10-task gate that decides whether a new/unknown local
model earns the full 50-task battery. Run it with:

```bash
bash runs/eval-2026-05-29/battery/run_screener.sh <model-key> <identifier> [ctx]
# ctx defaults to 24576; always loads --parallel 1
```

## Tasks (10)

`A1 A7 C1 C4 D1 F1 H2 H4 I6 I8`

| id | lang | kind | role in the screen |
|----|------|------|--------------------|
| A1 | rust | bugfix | weak-model separator (also the template/connectivity smoke) |
| A7 | rust | debug-error | weak-model separator |
| C1 | js | bugfix | mid/top-tier discriminator |
| C4 | js | create-file | mid/top-tier discriminator |
| D1 | ts | bugfix | weak-model separator |
| F1 | shell | bugfix | weak-model separator |
| H2 | sql | add-feature | weak-model separator |
| H4 | sql | debug-error | mid/top-tier discriminator |
| I6 | bigrepo | enumerate | mid/top-tier discriminator (large-repo nav) |
| I8 | bigrepo | answer-codebase | mid/top-tier discriminator (large-repo nav) |

- **A1/A7/D1/F1/H2** — every historical **≥86%** model passed all five; **granite
  (78%) failed all five**. These separate the weak tier cleanly.
- **C1/C4/H4/I6/I8** — spread the mid/top tier apart.
- **I1/I3 are excluded on purpose.** They fail for nearly *every* model (including
  the champion) → zero discrimination, and they cost ~480 s of timeout budget. The
  literal "hardest 10" also mis-ranks (qwen3.5-9b at 88% would score 4/10 and be
  wrongly rejected while granite at 78% scores 5/10). SCREEN-10 separates the
  ≥86% models from the <80% models perfectly across all 7 clean historical runs.

## Scoring & gate

- **Score = count of status *exactly* `PASS`.** `PASS(TIMEOUT)` does **not** count
  (same rule as core-50 scoring).
- **Gate:**
  - **`≥7`** → run the full 50-task battery.
  - **`6`** → re-run only the failed tasks once (LM-Studio-eviction flake check),
    then re-judge.
  - **`≤5`** → reject; record the screener row only, no full battery.
- Worst-case wall ≈ 27 min (sum of the 10 timeouts); a healthy model is ~5–10 min.

## Back-test (validates the gate)

Historical full runs scored through SCREEN-10:

| model | full-50 | SCREEN-10 | gate ≥7 |
|---|---|---|---|
| q3-35b (v0.8.1) | 92% | 10 | pass ✓ |
| q3-35b (v0.16.0) | 94% | 8 | pass ✓ |
| q4-35b | 88% | 9 | pass ✓ |
| qwen3.5-9b | 88% | 7 | pass ✓ (borderline by design) |
| qwen3.5-4b | 90% | 8 | pass ✓ |
| gpt-oss-20b | 86% | 8 | pass ✓ |
| granite-4.1-8b | 78% | 2 | reject ✓ |
| nemotron-3-nano-4b | ~73% (partial) | ≤5 | reject ✓ |

## Who skips the screener

- **Known-good (≥86%)** models go **straight to the full battery** — the screener
  is for new/unknown models only.
- **Known-bad / impractical** models keep their historical verdicts as table notes
  (no re-run): q4-35b (RAM-spill), qwen3.6-27b (dense/slow), granite-4.1-8b (78%),
  nemotron-3-nano-4b (rejected), nemotron-omni-30b-**reasoning** (too slow @24k).

## Gotchas

- A task failing in **0–1 s** is a load/template/connectivity problem, not a model
  quality result — check `logs-screen-<id>/<id>.log` for HTTP 400 / jinja before
  trusting the score. The screener aborts early (exit 7) if the A1 smoke shows a
  template-incompatible model.
- `FAIL(TIMEOUT)` with an **empty/near-empty** log = LM-Studio eviction flake →
  re-run (`run_battery.sh` already auto-retries EC=124 once). Timeout **with**
  output = a real spiral.
- Confirm `CONTEXT 24576 / PARALLEL 1` in `lms ps` before trusting a run.
