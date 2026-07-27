# The held-out split — decided 2026-07-27

Resolves the contamination blocker that has been open since the writeup draft of 2026-07-25.

**Decision (David, 2026-07-27): publish a sample, hold back the rest.**

Publishing the whole corpus publishes the hidden tests and reference solutions, which destroys
the instrument permanently — any model trained on public data after that date may have seen the
grading key. Publishing nothing makes every number in the writeup an unverifiable claim, which
is the single sharpest objection to a private benchmark. The split takes the middle.

---

## What ships and what stays sealed

**PUBLIC SAMPLE — 15 tasks, complete (fixture + hidden tests + reference solution):**

| task | lang | type | discriminates |
|---|---|---|---|
| Q02 | rust | boundary | 0 |
| Q11 | rust | concurrency | 2 |
| Q12 | rust | perf | 1 |
| Q15 | python | error-handling | 1 |
| Q20 | python | refactor | 2 |
| Q21 | python | multi-file | 0 |
| Q27 | js | bugfix | 1 |
| Q30 | js | api-misuse | 1 |
| Q31 | js | implement-spec | 0 |
| Q37 | ts | boundary | 1 |
| Q39 | ts | api-misuse | 0 |
| Q55 | ts | implement-spec | 0 |
| Q47 | shell | error-handling | 1 |
| Q48 | shell | api-misuse | 1 |
| Q50 | shell | implement-spec | 1 |

"discriminates" = how many of the 14 distinct benchmarked model configs failed that task.

Composition is **exactly 3 tasks per language** (rust/python/js/ts/shell) and covers **all 9 task
types** (implement-spec 3, api-misuse 3, boundary 2, error-handling 2, bugfix, refactor,
multi-file, perf, concurrency 1 each). A reader can therefore see a real fixture, a real hidden
reviewer test and a real reference solution for every language and every category in the corpus.

**SEALED — the remaining 41 tasks**, including the entire high-signal spine: the 14 tasks that
each defeat 4 or more of the 14 benchmarked configs — **Q01 Q03 Q05 Q13 Q24 Q25 Q26 Q35 Q43 Q44
Q46 Q51 Q52 Q53**. Every headline number is quoted from the full 56.

## Cost of the split, measured

Across the 14 distinct model configs there are **161 task-failures** in total. The public 15
carry **12 of them — 7.5%.** The sealed 41 retain **92.5% of all discriminating signal.**

The sample was selected *only* from the low-discrimination bands (0–2 failures), subject to the
language and type coverage constraints above. No task that defeats 3 or more configs is
published.

---

## ⚠️ Two properties of this split that must be stated wherever it is published

### 1. The public sample is a METHOD DEMONSTRATOR, not a leaderboard

Scores on the public 15:

| model | public 15 | sealed 41 | full 56 |
|---|---|---|---|
| gemma-4-26b-a4b-qat | 15/15 | 40/41 | 55 |
| gemma-4-26B-A4B-it | 15/15 | 39/41 | 54 |
| champion 3.06bpw (q8 control) | 15/15 | 35/41 | 50 |
| qwen3.6-35b-a3b UD-IQ4_XS | 15/15 | 35/41 | 50 |
| MTP-GPU-4 / MTP-GPU-3 | 15/15 | 34/41 | 49 |
| champion control (2.27.1) | 15/15 | 33/41 | 48 |
| gemma-4-e4b | 15/15 | 28/41 | 43 |
| qwen3-coder-30b | 14/15 | 29/41 | 43 |
| north-mini-code-1.0 | 13/15 | 29/41 | 42 |
| gemma-4-12b Q8_0 | 12/15 | 30/41 | 42 |
| gemma-4-12b-qat | 15/15 | 26/41 | 41 |
| gpt-oss-20b | 13/15 | 27/41 | 40 |
| devstral-small-2-24b | 15/15 | 25/41 | 40 |
| granite-4.1-8b | 11/15 | 14/41 | 25 |

**Nine of fourteen configs score 15/15.** The public sample cannot rank anything above ~43/56,
and it **inverts** in places: `devstral` scores 15/15 public but 40/56 full, while `gemma-4-12b`
scores 12/15 public but 42/56 full. Same for `gemma-4-12b-qat` (15/15 public, 41/56 full).

⇒ **Never rank models on the public sample.** It exists so the method can be audited and the
harness run end-to-end, not so scores can be compared. This must be said explicitly in the
writeup and in the corpus README, or the first reader to run it will publish a table of 100%s.

The degeneracy is itself a result worth reporting: **the top six configurations are
indistinguishable on ordinary coding tasks.** Only the trap-dense tasks separate them — which is
the whole thesis of the corpus.

### 2. The obvious objection, and the answer

*"You published only the tasks everyone passes, so I cannot verify your hard tasks are fair."*

This is the strongest critique of the design and it is partly correct. The honest answers:

- Every task in the corpus — public and sealed — passed the same **test-first gate**
  (`gate_q50.sh`): the verifier must FAIL on the untouched fixture and PASS on fixture +
  reference solution. Tasks that are accidentally already passing, or impossible, cannot enter.
- Traps are **unstated in the prompt but fair to a reviewer** — degenerate inputs, falsy-vs-absent,
  boundary conditions the spec implies. They are not secret gotchas; the published 15 include
  low-discrimination examples of the same trap classes so the style is inspectable.
- **Escalation available if reviewers press:** publish the *prompts and fixtures* for all 56 while
  still withholding hidden tests and reference solutions for the sealed 41. Since discrimination
  comes from what the prompt deliberately does **not** say, a prompt reveals materially less than
  a grading key. Holding this in reserve rather than shipping it keeps the option open; shipping
  it later is easy, un-publishing is impossible.

---

## ★ Finding this analysis produced: the corpus is far less saturated than believed

The session-7 note concluded the corpus was saturating, on the basis that **44 of 56 tasks never
fail**. That figure is **champion-only**. Across all 14 benchmarked configs:

- **Only 7 tasks are never failed by anyone**: Q02 Q21 Q31 Q32 Q33 Q39 Q55.
- **49 of 56 tasks discriminate at least one model.**

So the saturation problem is narrower than stated: the corpus is saturated *against the top two
gemma configs specifically* (55/56 and 54/56), not in general. This does not cancel the tier-2
plan — a benchmark whose ceiling is 55/56 still needs headroom — but it does change the reason.
Tier 2 is needed to discriminate **at the top of the range**, not to rescue a corpus that has
stopped working. Most of Q56 is still doing real work.

⚠️ It also means the seven zero-discrimination tasks are the only ones that are genuinely free to
publish. Five of them are in the public sample by construction (Q02 Q21 Q31 Q39 Q55); Q32 and Q33
were left sealed only because the language/type quota was already filled.
