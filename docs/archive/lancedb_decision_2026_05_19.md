# LanceDB rich memory — decision record

**Date:** 2026-05-19
**Phase:** 8 of `docs/archive/sprint_import_2026_05_19.md`
**Status:** **NOT NOW** — `recall.rs` + `antipatterns.rs` Jaccard cover the use case at v0.5.x scale. Re-evaluate when Phase 6 bench produces evidence that flat embeddings can't keep up.

## Question

Should claudette add LanceDB as an opt-in `--memory=rich` feature flag, lifted from stealthsambaV2 (Hadar-touched code, idea-only carry per [[project-import-sweep-2026-05-19]] §5)?

## What LanceDB would buy us

stealthsambaV2 paired LanceDB with petgraph and an antipattern-auto-detection loop. The combined system gives you:

1. **Embedding-grade similarity** instead of Jaccard token overlap. Better recall on paraphrased failures (`"the parser crashed"` vs `"failed to parse"`).
2. **Cross-mission corpus retrieval** — find precedents from prior missions at planning time.
3. **Graph edges** (petgraph) encoding dependencies + history so the planner can see *why* a similar mission worked.
4. **Persisted vector index** — fast reload at startup; today claudette's `recall.rs` rebuilds embeddings into RAM.

## What claudette has today (post-Phase 7)

- `recall.rs` (committed 2026-05-14, extended 2026-05-15 — see [[project-recall-embedding-probe-gap-fix]]): nomic-embed-text via Ollama, flat-file persistence under `~/.claudette/recall/`, sticky-disable + async indexer + `/recall reprobe` for mid-session recovery. Production-ready.
- `antipatterns.rs` (committed 2026-05-19, Phase 7): captures forge failures, clusters by Jaccard, graduates rules into the system-prompt overlay.

This stack covers the **antipattern feedback loop** end-to-end without LanceDB. The Jaccard similarity is empirically good enough at the scale claudette runs at today (single user, < 1000 missions / month).

## What would trigger a LanceDB build

Concrete signals that the flat-file + Jaccard stack has run out of road:

- **Phase 6 bench data shows recall miss-rate > 20%** on the 10-template corpus. (Miss = "a previously-shipped fix was a relevant precedent but the recall didn't surface it.")
- **Antipattern clusters fail to coalesce** because paraphrased feedback ("range bounds wrong" vs "off-by-one") doesn't share Jaccard tokens. Symptom: graduation count stalls below 1/week with > 10 failures/week, none clustering.
- **Cross-mission queries take > 200ms** to scan the flat-file corpus. Symptom: forge planner stage gains > 1s of latency.

None of these are observed at v0.5.4. The Phase 6 bench is where these would surface, and Phase 6 ships before this decision needs revisiting.

## What it costs to add later (insurance the deferral is cheap)

- New dependency: `lancedb` crate (~30 MB compressed binary contribution; pulls in Arrow). Claudette's "lean deps" baseline becomes harder to defend.
- New runtime requirement: a writable `~/.claudette/lance/` directory and migration logic from the existing flat-file recall.
- Schema decisions (embedding model, dimension, distance metric) that lock in for at least one major version.

These are all manageable when the data justifies them. They aren't manageable when adding LanceDB now would compete with shipping Phase 6 + closing out the import sweep.

## Decision

**Defer.** Add a follow-up note to revisit after Phase 6 bench produces empirical recall-miss-rate evidence. Open the discussion with concrete numbers, not vibes.

## Open exit criteria for revisiting

Add a one-line check to `docs/archive/lancedb_decision_2026_05_19.md` (this file) each time the bench harness emits a new run:

```
[YYYY-MM-DD] bench run-id <id>: recall miss-rate <pct>% (template <name>). LanceDB decision: defer / build.
```

When three consecutive entries show miss-rate > 20%, flip the decision and open a sprint.

## References

- [[project-recall-embedding-probe-gap-fix]] — claudette's current embedding stack
- [[project-import-sweep-2026-05-19]] §3.1 — original Tier-2 entry justifying the deferral
- `docs/archive/sprint_import_2026_05_19.md` Phase 8 — sprint plan slot
- `docs/archive/import_sweep_2026_05_19.md` §5 — Hadar-touched code restriction (lift ideas, not code)
