**Task:** Create the pure context-eviction module — trigger math, staleness selection, stub builder — with full unit tests. NOTHING calls this module yet (the send-path hook is a separate Claude-led PR), so this change is behavior-preserving by construction. Design: `runs/codev-2026-07-11/design-context-eviction.md`.

Numbered steps — follow exactly:

1. In `crates/claudette/src/compact.rs` (mounted from `runtime/compact.rs`): change `fn estimate_message_tokens(` to `pub(crate) fn estimate_message_tokens(` — one word, nothing else in that file.

2. Create `crates/claudette/src/runtime/context_evict.rs`. Module doc: wire-level eviction pass that, under context pressure, replaces the bodies of STALE tool results with a short recovery stub; never touches the current turn or the most-recent K tool results; persisted session data is never modified (callers apply this to the outgoing payload only). Note the #61 lesson in the doc: the stub must not invite re-fetching. Then add `#![allow(dead_code)] // wired into the send path in the follow-up PR (W5b)`.

3. Implement with these EXACT names, signatures, and behaviors:
   - `pub(crate) const EVICT_ENV: &str = "CLAUDETTE_EVICT_TOOL_OUTPUT";`
   - `pub(crate) const KEEP_RECENT_TOOL_RESULTS: usize = 8;`
   - `pub(crate) const MIN_EVICTABLE_CHARS: usize = 512;`
   - `pub(crate) const DEFAULT_TRIGGER_PERCENT: usize = 60;`
   - `pub(crate) const STUB_MARKER: &str = "{\"evicted\":true";`
   - `pub(crate) fn trigger_percent() -> Option<usize>` — reads `EVICT_ENV` (trimmed): unset or empty → `None` (feature OFF); `1`/`true`/`yes`/`on` (ASCII case-insensitive) → `Some(DEFAULT_TRIGGER_PERCENT)`; an integer in `10..=90` → `Some(n)`; anything else → `None` (fail-closed OFF).
   - `pub(crate) fn stub_body(tool_name: &str, original_chars: usize) -> String` — returns exactly this JSON (one line, `format!`-ed):
     `{"evicted":true,"tool":"<tool_name>","original_chars":<n>,"note":"Stale output from an earlier turn, cleared to free context. Anything decided from it is already reflected in the conversation. Do NOT re-run the tool just to restore this text — only re-run it if a NEW step genuinely needs the raw content."}`
   - `pub(crate) fn evict_stale_tool_outputs(messages: &[ConversationMessage], num_ctx: usize, percent: usize) -> Option<Vec<ConversationMessage>>` with this exact algorithm:
     1. `threshold = num_ctx.saturating_mul(percent) / 100`; `estimate = messages.iter().map(crate::compact::estimate_message_tokens).sum::<usize>()`. If `estimate < threshold` → `None`.
     2. Current-turn boundary: index of the LAST message with `role == MessageRole::User`. Every message at that index or later is immune. (No user message at all → `None`; nothing is safely stale.)
     3. Recency immunity: walk ALL messages' `ContentBlock::ToolResult` blocks in order; the last `KEEP_RECENT_TOOL_RESULTS` of them (by position, counting from the end, current turn included in the count) are immune.
     4. Candidates: ToolResult blocks BEFORE the boundary, not recency-immune, `output.len() >= MIN_EVICTABLE_CHARS`, and `!output.starts_with(STUB_MARKER)`.
     5. Evict oldest-first: clone the messages, replace each candidate's `output` with `stub_body(tool_name, original_len)`, subtract `(original_len - stub_len) / 4` from `estimate`, stop as soon as `estimate < threshold` or candidates run out.
     6. Evicted at least one → `Some(new_vec)`; else `None`.
   - Use the existing types/constructors from `crate::session` (`ConversationMessage`, `ContentBlock`, `MessageRole`) — grep for the `tool_result` constructor and the user/assistant helpers rather than building structs by hand in tests, where they exist.

4. `#[cfg(test)] mod tests` in the same file, own `static ENV_LOCK: std::sync::Mutex<()>` for the env-reading tests (restore vars before dropping the guard). EXACTLY these tests:
   - `trigger_percent_parses` — unset → None; `"1"`,`"true"`,`"ON"` → Some(60); `"40"` → Some(40); `"5"` → None; `"95"` → None; `"garbage"` → None; `""` → None.
   - `under_threshold_is_passthrough_none`
   - `no_user_message_is_none`
   - `current_turn_results_are_immune` — one huge stale-shaped result placed AFTER the last user message, over threshold → None.
   - `last_k_tool_results_are_immune` — 9 old tool results over threshold: only the oldest 1 is evictable (8 kept).
   - `evicts_oldest_first_and_stops_at_threshold` — two big stale results where evicting the first is enough → second keeps its body.
   - `small_results_are_skipped` — sub-512-char stale results → None.
   - `already_stubbed_results_are_skipped` — a result whose output starts with `STUB_MARKER` is not re-evicted (idempotence).
   - `stub_body_is_valid_json_and_discourages_refetch` — parses as JSON; contains the tool name, the char count, and the substring `Do NOT re-run`.
   - `eviction_preserves_message_and_block_counts` — same number of messages/blocks after eviction; only `output` strings changed.
5. Register the module: in `crates/claudette/src/lib.rs`, add, directly after the `#[path = "runtime/compact.rs"] pub mod compact;` pair:
   ```rust
   #[path = "runtime/context_evict.rs"]
   pub mod context_evict;
   ```
6. Touch NOTHING else.

**Do NOT touch:**
- `runs/eval-2026-05-29/battery/MODEL-COMPARISON.md`
- any file under `tests/results_*`
- any `tests/*_prompts.txt`
- any file not named above (this task touches exactly THREE files: new `context_evict.rs`, one-word visibility change in `runtime/compact.rs`, two-line mount in `lib.rs`).

**Gate (run before finishing):** `cargo fmt --all && cargo clippy --all-targets --all-features --no-deps -- -D warnings && cargo test && cargo test --all-features` — all must pass.

**After the gate passes, COMMIT the changes yourself** (the Verifier reads committed diffs), **but do NOT push and do NOT open a PR** — the operator pushes the mission branch and opens the PR. Commit message (exactly, no Co-Authored-By trailer):
```
feat(runtime): pure stale-tool-output eviction module (knob-gated)

First half of the context-eviction design
(runs/codev-2026-07-11/design-context-eviction.md). Pure pass only:
CLAUDETTE_EVICT_TOOL_OUTPUT knob parsing (default OFF, fail-closed),
60%-of-num_ctx trigger math, staleness selection (current turn and the
last 8 tool results immune, 512-char floor, oldest-first), and a stub
body written to NOT invite re-fetching (the #61 failure mode). Nothing
calls the pass yet; the send-path hook lands separately.
```

**Expected diff:** 3 files — `runtime/context_evict.rs` (new, roughly 300–400 lines incl. tests), `runtime/compact.rs` (1 word), `lib.rs` (+2 lines).

After the gate passes, STOP — report done and let the pipeline submit.
