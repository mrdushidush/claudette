# Brain100 LM Studio Shopping — Cumulative Scoreboard

Aggregate results across LM Studio brain candidates tested via
`brain100_lmstudio_shopping.sh`. Tiers: T1 Basic (1-20) / T2 Params (21-40) /
T3 Multi-step (41-60) / T4 Edge cases (61-80) / T5 Complex (81-100).

| Model | Score | T1 | T2 | T3 | T4 | T5 | Wall | Notes |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|---|
| **unsloth/qwen3.6-35b-a3b** (UD-Q3_K_XL) | **95%** | 20/20 | 19/20 | 19/20 | 18/20 | 19/20 | 2894s | **DAILY DRIVER (2026-04-30)** — both brain + codet roles, no swap dance |
| devstral-small-2-24b-instruct-2512 | 96% | 20/20 | 20/20 | 18/20 | 20/20 | 18/20 | 1448s | Prior daily driver (2026-04-28). Faster but dense → hotter GPU. Retired in favor of Qwen 3.6's MoE thermals. |
| mistralai/ministral-3-14b-reasoning | 93% | 19/20 | 18/20 | 17/20 | 20/20 | 19/20 | 1294s | T5 leader; only co-resident-feasible option on 16 GB but -3pts vs solo Qwen 3.6 |
| qwen3.5-4b | 89% | 20/20 | 20/20 | 15/20 | 18/20 | 16/20 | 752s | Speed fallback; -6pts vs daily driver, half the wall time |

## 2026-04-30 decision: Qwen 3.6 35B-A3B becomes the single brain

**One model, both roles** (agent loop *and* codet sidecar). Trade-offs accepted:

- **Score:** 95 vs Devstral 96 — within bench noise (prompt 52 was unload collateral; adjusted = 96 vs 96).
- **Wall time:** 2× slower per prompt (29s vs 15s avg) due to reasoning trace + MoE-on-CPU PCIe swap. Acceptable.
- **GPU thermals:** Qwen 3.6 is A3B MoE (3B active per token) → GPU runs materially cooler than Devstral's dense 24B. Sustained-session ergonomics win.
- **Architecture simplification:** kills the brain↔codet swap dance. One model load, one model in memory, one runtime story.
- **Tier shape:** Qwen wins T3 multi-step + T5 complex; Devstral wins T2 params + T4 edge. Different fail modes, similar magnitude.

## How to reproduce

```bash
cd D:/dev/claudette
# Edit tests/brain100_lmstudio_shopping.sh MODELS array as needed
bash tests/brain100_lmstudio_shopping.sh
```

Per-prompt logs land in `tests/results_brain100_<model_safe_name>/` (gitignored).
Summary.txt per run aggregates into this file.
