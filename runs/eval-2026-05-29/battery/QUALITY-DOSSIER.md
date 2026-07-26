# QUALITY-DOSSIER — the hunt for a better-quality local brain

Campaign: `launch-drafts/goal_quality_champion_2026_07_21.md`. Battery: **Q50** (56 tasks,
`Q50.md`), hidden-verification, run via `BATTERY_MANIFEST=manifest-q50.tsv`.
Box: RTX 5060 Ti 16 GB, 32 GB RAM, Windows 11. Started 2026-07-21.

## Champion baseline (the bar to beat)

Speed champion / daily driver: **`qwen3.6-35b-a3b-mtp`** (byteshape MTP-GPU-2, 3.06 bpw,
13.6 GB, VRAM-resident) @ ctx 32768 / KV q8 / parallel 1.

| run | tag | score | fails |
|-----|-----|-------|-------|
| 1 (50-task) | q50-champ-bs306 | 46/50 | pre-hardening |
| 2 (50-task) | q50-champ-bs306-hard1 | 47/50 | +batch-1 edges (score went UP → edges futile) |
| 3 (56-task) | q50-champ-full56 | 49/56 | Q03 Q05 Q25 Q45 Q50 Q51 Q52 |
| 4 (56-task) | full56-v2 | 50/56 | Q03 Q05 Q13 Q25 Q51 Q52 |
| 5 (56-task) | full56-v3 | 50/56 | Q03 Q05 Q13 Q25 Q51 Q52 |

### ★ FROZEN BASELINE (2026-07-22): champion = **50/56** (median of 49/50/50; aggregate variance ±0.5)
⚠️ **SUPERSEDED 2026-07-25 — read "THE REAL FINDING" below before using this paragraph.** With
6 runs instead of 3, **only Q03 fails every time**; Q05/Q25/Q51/Q52 are 5/6, not 5-for-5, and two
first-time failures (Q26, Q36) appeared. The aggregate 50/56 holds, but **the "stable
discriminating signal" below was a 3-sample artifact.** Do not screen on these five.

~~**Consistent failures (3/3 runs) — the stable discriminating signal:**~~ Q03 (people==0 guard),
Q05 (blank-line skip), Q25 (integer-dollar price), Q51 (whitespace in duration), Q52
(capacity-0 ring buffer). Flaky: Q13 (2/3), Q45/Q50 (1/3). The hard tail bit on 2/6 (Q51,Q52);
the champion PASSED the genuinely-hard Q53 multi-line-CSV, Q54 topo-sort, Q55 merge-intervals,
Q56 version-compare. Headroom to beat: 6 points (50→56). ⚠️ The old "a model fixing the 5
consistent failures → ~55/56 = +5, a clear crown" reasoning **does not hold** — those five are
not reliably failing, so "fixing" them is partly just winning coin flips. **Judge a crown on the
aggregate full-56 score across ≥2 runs, not on which tasks got fixed.**

**Key finding:** the byteshape champion is an *exceptionally strong code-quality model*, not
just fast — ~87.5% on a fair, trap-dense battery, near the local-model ceiling. Fair
edge-addition can't move it (proven). Discrimination comes from its ~4 consistent failures.

## Crown rule (reframed for a near-ceiling champion)

A quality champion must **(a)** score above the champion's median by **≥ +4** (clear the ±3
brain100 noise band, measured as median-of-≥2-runs), **AND (b)** fix a MAJORITY of the
champion's *consistent* failures (Q03/Q05/Q51/Q52) — i.e. be better on the specific quality
dimensions the champion misses, not just noise. If nothing clears both, **champion holds both
crowns** — a valid, and given the champion's strength, likely result. Speed breaks no ties;
the speed champion stays the daily driver regardless (winner = on-demand second brain).

## Census (ordered by expected quality-gain per download-GB)

RAM 32 GB caps candidates at ~40 GB weights+KV (VRAM 16 + RAM ~24 usable) → 120B-class is
OUT; the 24–35B-A3B MoE band is the sweet spot (where the champion already lives).

### On disk — FREE, test first
| candidate | size | why it might beat the champion | risk |
|-----------|------|-------------------------------|------|
| `qwen3.6-35b-a3b@iq4_xs` | 19.5 GB | higher bpw (~4.25) of the SAME model → less quant loss | spills a little; IQ dequant slow on CPU-expert path |
| `qwen3-coder-30b-a3b-instruct` | 17.7 GB | 2026's top-rated local coder (50.3% SWE-bench) | different lineage; template health unknown → A1 gate |
| `nvidia-nemotron-3-nano-omni-30b-a3b-reasoning` | 24.9 GB | reasoning MoE, may catch unstated traps | slow (reasoning); template maturity unknown |

### Downloads — the byteshape MTP ladder up (keeps MTP speed + higher quality)
| candidate | ~size | note |
|-----------|-------|------|
| byteshape MTP-GPU-3 (~3.53 bpw) | 15.7 GB | fits VRAM; cheapest quality bump, never batteried |
| byteshape MTP-GPU-4 (~3.97 bpw) | 17.6 GB | slight spill |
| byteshape MTP-GPU-5 (~4.19 bpw) | 18.6 GB | spill; byteshape's 24GB main rec |
| unsloth UD Q5_K / Q6_K of qwen3.6-35b-a3b | ~24–29 GB | max-quality non-MTP; spills, no MTP speed |
| Devstral-Small-24B / dense Qwen3.6-27B @ Q6 | ~18–22 GB | different lineages; Devstral = agent-tuned |

## Hunt protocol (per candidate)

Load (LMS bare id, confirm ctx 32768/parallel 1 in `lms ps`) → **A1 smoke gate** (template
health) → speed probe (`probe_speed.sh`, must clear ~15 tok/s) → full 56 via
`BATTERY_TAG=q50-<candidate>` → checkpoint: score + wall + which champion-failures it fixed.
Kill-fast: median >15 min/task or RAM thrash → abort, keep partial as a note.
Downloads are the thermal spike — watch ≤71 °C (David 2026-07-21); prefer download-then-run.

## Decision matrix (filled as candidates run)

⚠️ **KV cache type is a REQUIRED field in every row from 2026-07-23 onward.** It was not
recorded before, and it turned out to be an uncontrolled variable that moves scores (see
"The KV-cache discovery" below). A row without a KV value is not comparable to one with it.

| config | KV | ctx | Q56 PASS | fixes champ-fails? | gen tok/s | spill | template | verdict |
|--------|----|-----|----------|--------------------|-----------|-------|----------|---------|
| champ bs-3.06 (MTP) | **q8_0** | 32768 | **50/56** (median 49/50/50) | — (baseline) | ~70 | none | ok | frozen baseline |
| `qwen3.6-35b-a3b@iq4_xs` | q8_0 | 32768 | **50/56** | 2/5 (Q05,Q52 ✓; Q03/Q25/Q51 ✗) | ~29 | slight | ok | **NO — ties baseline, +0; regresses Q14/Q49; slower** |
| `qwen3.6-35b-a3b-mtp@iq4_xs` (MTP-GPU-3) | q8_0 | 32768 | **49/56** | 0/5 | ~40 | none | ok | **NO — −1 and regresses Q01/Q53; screen 0/5** |
| champ bs-3.06 @ fp16 KV | **f16** | 65536 | **50/56** (r1, 58m49s) | 1/5 | 73 probe / **2.79× slower e2e** | **96.7% VRAM** | ok | **NO — +0 and 2.8× wall-clock** |
| champ bs-3.06 @ fp16 KV | **f16** | 32768 | **50/56** (r2, 26m15s) | 1/5 | **1.24× slower e2e** | 92% VRAM | ok | **NO — +0, still slower, Q46 regressed 2/2** |
| _candidates…_ | | | | | | | | |

### IQ4_XS result (full-56, 2026-07-22, tag `q50-iq4xs`, ctx 32768)
50/56 PASS in ~53 min wall. FAILS: Q03 Q13 Q14 Q25 Q49 Q51. Same aggregate as the
champion median but a *different profile*: it FIXED Q05 (blank-line skip) and Q52
(capacity-0 ring buffer), yet REGRESSED on Q14 (deep_merge bugfix — champion passes),
Q49 (multi-file shell — champion passes), and Q13 (champion-flaky). Net +0 vs baseline,
fixes only 2/5 consistent fails → **fails the crown rule on both axes**, and it is ~2.4×
slower (29 vs 70 tok/s) with a slight RAM spill. Not a crown candidate — a strictly worse
trade than the champion. *Validation note:* IQ4_XS cleared exactly the ≥2/5 screen gate,
and the full run confirmed the screen threshold is calibrated right (a marginal pass that
the full run legitimately rejects).

### MTP-GPU-3 result (full-56, 2026-07-22/23, tag `q50-mtpgpu3`) — REJECTED *in the q8 config*

`byteshape/Qwen3.6-35B-A3B-MTP-GGUF` at ~3.53 bpw (`qwen3.6-35b-a3b-mtp@iq4_xs`, 15.67 GB) —
one rung up the ladder from the champion (MTP-GPU-2, 3.06 bpw). Tested in two configs:

- **First load, fp16 KV, ctx 32768:** A1 smoke PASS but only **24.6 tok/s** (13730 MiB — the
  weights spill at this bpw). The screen was interrupted partway; the partial recorded
  **Q03 FAIL, Q05 PASS**.
- **Re-loaded with KV q8** (David): **39.9 tok/s** (14891 MiB) — but quality cratered.
  Screen **0/5**. Promoted to a full 56 anyway to characterise it: **49/56**, fails
  **Q01 Q03 Q05 Q25 Q51 Q52 Q53**. It **regressed Q01 and Q53**, both of which the champion
  passes; Q53 (multi-line CSV) is one of the genuinely hard tail tasks.

**Verdict: −1 vs baseline and two regressions → fails the crown rule decisively in that config.**

⚠️ **This rejection is config-scoped, not model-scoped.** The two loads differed by more than
the KV type (VRAM went 13730 → 14891 MiB, so the offload split moved too), and the one fp16
data point we have is a **Q05 PASS** — the exact task the q8 run failed. Per the runbook,
**MTP-GPU-3 must be re-screened at fp16 KV** before it is written off as a model.

### The KV-cache discovery (2026-07-23) — an uncontrolled variable in every prior row

David's insight: **the frozen 50/56 baseline was measured with KV cache q8**, and nothing in
the harness recorded that. Re-loading the *same champion weights* at **fp16 KV** (bare id,
ctx 65536, 15556 MiB) gave:

- **73.02 tok/s median** — vs ~70 at q8. **fp16 KV costs zero speed and ~140 MiB here.**
  The usual reason to quantise the KV cache simply does not apply on this model at this size.
- **Screen 2/5: Q05 and Q52 now PASS** (tag `q50-champ-kvfp16-screen`). Each had failed
  **0/3** across all three q8 baseline runs — so this is not flakiness in the noise band.
  Q03/Q25/Q51 still fail.

Two independent observations point the same way:

1. **UD-IQ4_XS fixed exactly Q05 + Q52** and nothing else — the same pair, from a *different*
   weight quant. If higher weight bpw were the cause, the fixed set should differ.
2. **MTP-GPU-3 collapsed under q8** (0/5 screen, regressing Q01/Q53) while its single fp16
   data point passed Q05.

The parsimonious reading is that **Q05/Q52 are sensitive to KV-cache precision, not to weight
bpw** — which means the IQ4_XS "lift" we attributed to weights on 2026-07-22 was probably never
about weights at all. It also means every candidate screened at q8 was measured with a handicap
the champion's own baseline shared, so *relative* rankings are probably intact, but the
**absolute** frozen number may be an underestimate.

⚠️ **Operational trap.** The KV type is **sticky, global, and not an `lms load` flag.** It lives
in the per-model default-config JSON and survives unloads — a bare `lms load` or a claudette JIT
load silently inherits it:

```
C:\Users\david\.lmstudio\.internal\user-concrete-model-default-config\
  byteshape\Qwen3.6-35B-A3B-MTP-GGUF\Qwen3.6-35B-A3B-IQ3_S-3.06bpw.gguf.json
```
keys `llm.load.llama.kCacheQuantizationType` / `vCacheQuantizationType`.

Consequence: **the frozen q8 baseline is not reproducible without editing that file back to
`q8_0`.** Verify it before and after every run and record the value in the row.

### fp16-KV full-56 run 1 (2026-07-25, tag `q50-champ-kvfp16-r1`, KV f16 / ctx 65536)

**50/56 in 58 min 49 s.** FAILS: **Q03 Q19 Q25 Q46 Q51 Q52**.

Against the frozen q8/32768 baseline (median 50/56; consistent fails Q03 Q05 Q25 Q51 Q52):

| | baseline (3 runs) | r1 | |
|---|---|---|---|
| Q05 | FAIL FAIL FAIL | **PASS** | ✅ fixed — the screen replicated |
| Q52 | FAIL FAIL FAIL | FAIL | ❌ **screen did NOT replicate** |
| Q19 | PASS PASS PASS | **FAIL** | ⚠️ new regression |
| Q46 | PASS PASS PASS | **FAIL** | ⚠️ new regression |

**Verdict on r1: does not clear the adoption bar.** The rule was "≥ 50 with no new
regressions". It hits 50 — but that is **+0**, it fixes only **1 of 5** consistent failures,
and it **breaks two tasks the champion passed 3/3**. This is the *exact* pattern the runbook
predicted from a 2/5 screen: UD-IQ4_XS cleared the same screen, then tied at 50/56 while
regressing two tasks. **A 2/5 screen is still not evidence of a lift.**

**Both regressions are genuine quality misses, not timeouts** (checked, because a 2.5× slowdown
makes timeouts a real alternative explanation):
- **Q19** (143 s, EC=1): unit ladder stops at TB — `human_readable_size(1125899906842624)`
  should be `"1.0 PB"`.
- **Q46** (99 s, EC=0): `range 5..3` returned `[5]`, expected `[]` — the degenerate start>end guard.

⚠️ **But note what they have in common: both are hardening batch-1 edge tasks**, the trap edges
added specifically to sit at the champion's competence boundary. Q50.md records that the
batch-1 probe found the champion missed exactly two of the sixteen — **Q25 and Q46**. So Q46 is
a known-borderline task that the three baseline runs happened to pass 3/3, and Q19 is the same
class of edge. These are real misses; whether they are a *systematic* regression or borderline
tasks resolving the other way needs another run to separate. Do not treat r1 alone as proof
that fp16 KV breaks Q19/Q46.

#### ⚠️ The bigger finding: "fp16 KV costs zero speed" is **wrong end-to-end**

The 2026-07-23 speed probe said 73.02 vs ~70 tok/s and we concluded fp16 was free. The full
run says otherwise:

| | q8 / 32768 (v3) | fp16 / 65536 (r1) | ratio |
|---|---|---|---|
| all 56 tasks | 1267 s (21.1 min) | 3529 s (58.8 min) | **2.79×** |
| excluding Q01 cold load | 1252 s | 3292 s | **2.63×** |
| **51 tasks with IDENTICAL verdicts** | **1164 s** | **2885 s** | **2.48×** |

Restricting to tasks where both configs reached the same PASS/FAIL verdict controls for "the
model did more work", and it is still **2.48× slower**. Worst same-verdict cases: Q09
11 s → 290 s, Q50 17 s → 133 s, Q43 17 s → 113 s.

**`probe_speed.sh` measured the wrong thing for this decision.** It times raw generation on
short prompts; the battery is dominated by prompt processing over a large KV cache. A config
can be neutral on tok/s and 2.5× worse in agent wall-clock. *Speed probes are not a substitute
for a timed full run.*

**Likely mechanism — VRAM pressure, not KV precision.** r1 sat at **15772 / 16311 MiB = 96.7%**,
about 539 MiB of headroom. fp16 doubles KV bytes/token *and* ctx doubled 32768 → 65536, so KV
allocation is ~4× the baseline's. At that occupancy any transient allocation spills.

⚠️ **r1 moved TWO variables at once** (KV type *and* ctx), so it cannot attribute the slowdown
to KV precision. That is what run 2 isolates: **fp16 KV at ctx 32768**, a single-variable
comparison against the frozen baseline.

### fp16-KV run 2 — the isolation run (2026-07-25, tag `q50-champ-kvfp16-32k-r2`, KV f16 / **ctx 32768**)

r1 moved two variables at once, so r2 held ctx at the baseline's 32768 and changed **only** the
KV type — a true single-variable comparison. Loaded explicitly (`lms load -c 32768`), verified
`f16`/`f16` before starting.

**50/56 in 26 min 15 s.** FAILS: Q03 Q05 Q13 Q25 Q46 Q52.

#### ✅ The speed penalty was ctx, not KV precision

| config | wall | vs baseline | VRAM |
|---|---|---|---|
| q8 / 32768 (v3 baseline) | 21 m 07 s | — | ~92% |
| **f16 / 32768** (r2) | **26 m 15 s** | **1.24×** | 15080 MiB (92%) |
| f16 / 65536 (r1) | 58 m 49 s | **2.79×** | 15772 MiB (**96.7%**) |

Same three opening tasks: Q01 15 s → 237 s → **10 s**, Q02 15 s → 46 s → **14 s**. At ctx 32768
fp16 is back at baseline speed. **The 2.8× penalty came from ctx 65536 pushing VRAM to 96.7%,
not from fp16 KV.** fp16 KV itself costs a modest ~24%.

⚠️ **Practical consequence: David has been daily-driving fp16 @ ctx 65536 since 2026-07-23** —
i.e. the slow corner of this table. Reverting the KV type restores the fast 64k config.

#### ❌ The KV-cache hypothesis is REFUTED

The 2026-07-23 screen scored 2/5 on **Q05 + Q52** and we read it as a KV-precision effect.
**Neither replicated.** Q52 failed both full fp16 runs; Q05 passed one and failed the other.
The screen was measuring **run-to-run nondeterminism**, not KV precision.

The full 6-run stability table — which shows the fail set rotates and only Q03 is constant — is
in "THE REAL FINDING" section below, after the q8 control run.

#### ⚠️ This also breaks the screen gate itself — see the 6-run stability table below

### q8 control run (2026-07-25, tag `q50-champ-q8-control-r3`, KV q8_0 / ctx 32768)

Run after reverting the KV config, in the **exact frozen-baseline config**, to prove the box had
not drifted underneath tonight's numbers.

**50/56 in 23 min 13 s.** FAILS: Q03 Q05 Q13 **Q26 Q36** Q51.

✅ **Score reproduces the frozen baseline exactly — no drift**, and the revert is confirmed live.
✅ Clean same-night speed comparison at matched ctx: **q8 23m13s vs f16 26m15s = f16 is ~1.13×
slower.** (The 1.24× against v3's 21m07s compared across different nights; this is the better number.)

### ★★ THE REAL FINDING: only **Q03** is a consistent failure — the fail set rotates

The control **passed Q25 and Q52**, two tasks that had failed in all five previous runs. That
forced a recount over all six full runs (3× q8 baseline, 2× fp16, 1× q8 control):

| task | v1 | v2 | v3 | r1 f16/64k | r2 f16/32k | r3 q8 ctrl | fails |
|---|---|---|---|---|---|---|---|
| **Q03** | F | F | F | F | F | F | **6/6** |
| Q05 | F | F | F | · | F | F | 5/6 |
| Q25 | F | F | F | F | F | · | 5/6 |
| Q51 | F | F | F | F | · | F | 5/6 |
| Q52 | F | F | F | F | F | · | 5/6 |
| Q13 | · | F | F | · | F | F | 4/6 |
| Q46 | · | · | · | F | F | · | 2/6 |
| Q19 | · | · | · | F | · | · | 1/6 |
| Q26 | · | · | · | · | · | F | 1/6 |
| Q36 | · | · | · | · | · | F | 1/6 |
| Q45 | F | · | · | · | · | · | 1/6 |
| Q50 | F | · | · | · | · | · | 1/6 |

**Scores: 49, 50, 50, 50, 50, 50.** The aggregate is rock-stable (±0.5). The *identity* of the
failing tasks is not: **12 distinct tasks took turns failing, 44 never failed once.**

That is the signature of a model at a stable competence level with a **pool of borderline tasks
resolving stochastically** — not of a fixed set of "things the champion can't do".

#### ⚠️ CORRECTION to the earlier entry in this file

Earlier tonight (after r1/r2 only) this dossier said the consistent set was **Q03/Q25/Q52** and
recommended screening on those three at 3/3. **The control refutes that** — Q25 and Q52 both
passed. Only Q03 survives all six runs, and a one-task screen has no discriminating power.

#### The screen gate should be ABANDONED, not re-tuned

`manifest-q50-champfails.tsv` assumed a stable champion-failure set. There isn't one. Any
small-subset screen inherits the per-task noise, which is why the gate misfired **twice** —
UD-IQ4_XS and fp16 KV each cleared "≥2/5" and then tied at 50/56.

**Use instead:** the **12 tasks that ever failed** (Q03 Q05 Q13 Q19 Q25 Q26 Q36 Q45 Q46 Q50 Q51 Q52),
scored **in aggregate**, not as named must-fixes. The other 44 are saturated for this champion and
buy nothing but wall-clock — so this is a ~4× cheaper screen that keeps essentially all the signal.
Champion reference on those 12: **6/12** (v3, r3). Promote a candidate to the full 56 only if it
clears that aggregate by a margin, and **judge the crown on the full 56 score, never on task identity.**
⚠️ Keep the full 56 for genuinely weaker models — the 44 saturated tasks still discriminate *those*.

### ★ VERDICT: fp16 KV REJECTED — revert to `q8_0`

- **+0 quality.** 50/56 in both runs, identical to the frozen q8 median. No lift at either ctx.
- **Slower.** 1.24× at matched ctx; 2.79× at the ctx it was actually being run at.
- **One possible regression** (Q46, 0/2 at fp16 vs 3/3 at q8) and no compensating gain.
- The evidence that motivated the whole experiment was a noisy screen.

The adoption rule was "≥ 50 **with no new regressions** → adopt". It ties rather than beats, it is
slower, and Q46 went the wrong way twice. **Revert `kCacheQuantizationType`/`vCacheQuantizationType`
to `q8_0`**, which also restores reproducibility of the frozen baseline.

**Knock-on: MTP-GPU-3 does NOT need an fp16 re-screen.** That re-run was owed only because q8 was
believed to suppress Q05/Q52. It does not. MTP-GPU-3's q8 rejection (49/56, regressed Q01+Q53)
stands on its own. **Saves a GPU night.**

## Hunt protocol v2 (2026-07-22, David) — screen-first, one model at a time, 2 h apart

Per-candidate, to save tokens and thermal budget:
1. **Pull ONE model** (download-then-run; watch NVMe ≤71 °C).
2. Load (LMS bare id, confirm ctx 32768 / parallel 1 in `lms ps`), A1 smoke gate, speed probe (≥15 tok/s).
   ⚠️ **The ≥15 tok/s gate is a FLOOR, not a speed measurement.** `probe_speed.sh` times raw
   generation on short prompts. On 2026-07-25 a config that probed *faster* than the champion
   (73 vs 70 tok/s) turned out **2.48× slower end-to-end** on the battery, because the battery is
   dominated by prompt processing over the KV cache. Use the probe to reject slow models; never
   cite it as evidence a config is fast. **Only a timed full run measures speed.**
   Record wall-clock total for every full run so this is comparable across rows.
3. **Screen on the 12 ever-failed tasks, scored in aggregate**
   (`manifest-q50-everfailed.tsv` = Q03 Q05 Q13 Q19 Q25 Q26 Q36 Q45 Q46 Q50 Q51 Q52).
   ⚠️ Superseded `manifest-q50-champfails.tsv` (the 5-task screen) — kept on disk only so old
   rows stay reproducible. **Do not screen on it; it misfired twice.**
4. **Gate: ≥9/12 → promote to the full 56. ≤8/12 → reject, record, move on.**
5. **Wait ~2 h between battery runs.** One candidate in flight at a time.

Candidate queue (coder-30b DROPPED per David — "will suck vs champion"):
byteshape MTP-GPU-3 (3.53 bpw) → MTP-GPU-4 (3.97) → MTP-GPU-5 (4.19), cheapest quality bump first.

## Status (2026-07-25)

Battery FROZEN at 56 tasks; champion median baseline **50/56 @ KV q8 / ctx 32768**.

Candidates resolved so far — **both rejected**:
- UD-IQ4_XS (on-disk): ties at 50/56, +0, regresses Q14/Q49, 2.4× slower.
- MTP-GPU-3 (3.53 bpw): 49/56 in the q8 config, regresses Q01/Q53, screen 0/5.
  ⚠️ *config-scoped rejection* — owed a re-screen at fp16 KV.

**fp16 KV: TESTED ×2 and REJECTED 2026-07-25.** 50/56 at ctx 65536 (58m49s) and 50/56 at ctx
32768 (26m15s) — **+0 vs the q8 median, and slower in both** (1.24× at matched ctx, 2.79× at
65536). The 2/5 screen that motivated it did not replicate: Q52 failed both full runs, Q05
passed one. **`kCacheQuantizationType`/`vCacheQuantizationType` reverted to `q8_0`** (backup:
`…gguf.json.bak-fp16-2026-07-25`), which also restores reproducibility of the frozen baseline.

**MTP-GPU-3 fp16 re-screen: CANCELLED.** It was owed only under the theory that q8 suppresses
Q05/Q52. That theory is dead, so its q8 rejection stands. Saves a GPU night.

**Method fixes banked tonight:**
- `probe_runtime_config.sh` + `RUNMETA.tsv` — the held-constant fields (KV, ctx, parallel, VRAM,
  claudette version **and binary mtime**) are now recorded per run instead of assumed.
- **Subset screening is retired.** There is no stable champion-failure set — over 6 runs only
  **Q03** fails every time, while 12 tasks rotate through the failure slots at a rock-stable
  50/56. Screen on the **12 ever-failed tasks in aggregate** (champion ref 6/12), decide on the
  full 56, and never on task identity. See Q50.md.
- Speed probes do not measure end-to-end speed; only a timed full run does.

## The corrected screen — `manifest-q50-everfailed.tsv` (built 2026-07-25)

The 12 tasks that failed at least once across the champion's six full-56 runs. Scored **in
aggregate**; task identity is explicitly NOT part of the gate.

**Champion reference, recomputed from the six frozen SCORES files:**

| run | screen | that run's failures |
|---|---|---|
| `champ-full56` | 5/12 | Q03 Q05 Q25 Q45 Q50 Q51 Q52 |
| `champ-full56-v2` | 6/12 | Q03 Q05 Q13 Q25 Q51 Q52 |
| `champ-full56-v3` | 6/12 | Q03 Q05 Q13 Q25 Q51 Q52 |
| `champ-kvfp16-r1` | 6/12 | Q03 Q19 Q25 Q46 Q51 Q52 |
| `champ-kvfp16-32k-r2` | 6/12 | Q03 Q05 Q13 Q25 Q46 Q52 |
| `champ-q8-control-r3` | 6/12 | Q03 Q05 Q13 Q26 Q36 Q51 |

**Champion = 6/12 median, range 5–6.** Note how well this vindicates the aggregate rule: no two
runs share a failure list, yet the count moves by at most 1. The old 5-task screen was reading
the rotation as signal.

**Why the gate is ≥9/12 — this is a bound, not a preference.** The champion passes **44/44** of
the tasks outside this set in every run, so a challenger's full-56 score is
`screen12 + (at most 44)`. To clear the crown rule (≥ 54 = 50 + 4) it therefore needs
**screen12 ≥ 10 with zero regressions on the other 44**. Anything scoring ≤9 on the screen
*cannot* win the crown even with a flawless remainder. The gate sits one point below that hard
bound (≥9) purely to absorb the ±1 run-to-run noise measured above.

Cost: 12 of 56 tasks, and the screen is ~4× cheaper than a full run. It is a **reject-fast
filter, not the verdict** — the crown is still decided on full-56 across ≥2 runs.

## MTP-GPU-4 (3.97 bpw) — SCREENED AND REJECTED 2026-07-25

Tag `q50-mtpgpu4-screen`. **7/12 on the corrected screen vs a ≥9/12 gate → rejected, no full 56.**

| field | value |
|---|---|
| model key | `qwen3.6-35b-a3b-mtp-gpu4` (own repo dir — see the naming hazard below) |
| config | KV q8_0 / ctx 32768 / parallel 1, **measured** |
| VRAM | 15,611 MiB of 16,311 after the expert-split tweak (13,923 before) |
| probe | 21.51 tok/s untweaked → **37.40 tok/s** tweaked (+74%) |
| screen | **7/12**, 12m28s, no timeouts (slowest task 214 s of a 600 s ceiling) |
| passes | Q05 Q19 Q26 Q36 Q45 Q46 Q50 |
| fails | Q03 Q13 Q25 Q51 Q52 |

**Why 7 is a reject and not a near-miss.** The champion sits at 6/12 median (5–6 range), so this
is +1 — inside the rotation noise. And the bound bites: max full-56 = `7 + 44` = **51**, i.e. +1
over the champion's 50, against a crown rule needing ≥54. Even granting a lucky +1 flip to 8/12
it cannot reach the crown. A full-56 night here would have bought a number we can already
bracket.

**The +1 is churn, not a lift.** Against the q8 control it *fixed* Q05/Q26/Q36 and *broke*
Q25/Q52 — five changes to net one point. That is the same rotation signature the champion
produces against itself, which is precisely what the aggregate screen was built to see through.

**And it costs 2× wall-clock**: 748 s on these 12 tasks vs the champion's 372 s (control r3) /
314 s (v3). It also needs 15.6 GB of a 16.3 GB card, so it cannot coexist with David's ctx-65536
daily-driver config.

### ★ The real finding: bpw is not the axis

Four points on the same model family, all at KV q8 / ctx 32768:

| variant | bpw | result |
|---|---|---|
| champion MTP-GPU-2 | 3.06 | 50/56 · **6/12** screen |
| MTP-GPU-3 | 3.53 | 49/56 (regressed Q01/Q53) |
| **MTP-GPU-4** | **3.97** | **7/12 screen** |
| UD-IQ4_XS | 4.25 | 50/56 (regressed Q14/Q49) |

**Quality is flat within noise from 3.06 → 4.25 bpw while speed varies ~3×.** Every rung of the
ladder has now returned "ties the champion, slower". The champion is the *lowest* bpw of the four
and still the best deal.

**▶ RECOMMENDATION: drop MTP-GPU-5 (4.19 bpw) from the queue.** It interpolates between two
measured-flat points (3.97 and 4.25) and would cost a download plus a GPU night to re-measure
noise. The quantisation ladder is exhausted; if the hunt continues it should move to a
**different model family**, not a higher bpw of this one.

### ⚠️ Naming hazard — byteshape ships three files all called `IQ4_XS`

3.53, 3.97 and 4.19 bpw share the quant name, and LM Studio derives its model key from it. Left
in the default directory, MTP-GPU-4 would have answered to `qwen3.6-35b-a3b-mtp@iq4_xs` — the
same key as the already-rejected MTP-GPU-3. It was downloaded into its own repo dir
(`Qwen3.6-35B-A3B-MTP-GPU4-GGUF`) to force a distinct key. **Do the same for MTP-GPU-5 if it is
ever pulled.**

Note also that a new repo dir gets a *fresh* per-model config, so KV does **not** inherit and
defaults silently. The config was pre-seeded to q8_0 before the first load and verified
`measured` in RUNMETA.

### Expert-split tweak (David, mid-session)

LM Studio's auto-split was conservative — 13,923 MiB, leaving 2.4 GB of VRAM unused. Setting
`numCpuExpertLayersRatio` to 0.2439 with `offloadRatio` 1 took it to 15,611 MiB. Effect:
probe 21.51 → 37.40 tok/s, and **A1 end-to-end 67 s → 30 s (2.2×)**.

⚠️ Another datapoint against the probe: it predicted 1.74× and the real gain was 2.2× — this
time the probe *under*-sold. It is directionally useful but quantitatively unreliable in both
directions. Only timed runs count.

This also removed a real confound. At the untweaked speed the candidate ran 4.8× the champion's
A1, close enough to the 600 s ceiling that a slow task could have scored FAIL for being slow
rather than wrong. The screen was restarted from scratch on the tweaked config for that reason.

---

## ★★★ gemma-4-26b-a4b-QAT — CROWNED QUALITY CHAMPION (3 runs, 2026-07-25/26)

**`google/gemma-4-26b-a4b-qat`, Q4_0, median 55/56 across three full runs. The champion's
50/56 is beaten by +5. This is the first model in the campaign to beat it at all.**

All three runs at byte-identical held constants, every RUNMETA row `measured`, same claudette
binary (mtime 2026-07-20 12:12:33): ctx 32768 / parallel 1 / KV q8_0+q8_0 / `offloadRatio` 1 /
`cpu_expert_ratio` **0.1** / 15530–15622 MiB of 16311. David tuned and approved the split himself.

| run | tag | score | wall | avg/task | fails |
|---|---|---|---|---|---|
| 1 | `q50-gemma4-qat` | **55/56** (98.2%) | 75m48s | 81s | Q46 |
| 2 | `q50-gemma4-qat-r2` | **54/56** (96.4%) | 75m35s | 80s | **Q11**, Q46 |
| 3 | `q50-gemma4-qat-r3` | **55/56** (98.2%) | 71m19s | 76s | Q46 |

**Median 55, mean 54.67, range 54–55.** Aggregate spread is ±0.5 — exactly as tight as the
champion's own 49/50/50/50/50/50. Two models, two competence plateaus, same instrument noise.

### The crown rule, all three clauses

| clause | requirement | result |
|---|---|---|
| (a) beat champion median | ≥ +4 over 50 (noise ±3) | **55 = +5** ✅ |
| (b) fix a majority of the discriminating failures | > 6 of the ever-failed 12 | **11/12 in all three runs** (champ 6/12) ✅ |
| (c) median of ≥2 runs | ≥2 | **3 runs** ✅ |

**→ CROWNED.** `google/gemma-4-26b-a4b-qat` is the quality champion — a **load-on-demand second
brain**. `qwen3.6-35b-a3b-mtp` keeps the daily-driver role on speed, per the campaign brief
("15 tok/s is acceptable if quality is much higher").

### What it actually fails

- **Q46 — 3/3, its one stable failure.** `range 5..3` returns `[5]`, expected `[]`; a start>end
  degenerate-input guard from the hardening batch. The champion also fails Q46, but only 2/6.
- **Q11 — 1/3** (run 2 only). Rust concurrency, lost update: `left: 34143`. Passed in runs 1 and 3.

So gemma's entire fail pool across 168 graded tasks is **two tasks**, one of them stable. 54 of
56 never failed once. Compare the champion, where **12 distinct tasks rotate** through the
failure slots across 6 runs. This is a materially different failure profile, not just a higher
score: gemma is not merely winning the coin flips, it has fewer coins.

### ⚠️ Methodological finding: the 12-task screen is champion-specific

**Q11 is not in `manifest-q50-everfailed.tsv`** — that set was derived from tasks the *champion*
ever failed. A challenger can therefore fail a task the screen cannot see, and its screen score
will overstate its full-56 result. It did no harm here (gemma scored 11/12 on the screen in all
three runs regardless, and the ≥9/12 gate was cleared on the first), but the derivation behind
the gate — "challenger full-56 ≤ screen12 + 44" — **only holds if the challenger passes all 44
off-screen tasks, which is exactly what Q11 violated.** In run 2 the bound predicted ≤55 and the
true score was 54.

**Consequence: the ≥9/12 screen is a reject-fast heuristic, not a sound bound.** Safe for killing
weak candidates cheaply; never sufficient to confirm a crown. The crown was decided on three full
56-task runs, which is the right call and should stay the standard. Say so in the writeup — a
benchmark that publishes a screen gate should publish its failure mode too.

### Two priors refuted (both were runtime-version artifacts, not model properties)

- **"Gemma 4 crashes / swap-unstable"** (2026-05-16): did not reproduce. Four clean loads, three
  75-minute runs, zero crashes.
- **"Failed the A1 template gate 0/1"**: template is **healthy** on LMS runtime 2.25.2. Smoke was
  5/5 — the best of any challenger, including Q35 which both gpt-oss-20b and coder-30b missed.

**Lesson (already recorded, now confirmed a third time): runtime updates flip template health in
both directions. Re-smoke, never assume — in either direction.**

### The Q53 clock worry — resolved, retire it

Run 1's Q53 took 662 s against a 700 s manifest ceiling (95%), raising the risk that a slower run
would lose the point to the clock rather than to quality. It did not: **549 s (r2) and 557 s (r3)**.
Run 1 was the outlier; the task sits comfortably ~79% of ceiling. No timeout-induced failure
occurred in any run.

### Cost

**3.3× slower than the champion** — median 75m35s vs ~23m, ~80 s/task. Acceptable for a
second brain that is loaded on demand, and the campaign brief priced this in from the start.
⚠️ The **1.19 GB vision projector is loaded** (`lms ps` shows 15.63 GB = 14.44 weights + 1.19
mmproj) and a coding battery never touches it. Reclaiming it is the obvious ~1.1 GB if a future
session wants to buy speed with a lower `cpu_expert_ratio`.
