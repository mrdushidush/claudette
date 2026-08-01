# Publication posture — the corpus ships fully open, with a contamination date

**Decision (David, 2026-08-01): publish everything, date the contamination.**
Supersedes the 2026-07-27 held-out split (publish 15 / seal 41), which is recorded below because
its analysis is still useful and its failure is the lesson.

---

## ⚠️ Contamination date: 2026-07-25

The full corpus — all 56 fixtures, **all hidden verifiers, and all reference solutions** — has been
publicly cloneable since the `battery/q50-quality-corpus` branch was pushed on **2026-07-25**.

- Every model in the published table was **released before that date** and therefore could not have
  trained on this corpus. **All 36 runs stand.**
- Any model released **after** 2026-07-25 must be treated as potentially contaminated on these 56
  tasks.
- The date must be reported alongside any number quoted from this corpus.

## How this was decided

The seal was discovered to be already broken on 2026-08-01, while preparing to publish. `origin/main`
is clean (zero Q-task verifiers), but the corpus branch is public and carries everything, including
all 14 tasks of the sealed spine.

Two options were weighed:

1. **Re-seal** — rewrite the branch history to strip `verify/` and `refsol/` for the sealed 41, force-push,
   keep the full corpus private. Rejected: it cannot un-clone anything already taken, a force-push on a
   public branch is itself visible, and it would make every published number rest on files nobody can check.
2. **Go fully open and date it.** Chosen. The measurement results are already collected against
   uncontaminated weights, and a stated contamination date is a stronger scientific artifact than a
   partial seal of unknown integrity.

**The design lesson, for the successor corpus: decide the publication posture before the first push,
not after 36 runs. A public repo is public in every branch.** Tier 2 (`T2.md`) is being built sealed
from the start.

## What survives publication

Everything the campaign actually argues. None of these depend on the tasks staying secret:

- Public leaderboards anti-correlate with this corpus.
- The error bar is a property of the model, not the benchmark.
- Determinism is within-session only.
- The failure list rotates; per-task claims from single runs are unfounded.
- Speed probes mispredict end-to-end speed in both directions.
- A held-constant nobody measures is not held.

What does **not** survive is Q56's ability to rank models released after 2026-07-25. That is a real
cost and is stated as such in the writeup rather than dressed up.

---

# Retained analysis — corpus discrimination

Recomputed 2026-08-01 across **all 34 ranking runs** (16 configurations; the two KV-fp16 champion
runs are excluded as a different config).

- **454 task-failures** in total.
- **51 of 56 tasks defeat at least one run.**
- **Only 5 tasks are never failed by anyone:** Q21, Q31, Q33, Q39, Q55.

The hardest tasks by failure count across those 34 runs:

| task | fails | | task | fails |
|---|---|---|---|---|
| Q03 | 26 | | Q53 | 18 |
| Q52 | 23 | | Q13 | 17 |
| Q51 | 22 | | Q01 | 17 |
| Q46 | 20 | | Q35 | 16 |
| Q25 | 20 | | Q44 | 14 |
| Q05 | 19 | | Q40 | 14 |

⚠️ **This supersedes the earlier "44 of 56 tasks never fail" note, which was champion-only.** It also
supersedes the 2026-07-27 figure of *seven* never-failed tasks: adding the two weakest models
(`gemma-4-e2b`, `qwen3.5-4b`) to the sweep pulled Q02 and Q32 out of that set.

⇒ The corpus is saturated **against the top two gemma configs specifically** (55/56 and 54/56), not
in general. Tier 2 is needed to discriminate at the *top* of the range, not to rescue a corpus that
has stopped working. Most of Q56 is still doing real work.

---

# Superseded — the 2026-07-27 held-out split (kept for the record)

The plan was: publish 15 tasks complete, seal 41 including the high-signal spine (Q01 Q03 Q05 Q13
Q24 Q25 Q26 Q35 Q43 Q44 Q46 Q51 Q52 Q53). The public 15 — Q02 Q11 Q12 Q15 Q20 Q21 Q27 Q30 Q31 Q37
Q39 Q47 Q48 Q50 Q55 — were exactly 3 per language, covered all 9 task types, and carried only 12 of
the then-161 task-failures (7.5%), leaving 92.5% of discriminating signal sealed.

**Its most important finding is still true and still worth stating wherever the corpus is used:**

> ### The easy tasks cannot rank anything
>
> Nine of fourteen configurations scored **15/15** on that public sample, and it **inverted** against
> the full score in three places — `devstral` 15/15 public but 40/56 full, `gemma-4-12b` 12/15 public
> but 42/56 full.
>
> That degeneracy is itself a result: **the top six configurations are indistinguishable on ordinary
> coding tasks.** Only the trap-dense tasks separate them, which is the entire thesis of the corpus.

The objection the split was designed to answer — *"you published only the tasks everyone passes, so I
cannot verify your hard ones are fair"* — is now moot: every task is published, and every task,
published or not, cleared the same test-first gate (`gate_q50.sh`), which requires the verifier to
FAIL on the untouched fixture and PASS on fixture + reference solution. Readers can audit the traps
directly instead of taking the gate's word for it.
