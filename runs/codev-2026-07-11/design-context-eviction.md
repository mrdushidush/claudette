# W5 design — stale tool-output eviction (wire-level, knob-gated, default OFF)

**One sentence:** when the outgoing prompt is estimated at ≥60% of `num_ctx`, a
pre-send pass replaces the bodies of *stale* tool results (older turns, beyond the
last-K window) with a short recovery stub — cutting per-turn prompt-processing,
the measured wall-clock dominator on the local backend.

**Ship shape:** knob `CLAUDETTE_EVICT_TOOL_OUTPUT=1` (default OFF; knob-off
byte-identical) · behavioral when ON → A/B gate · default recommendation goes in
the PR body, David decides.

## Why #61 failed, and how this differs (mandatory history)

PR #61 ("elide stale repo_map results from the wire", CLOSED unmerged): with the
map elided to a placeholder saying *"call repo_map again if you need it"*, the 35B
**re-fetched repo_map in a loop** — net slower than the bloat it cured. Superseded
by #62 (smaller map output + tool-description steering; no context surgery).

Four deliberate differences:

| #61 | this design |
|---|---|
| fired unconditionally on the next user turn | fires only under real pressure (≥60% of num_ctx) |
| targeted the single freshest repo_map (the model's working set) | never touches the current turn OR the last K=8 tool results — the working set stays intact |
| stub was an active invitation to re-fetch | stub phrased to discourage re-fetching (below), and the duplicate-call loop-breakers (#77 lineage) backstop pathological repeats |
| repo_map-specific | tool-agnostic: any bulky stale result (read_file, shell, grep, repo_map…) |

## Mechanism

Wire-level only, mirroring #61's transport choice AND `evict_older_image_bytes`'s
oldest-first selection: persisted session/history/undo/transcript are untouched;
the pass runs on the messages payload at send time (`Cow::Owned` only when it
actually stubs; borrows otherwise — the no-pressure path allocates nothing).

**Trigger math (exact):** let `E` = the existing char/4 token estimate over the
outgoing messages. Pass runs iff `E ≥ 0.60 × num_ctx` (env-derived, same source as
compaction). Eviction proceeds oldest-first until `E < 0.60 × num_ctx` or no
candidates remain. Note the interplay: default compaction fires at `num_ctx/2` —
at 60% eviction is the *between-compaction relief valve*. A 40% variant (eviction
as the first line, delaying lossy summarization) is worth an A/B arm; knob design
allows `CLAUDETTE_EVICT_TOOL_OUTPUT=<percent>` (bare `1` → 60).

**Staleness (all must hold):**
1. message is a tool result from a turn BEFORE the last user message (current
   turn is never touched);
2. not among the last K=8 tool results of the whole conversation;
3. body ≥ 512 chars (stubbing short results saves nothing);
4. not already a stub.

**Stub format** (house style of `duplicate_edit_body`):
```json
{"evicted":true,"tool":"<name>","original_chars":12345,
 "note":"Stale output from an earlier turn, cleared to free context. Anything
 decided from it is already reflected in the conversation. Do NOT re-run the
 tool just to restore this text — only re-run it if a NEW step genuinely needs
 the raw content."}
```

## Test plan

Unit (pure pass over `Vec<ConversationMessage>`): under-threshold → passthrough
borrow; current-turn immunity; last-K immunity; oldest-first order; floor;
already-stubbed idempotence; stub JSON shape; knob-off → passthrough regardless of
size. Integration: knob-off byte-identical wire payload (snapshot test); knob-on
end-to-end turn against a stub backend asserting the payload shrank.

## Measurement plan (record in PLAN.md)

20-turn dogfood session on the champion, knob off vs on: tokens-in on each turn
(SSE usage field), wall per turn, any re-fetch behavior in `lms log stream`.
Success = later-turn tokens-in drops materially with zero re-fetch loops. Then
brain100 A/B knob-ON on both gate models (Appendix C), with special eyes on
I-series (deep-locate needs old context!) — I-series regression = the staleness
window is too aggressive; raise K / restrict to same-file re-reads first.

## Risk split for implementation (decided now, per goal doc)

- **Card W5a (Claudette-forge):** the pure eviction module — candidate selection,
  trigger math, stub builder, ALL unit tests. No wiring. Mechanical given exact
  spec.
- **W5b (Claude-led under the escalation rule, decided at spec time):** the send-
  path hook in `runtime/conversation.rs` (`ApiRequest` Cow plumbing + estimator
  call). This is the exact surface #61 got wrong and it touches every request; it
  needs the reviewer's-eye judgment, not a card. Claudette still reviews-by-forge?
  No — Claude writes it, Claudette's Verifier value is nil on Claude PRs; David
  reviews that PR himself.
