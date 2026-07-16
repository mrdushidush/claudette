//! Context-eviction pass — replaces stale tool-result bodies with a short
//! recovery stub when context pressure exceeds a knob-gated threshold.
//!
//! This is the wire-level eviction pass that, under context pressure, replaces
//! the bodies of STALE tool results with a short recovery stub; it never touches
//! the current turn or the most-recent K tool results; persisted session data is
//! never modified (callers apply this to the outgoing payload only).
//!
//! # Stub semantics (#61 lesson)
//! The stub text explicitly instructs the model NOT to re-run the tool just to
//! restore the evicted content. This prevents the #61 failure mode where a
//! model sees an empty/stale result and blindly re-invokes the same tool,
//! wasting tokens on redundant work that was already reflected in downstream
//! reasoning.

#![allow(dead_code)] // wired into the send path in the follow-up PR (W5b)

use crate::compact::estimate_message_tokens;
use crate::session::{ContentBlock, ConversationMessage, MessageRole};

/// Environment variable name for the eviction knob.
pub(crate) const EVICT_ENV: &str = "CLAUDETTE_EVICT_TOOL_OUTPUT";

/// Number of most-recent tool-result blocks to keep immune from eviction.
pub(crate) const KEEP_RECENT_TOOL_RESULTS: usize = 8;

/// Minimum output length (chars) for a ToolResult to be considered evictable.
pub(crate) const MIN_EVICTABLE_CHARS: usize = 512;

/// Default trigger percentage — the fraction of num_ctx at which eviction
/// activates. Only used when EVICT_ENV is set to a truthy value with no
/// explicit percentage.
pub(crate) const DEFAULT_TRIGGER_PERCENT: usize = 60;

/// Prefix marker for stubbed (evicted) output. Used to detect already-evicted
/// results and avoid double-eviction.
pub(crate) const STUB_MARKER: &str = "{\"evicted\":true";

/// Parse the eviction trigger from the environment variable.
///
/// Returns `Some(percent)` when the knob is ON, `None` when OFF.
///
/// Parsing rules (ASCII case-insensitive):
/// - Unset or empty → `None` (feature OFF)
/// - `"1"`, `"true"`, `"yes"`, `"on"` → `Some(DEFAULT_TRIGGER_PERCENT)` (= 60)
/// - An integer in `10..=90` → `Some(n)`
/// - Anything else → `None` (fail-closed OFF)
pub(crate) fn trigger_percent() -> Option<usize> {
    let raw = std::env::var(EVICT_ENV).ok()?.trim().to_ascii_lowercase();
    match raw.as_str() {
        "1" | "true" | "yes" | "on" => Some(DEFAULT_TRIGGER_PERCENT),
        n => n.parse::<usize>().ok().filter(|v| (10..=90).contains(v)),
    }
}

/// Build the stub body for an evicted tool result.
///
/// Returns a single-line JSON object with the eviction metadata and a
/// directive discouraging re-fetching.
pub(crate) fn stub_body(tool_name: &str, original_chars: usize) -> String {
    format!(
        "{{\"evicted\":true,\"tool\":\"{}\",\"original_chars\":{},\"note\":\"Stale output from an earlier turn, cleared to free context. Anything decided from it is already reflected in the conversation. Do NOT re-run the tool just to restore this text — only re-run it if a NEW step genuinely needs the raw content.\"}}",
        tool_name,
        original_chars
    )
}

/// Evict stale tool outputs when context pressure exceeds the threshold.
///
/// Algorithm:
/// 1. Compute token estimate across all messages. If below threshold → None.
/// 2. Find the current-turn boundary (last User message index). Messages at or
///    after that index are immune.
/// 3. Walk all ToolResult blocks; the last `KEEP_RECENT_TOOL_RESULTS` are
///    recency-immune regardless of position.
/// 4. Candidates: before boundary, not recency-immune, output >= MIN chars,
///    doesn't already start with STUB_MARKER.
/// 5. Evict oldest-first (by position), replacing each candidate's output with
///    the stub body and subtracting token savings from estimate. Stop when
///    estimate < threshold or candidates run out.
/// 6. If at least one was evicted → Some(new_vec); else None.
pub(crate) fn evict_stale_tool_outputs(
    messages: &[ConversationMessage],
    num_ctx: usize,
    percent: usize,
) -> Option<Vec<ConversationMessage>> {
    let threshold = num_ctx.saturating_mul(percent) / 100;
    let estimate: usize = messages.iter().map(estimate_message_tokens).sum();

    if estimate < threshold {
        return None;
    }

    // Current-turn boundary: index of the LAST User message.
    let last_user_idx = messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, m)| m.role == MessageRole::User)
        .map(|(i, _)| i);

    // No user message at all → nothing is safely stale.
    let boundary = last_user_idx?;

    // Recency immunity: collect the last KEEP_RECENT_TOOL_RESULTS ToolResult
    // blocks by position (across all messages). Any candidate that appears in
    // this set is immune.
    let mut tool_result_positions: Vec<(usize, usize)> = Vec::new(); // (msg_idx, block_idx)
    for (mi, msg) in messages.iter().enumerate() {
        for (bi, block) in msg.blocks.iter().enumerate() {
            if matches!(block, ContentBlock::ToolResult { .. }) {
                tool_result_positions.push((mi, bi));
            }
        }
    }

    let recency_immune: std::collections::HashSet<(usize, usize)> = tool_result_positions
        .iter()
        .rev()
        .take(KEEP_RECENT_TOOL_RESULTS)
        .copied()
        .collect();

    // Candidates: ToolResult blocks BEFORE the boundary that are not recency-immune.
    let candidates: Vec<(usize, usize)> = tool_result_positions
        .into_iter()
        .filter(|&(mi, _)| mi < boundary)
        .filter(|pos| !recency_immune.contains(pos))
        .collect();

    // Filter by output size and stub marker.
    let mut evictable: Vec<(usize, usize)> = Vec::new();
    for &(mi, bi) in &candidates {
        let msg = &messages[mi];
        if let ContentBlock::ToolResult { output, .. } = &msg.blocks[bi] {
            if output.len() >= MIN_EVICTABLE_CHARS && !output.starts_with(STUB_MARKER) {
                evictable.push((mi, bi));
            }
        }
    }

    // Evict oldest-first.
    let mut result: Vec<ConversationMessage> = messages.to_vec();
    let mut current_estimate = estimate;
    let mut evicted_any = false;

    for &(mi, bi) in &evictable {
        if current_estimate < threshold {
            break;
        }
        if let ContentBlock::ToolResult {
            tool_name, output, ..
        } = &mut result[mi].blocks[bi]
        {
            let stub = stub_body(tool_name, output.len());
            current_estimate =
                current_estimate.saturating_sub(output.len().saturating_sub(stub.len()) / 4);
            *output = stub;
            evicted_any = true;
        }
    }

    if evicted_any {
        Some(result)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ── trigger_percent_parses ────────────────────────────────────────────

    #[test]
    fn trigger_percent_parses() {
        let _lock = ENV_LOCK.lock().unwrap();

        // Unset → None
        std::env::remove_var(EVICT_ENV);
        assert_eq!(trigger_percent(), None);

        // "1" → Some(60)
        std::env::set_var(EVICT_ENV, "1");
        assert_eq!(trigger_percent(), Some(60));

        // "true" → Some(60)
        std::env::set_var(EVICT_ENV, "true");
        assert_eq!(trigger_percent(), Some(60));

        // "ON" (uppercase) → Some(60)
        std::env::set_var(EVICT_ENV, "ON");
        assert_eq!(trigger_percent(), Some(60));

        // "40" → Some(40)
        std::env::set_var(EVICT_ENV, "40");
        assert_eq!(trigger_percent(), Some(40));

        // "5" → None (below 10)
        std::env::set_var(EVICT_ENV, "5");
        assert_eq!(trigger_percent(), None);

        // "95" → None (above 90)
        std::env::set_var(EVICT_ENV, "95");
        assert_eq!(trigger_percent(), None);

        // "garbage" → None
        std::env::set_var(EVICT_ENV, "garbage");
        assert_eq!(trigger_percent(), None);

        // "" → None
        std::env::set_var(EVICT_ENV, "");
        assert_eq!(trigger_percent(), None);

        // Clean up
        std::env::remove_var(EVICT_ENV);
    }

    // ── under_threshold_is_passthrough_none ────────────────────────────────

    #[test]
    fn under_threshold_is_passthrough_none() {
        let msgs = vec![ConversationMessage::user_text("hello")];
        // num_ctx=100, percent=60 → threshold=60; estimate ~2 tokens → None
        assert!(evict_stale_tool_outputs(&msgs, 100, 60).is_none());
    }

    // ── no_user_message_is_none ────────────────────────────────────────────

    #[test]
    fn no_user_message_is_none() {
        // Big enough to be over the threshold — the None must come from the
        // missing-user-message check, not the pressure check.
        let msgs = vec![ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "x".repeat(4000),
        }])];
        assert!(evict_stale_tool_outputs(&msgs, 1000, 60).is_none());
    }

    // ── current_turn_results_are_immune ─────────────────────────────────────

    #[test]
    fn current_turn_results_are_immune() {
        // user → assistant tool_use → huge stale-shaped result. Everything at
        // or after the last user message is the current turn and immune, even
        // though the estimate is over the threshold.
        let msgs = vec![
            ConversationMessage::user_text("start"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "read_file".into(),
                input: "{}".into(),
            }]),
            ConversationMessage::tool_result("t1", "read_file", "x".repeat(4096), false),
        ];

        // estimate ≈ 1030 tokens, threshold = 600 → over, but nothing evictable.
        assert!(evict_stale_tool_outputs(&msgs, 1000, 60).is_none());
    }

    // ── last_k_tool_results_are_immune ──────────────────────────────────────

    #[test]
    fn last_k_tool_results_are_immune() {
        // user → 9 (assistant tool_use, tool_result) pairs → user. The
        // trailing user message makes all 9 results stale; the last 8 are
        // recency-immune, so only the oldest (message index 2) is evictable.
        let mut msgs = vec![ConversationMessage::user_text("start")];
        for i in 0..9 {
            let tu_id = format!("tu_{i}");
            msgs.push(ConversationMessage::assistant(vec![
                ContentBlock::ToolUse {
                    id: tu_id.clone(),
                    name: "tool".into(),
                    input: "{}".into(),
                },
            ]));
            msgs.push(ConversationMessage::tool_result(
                &tu_id,
                "tool",
                "A".repeat(600),
                false,
            ));
        }
        msgs.push(ConversationMessage::user_text("next"));

        // estimate ≈ 1390 tokens, threshold = 1200 → over. Evicting the one
        // candidate is not enough to get back under the threshold, but partial
        // relief still returns Some (evicted at least one → Some).
        let result = evict_stale_tool_outputs(&msgs, 2000, 60).expect("one eviction expected");

        // Tool results sit at message indices 2, 4, …, 18. Only the oldest
        // (index 2) may be stubbed.
        for mi in (2..=18).step_by(2) {
            if let ContentBlock::ToolResult { output, .. } = &result[mi].blocks[0] {
                if mi == 2 {
                    assert!(
                        output.starts_with(STUB_MARKER),
                        "oldest result must be stubbed"
                    );
                } else {
                    assert!(
                        !output.starts_with(STUB_MARKER),
                        "msg {mi} must keep its body"
                    );
                }
            } else {
                panic!("expected ToolResult at message {mi}");
            }
        }
    }

    // ── evicts_oldest_first_and_stops_at_threshold ──────────────────────────

    #[test]
    fn evicts_oldest_first_and_stops_at_threshold() {
        // Two big stale results (both candidates: 10 results total, the 8
        // small fillers absorb the recency immunity), then a fresh user
        // message. Evicting the first big result is enough to drop below the
        // threshold, so the second keeps its body.
        let mut msgs = vec![
            ConversationMessage::user_text("start"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "tool_a".into(),
                input: "{}".into(),
            }]),
            ConversationMessage::tool_result("t1", "tool_a", "B".repeat(4096), false),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t2".into(),
                name: "tool_b".into(),
                input: "{}".into(),
            }]),
            ConversationMessage::tool_result("t2", "tool_b", "C".repeat(4096), false),
        ];
        for i in 0..8 {
            let tu_id = format!("f_{i}");
            msgs.push(ConversationMessage::assistant(vec![
                ContentBlock::ToolUse {
                    id: tu_id.clone(),
                    name: "tool".into(),
                    input: "{}".into(),
                },
            ]));
            msgs.push(ConversationMessage::tool_result(
                &tu_id,
                "tool",
                "x".repeat(100),
                false,
            ));
        }
        msgs.push(ConversationMessage::user_text("next"));

        // estimate ≈ 2290 tokens, threshold = 1500; evicting tool_a frees
        // ~950 → ~1340 < 1500 → stop before tool_b.
        let result = evict_stale_tool_outputs(&msgs, 2500, 60).expect("eviction expected");

        if let ContentBlock::ToolResult { output, .. } = &result[2].blocks[0] {
            assert!(
                output.starts_with(STUB_MARKER),
                "oldest big result must be stubbed"
            );
        } else {
            panic!("expected ToolResult at index 2");
        }
        if let ContentBlock::ToolResult { output, .. } = &result[4].blocks[0] {
            assert!(
                !output.starts_with(STUB_MARKER),
                "second big result must keep its body"
            );
        } else {
            panic!("expected ToolResult at index 4");
        }
    }

    // ── small_results_are_skipped ───────────────────────────────────────────

    #[test]
    fn small_results_are_skipped() {
        // The only non-immune candidate is under MIN_EVICTABLE_CHARS, so
        // nothing is evicted even though the estimate is over the threshold.
        let mut msgs = vec![
            ConversationMessage::user_text("start"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "tool".into(),
                input: "{}".into(),
            }]),
            // Only 200 chars — below MIN_EVICTABLE_CHARS (512).
            ConversationMessage::tool_result("t1", "tool", "x".repeat(200), false),
        ];
        for i in 0..8 {
            let tu_id = format!("f_{i}");
            msgs.push(ConversationMessage::assistant(vec![
                ContentBlock::ToolUse {
                    id: tu_id.clone(),
                    name: "tool".into(),
                    input: "{}".into(),
                },
            ]));
            msgs.push(ConversationMessage::tool_result(
                &tu_id,
                "tool",
                "x".repeat(600),
                false,
            ));
        }
        msgs.push(ConversationMessage::user_text("next"));

        // estimate ≈ 1290 tokens, threshold = 1200 → over, but no eviction.
        assert!(evict_stale_tool_outputs(&msgs, 2000, 60).is_none());
    }

    // ── already_stubbed_results_are_skipped ──────────────────────────────────

    #[test]
    fn already_stubbed_results_are_skipped() {
        // A previously-evicted result, padded past the size floor so only the
        // marker check can skip it — it must not be re-evicted (idempotence).
        let mut stubbed = stub_body("tool", 2048);
        stubbed.push_str(&"x".repeat(600));

        let mut msgs = vec![
            ConversationMessage::user_text("start"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "tool".into(),
                input: "{}".into(),
            }]),
            ConversationMessage::tool_result("t1", "tool", stubbed, false),
        ];
        for i in 0..8 {
            let tu_id = format!("f_{i}");
            msgs.push(ConversationMessage::assistant(vec![
                ContentBlock::ToolUse {
                    id: tu_id.clone(),
                    name: "tool".into(),
                    input: "{}".into(),
                },
            ]));
            msgs.push(ConversationMessage::tool_result(
                &tu_id,
                "tool",
                "x".repeat(600),
                false,
            ));
        }
        msgs.push(ConversationMessage::user_text("next"));

        // estimate ≈ 1460 tokens, threshold = 1200 → over, but the only
        // candidate already starts with STUB_MARKER → None.
        assert!(evict_stale_tool_outputs(&msgs, 2000, 60).is_none());
    }

    // ── stub_body_is_valid_json_and_discourages_refetch ─────────────────────

    #[test]
    fn stub_body_is_valid_json_and_discourages_refetch() {
        let body = stub_body("read_file", 1234);
        assert!(body.starts_with(STUB_MARKER));

        // Parse as JSON — should succeed.
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("stub must be valid JSON");
        let obj = parsed.as_object().expect("stub must be an object");

        assert_eq!(obj.get("evicted"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(
            obj.get("tool"),
            Some(&serde_json::Value::String("read_file".into()))
        );
        assert_eq!(
            obj.get("original_chars"),
            Some(&serde_json::Value::Number(1234.into()))
        );

        // Contains the discouragement text.
        let note = obj
            .get("note")
            .and_then(|v| v.as_str())
            .expect("stub must have a note");
        assert!(note.contains("Do NOT re-run"));
    }

    // ── eviction_preserves_message_and_block_counts ──────────────────────────

    #[test]
    fn eviction_preserves_message_and_block_counts() {
        // Same 9-pairs-plus-trailing-user shape as the recency test — one
        // eviction fires, and the message/block structure must be unchanged.
        let mut msgs = vec![ConversationMessage::user_text("start")];
        for i in 0..9 {
            let tu_id = format!("tu_{i}");
            msgs.push(ConversationMessage::assistant(vec![
                ContentBlock::ToolUse {
                    id: tu_id.clone(),
                    name: "tool".into(),
                    input: "{}".into(),
                },
            ]));
            msgs.push(ConversationMessage::tool_result(
                &tu_id,
                "tool",
                "x".repeat(600),
                false,
            ));
        }
        msgs.push(ConversationMessage::user_text("next"));

        let original_msg_count = msgs.len();
        let original_block_count: usize = msgs.iter().map(|m| m.blocks.len()).sum();

        let result = evict_stale_tool_outputs(&msgs, 2000, 60).expect("one eviction expected");

        assert_eq!(result.len(), original_msg_count);
        let new_block_count: usize = result.iter().map(|m| m.blocks.len()).sum();
        assert_eq!(new_block_count, original_block_count);

        // Only output strings changed — verify structure is identical.
        for (orig, evicted) in msgs.iter().zip(result.iter()) {
            assert_eq!(orig.role, evicted.role);
            assert_eq!(orig.blocks.len(), evicted.blocks.len());
            for (ob, eb) in orig.blocks.iter().zip(evicted.blocks.iter()) {
                assert_same_block_type(ob, eb);
            }
        }
    }

    fn assert_same_block_type(a: &ContentBlock, b: &ContentBlock) {
        match (a, b) {
            (ContentBlock::Text { .. }, ContentBlock::Text { .. }) => {}
            (ContentBlock::Image { .. }, ContentBlock::Image { .. }) => {}
            (ContentBlock::ToolUse { .. }, ContentBlock::ToolUse { .. }) => {}
            (ContentBlock::ToolResult { .. }, ContentBlock::ToolResult { .. }) => {}
            _ => panic!("block types differ"),
        }
    }
}
