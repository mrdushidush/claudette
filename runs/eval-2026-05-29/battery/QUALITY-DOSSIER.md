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
**Consistent failures (3/3 runs) — the stable discriminating signal:** Q03 (people==0 guard),
Q05 (blank-line skip), Q25 (integer-dollar price), Q51 (whitespace in duration), Q52
(capacity-0 ring buffer). Flaky: Q13 (2/3), Q45/Q50 (1/3). The hard tail bit on 2/6 (Q51,Q52);
the champion PASSED the genuinely-hard Q53 multi-line-CSV, Q54 topo-sort, Q55 merge-intervals,
Q56 version-compare. Headroom to beat: 6 points (50→56). A model fixing the 5 consistent
failures → ~55/56 = **+5, a clear crown**.

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

| config | Q56 PASS | fixes champ-fails? | gen tok/s | spill | template | verdict |
|--------|----------|--------------------|-----------|-------|----------|---------|
| champ bs-3.06 (MTP) | **50/56** (median 49/50/50) | — (baseline) | ~70 | none | ok | baseline |
| `qwen3.6-35b-a3b@iq4_xs` | **50/56** | 2/5 (Q05,Q52 ✓; Q03/Q25/Q51 ✗) | ~29 | slight | ok | **NO — ties baseline, +0; regresses Q14/Q49; slower** |
| _candidates…_ | | | | | | |

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

## Hunt protocol v2 (2026-07-22, David) — screen-first, one model at a time, 2 h apart

Per-candidate, to save tokens and thermal budget:
1. **Pull ONE model** (download-then-run; watch NVMe ≤71 °C).
2. Load (LMS bare id, confirm ctx 32768 / parallel 1 in `lms ps`), A1 smoke gate, speed probe (≥15 tok/s).
3. **Screen on the 5 champion-consistent-fails only** (`manifest-q50-champfails.tsv` = Q03 Q05 Q25 Q51 Q52).
4. **Gate:** passes **≥2 of 5** → promote to full 56. **<2** → reject, record, move on (no full run).
5. **Wait ~2 h between battery runs.** One candidate in flight at a time.

Candidate queue (coder-30b DROPPED per David — "will suck vs champion"):
byteshape MTP-GPU-3 (3.53 bpw) → MTP-GPU-4 (3.97) → MTP-GPU-5 (4.19), cheapest quality bump first.

## Status

Battery FROZEN (50/56 median baseline). IQ4_XS on-disk candidate RUN + REJECTED (ties, +0).
Next per protocol v2: pull byteshape MTP-GPU-3 (3.53 bpw) → screen on the 5 champ-fails → full-56 only if ≥2 pass.
