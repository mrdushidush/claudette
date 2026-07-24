# CLAUDE.md — claudette repo notes

Claudette is an air-gapped AI coding agent: single Rust binary (one crate,
`crates/claudette`), local models only (Ollama / LM Studio), published on crates.io.
`--offline` hard-blocks all egress except the local model server. See README for
features; `docs/PRIVACY.md` for the privacy posture.

## Current focus (2026-07)

- Branch `battery/q50-quality-corpus`: the Q50 hidden-verification quality battery,
  **frozen 2026-07-22 at 56 tasks**. Champion `qwen3.6-35b-a3b-mtp` baseline 50/56.
  Crown rule: challenger must beat median by ≥+4 AND fix a majority of the 5
  discriminating failures (Q03/Q05/Q25/Q51/Q52). Corpus lives only on this branch;
  `main` stays at the control commit. Campaign goal:
  `launch-drafts/goal_quality_champion_2026_07_21.md`.
- `D:\dev\claudette-forge` is a deliberate second clone used as a **safe test sandbox**
  (so claudette doesn't operate on her own repo). Do not delete or "clean up" — its
  staleness is intentional.

## Ancestry & the 2026-07-24 salvage

The whole ancestor family (ABCC → tacticode/battleclaw-v2 → battleclaw-forge →
clawForge → claudette; plus Beast) was archived to `D:\dev\_archive\` after an
idea-mining pass. Salvaged into this repo (may still be uncommitted):

- `docs/essence-from-ancestors-2026-07.md` — **the curated import backlog**: ranked
  evidence-backed ideas never inherited (grounded-feedback fix loop, agentic localizing
  Planner, LLM-judge score clamping, watchdog timeout, local-model behavior catalog),
  the deliberately-cut list (revive only with a reason), and the family's binding
  lesson: LLM judges inflate — deterministic ground truth may only be *lowered* by a
  model, never raised.
- `docs/history/claudettes-forge/` — the ancestor's sprint/decision paper trail.
- `crates/claudette/personas/sentinel9.md` — restored persona file. Wiring it back is
  an **open decision**: legacy-audit PR-F cut it, but PROJECT_REVIEW §14.1-C wanted
  CodeX-7 + Sentinel-9 from day 1 with a `--faceless` toggle.
- `runs/clawforge-codet-benchmarks/` — Codet model-shootout datasets from clawForge
  (~10 Ollama models, 3b–30b), reference data for battery work.

Lineage bible: `D:\dev\_archive\abcc_projects\abcc_projects\PROJECT_REVIEW.md` (210 KB,
§12 cherry-pick catalog, §14 locked decisions) + `TIMELINE.md`. Beast's post-mortem:
`D:\dev\_archive\Beast\DECISION-beast-vs-claudette.md` (also pushed to private
github.com/mrdushidush/beast).

## Planned first imports (owner-agreed direction, not yet started)

1. Grounded-feedback fix loop — pipe real test-failure output into forge fix rounds.
2. Static clamp on the forge Verifier's brain score (build fails → ceiling 4.0, etc.).
Both touch the forge Verifier; see essence doc §2.1/§2.3 for sources and evidence.

## Longer-term goal

Claudette's first "real" external repo to run and maintain will be
`D:\dev\algo-trading-bot` (AlgoArb — Algorand arb + Folks liquidation bot, dormant
since 2026-06-11, revival planned). Its lessons docs: `docs/IMPROVEMENT_PLAN.md` in
that repo; sibling post-mortems in `D:\dev\_archive\sui-arb-bot\ANALYSIS.md` and
`D:\dev\_archive\base-liquidator\CLAUDE.md`. Note: its `.env` holds a live funded
wallet mnemonic — never commit, never echo, and it should be rotated before claudette
operates that repo.
