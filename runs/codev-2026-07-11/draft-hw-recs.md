# W2 draft — `hw.rs` `recommend_brain` refresh (2026-07-11)

Scope: table/score/id refresh ONLY. Thresholds (`FLAGSHIP_TIER_GIB = 15.0`) and
VRAM banding are explicitly out of scope. Single caller verified: `doctor.rs:421`
(user-facing doctor output; not model-visible) → **non-behavioral, no A/B**.

Sources: `runs/eval-2026-05-29/battery/CHAMPION-DOSSIER.md` §6,
v0.16.0 sweep tsvs (PR #184), README/docs tables refreshed by #182.

## Current → proposed

### Tier 1: ≥15 GiB + LM Studio (`openai_compat = true`)

Current: `qwen3.6-35b-a3b@q3_k_xl`, "92% … best accuracy" (STALE: 2026-05-30 numbers).

Proposed:
```rust
BrainRec {
    model: "byteshape/qwen3.6-35b-a3b-mtp",
    why: "50/50 + K 8/8 on the 50-task battery (49/50 at 64k ctx), ~70-76 tok/s — \
          fully VRAM-resident at 13.6 GB, zero RAM spill. Load with the README's \
          champion command (ctx 65536 + MTP draft flags)",
    alternatives: "same-score official-lineage alt qwen3.6-35b-a3b@iq4_xs (50/50, \
                   spills to RAM, 27.8 tok/s); known-good rollback \
                   qwen3.6-35b-a3b@q3_k_xl (47/50, 33.8 tok/s)",
}
```
Id note: public/catalog id keeps the `byteshape/` prefix (matches README + a
catalog download's LMS key); a side-loaded copy registers bare — the doctor's
"exact id from lms ps" guidance in configuration.md already covers that.

### Tier 2: ≥15 GiB + Ollama (`openai_compat = false`) — **DAVID GATE**

Current: `qwen3.5:9b`, "88% — best brain packaged on Ollama" (BROKEN: 9b
empty-turns under runtime 2.24.0 template flip; can no longer certify).

Candidates (2026-07-10 sweep, measured):
| option | model | score | notes |
|---|---|---|---|
| A | `qwen3.5:4b` | 45/50 (90%) + K 8/8 | best certified pullable score; same pick as sub-16 tier — tier collapses to "4b + switch-to-LMS advice" |
| B | `gpt-oss-20b` | 41/50 (82%) + K 7/8 | fastest full battery (6.1 min); weak at multi-site refactor; actually uses the 16 GB |
| C | A, with `why` leading on "the certified 16 GB picks are LM Studio-only — switch backends" | 45/50 | strongest push toward the champion |

All three keep `alternatives` pointing at the LMS switch
(`CLAUDETTE_OPENAI_COMPAT=1` + champion id) — the existing test asserts that
string is present, and it stays true.

### Tier 3: <15 GiB (both backends)

Current: `qwen3.5-4b`/`qwen3.5:4b`, "90% … in 8 min on ~3.4 GB"; alternatives
mention 9b (88%) + gpt-oss-20b (86%) — both stale/wrong now.

Proposed:
```rust
BrainRec {
    model: if openai_compat { "qwen3.5-4b" } else { "qwen3.5:4b" },
    why: "45/50 (90%) + K 8/8 on the 50-task battery — best value; ~3.4 GB pull, \
          runs on an 8 GB GPU or plain CPU",
    alternatives: "gpt-oss-20b (41/50, ~13 GB, fastest full-battery run) if you \
                   have the headroom",
}
```
(9b dropped entirely: template-broken on current runtime.)

### Doc comments + tests

- `BrainRec` struct doc + `recommend_brain` doc block: reseed from "2026-05-30
  run" to "2026-07-10/11 batteries (v0.16.0 sweep + champion dossier)"; rewrite
  the 9b rationale sentence (92%/88%/86% percentages all go).
- Tests `recommend_boundaries_match_the_certified_table` (hw.rs:163-184):
  - lines 168-170: expect `byteshape/qwen3.6-35b-a3b-mtp` (×3)
  - line 174: expect the David-chosen Ollama pick
  - line 175: `contains("CLAUDETTE_OPENAI_COMPAT=1")` — still valid, keep
  - lines 180-183: unchanged (4b ids stay)
- Docs echo grep: README + docs/hardware.md + docs/comparison.md +
  docs/configuration.md verified already fresh from #182 — **no doc edits in
  this card** (doctor's strings just catch up to them).
