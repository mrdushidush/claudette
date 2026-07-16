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
    let last_user_idx = messages.iter().enumerate().rev().find(|(_, m)| m.role == MessageRole::User).map(|(i, _)| i);

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

    for &(mi, bi) in &evictable {
        if current_estimate < threshold {
            break;
        }
        let stub = match &result[mi].blocks[bi] {
            ContentBlock::ToolResult {
                tool_name, output, ..
            } => stub_body(tool_name, output.len()),
            _ => continue, // safety: should always be ToolResult
        };
        let stub_len = stub.len();
        if let ContentBlock::ToolResult { output, .. } = &result[mi].blocks[bi] {
            current_estimate -= (output.len() - stub_len) / 4;
        }
        result[mi].blocks[bi] = ContentBlock::ToolResult {
            tool_use_id: match &result[mi].blocks[bi] {
                ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.clone(),
                _ => continue,
            },
            tool_name: match &result[mi].blocks[bi] {
                ContentBlock::ToolResult { tool_name, .. } => tool_name.clone(),
                _ => continue,
            },
            output: stub,
            is_error: match &result[mi].blocks[bi] {
                ContentBlock::ToolResult { is_error, .. } => *is_error,
                _ => continue,
            },
        };
    }

    if current_estimate < threshold {
        Some(result)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ConversationMessage};
    use std::sync::Mutex;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let msgs = vec![ConversationMessage::assistant(vec![])];
        assert!(evict_stale_tool_outputs(&msgs, 100_000, 60).is_none());
    }

    // ── current_turn_results_are_immune ─────────────────────────────────────

    #[test]
    fn current_turn_results_are_immune() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var(EVICT_ENV, "1");

        // Build: user msg → assistant with tool_use → huge stale-shaped result
        // placed AFTER the last user message. The boundary is the user index;
        // everything at or after it is immune.
        let msgs = vec![
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "read_file".into(),
                input: "{}".into(),
            }]),
            ConversationMessage::tool_result("t1", "read_file", "x".repeat(2048), false),
        ];

        // The tool result is at index 1, boundary (last user) doesn't exist → None.
        assert!(evict_stale_tool_outputs(&msgs, 100_000, 60).is_none());
    }

    // ── last_k_tool_results_are_immune ──────────────────────────────────────

    #[test]
    fn last_k_tool_results_are_immune() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var(EVICT_ENV, "1");

        // Build: user → assistant → 9 old tool results (each >512 chars)
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

        // 9 tool results → last 8 are immune. Only the oldest (index 0) is evictable.
        let result = evict_stale_tool_outputs(&msgs, 100_000, 60);
        assert!(result.is_some());
        let result = result.unwrap();

        // Check that only the first tool result was stubbed (index 8 in messages).
        // The last 8 should still have their original output.
        for i in 1..9 {
            let msg_idx = 2 * i; // user at 0, then assistant+tool pairs
            if let ContentBlock::ToolResult { output, .. } = &result[msg_idx].blocks[0] {
                assert!(
                    output.len() > MIN_EVICTABLE_CHARS,
                    "msg {} should not be stubbed",
                    msg_idx
                );
            }
        }

        // The first tool result (index 2) should be stubbed.
        if let ContentBlock::ToolResult { output, .. } = &result[2].blocks[0] {
            assert!(
                output.starts_with(STUB_MARKER),
                "first tool result should be stubbed"
            );
        } else {
            panic!("expected ToolResult at index 2");
        }
    }

    // ── evicts_oldest_first_and_stops_at_threshold ──────────────────────────

    #[test]
    fn evicts_oldest_first_and_stops_at_threshold() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var(EVICT_ENV, "1");

        // Two big stale results. Evicting the first should be enough to drop below threshold.
        let msgs = vec![
            ConversationMessage::user_text("start"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "tool_a".into(),
                input: "{}".into(),
            }]),
            ConversationMessage::tool_result("t1", "tool_a", "B".repeat(2048), false),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t2".into(),
                name: "tool_b".into(),
                input: "{}".into(),
            }]),
            ConversationMessage::tool_result("t2", "tool_b", "C".repeat(2048), false),
        ];

        let result = evict_stale_tool_outputs(&msgs, 100_000, 60);
        assert!(result.is_some());
        let result = result.unwrap();

        // First tool result should be stubbed.
        if let ContentBlock::ToolResult { output, .. } = &result[2].blocks[0] {
            assert!(output.starts_with(STUB_MARKER));
        } else {
            panic!("expected ToolResult at index 2");
        }

        // Second tool result should NOT be stubbed (eviction stopped after first).
        if let ContentBlock::ToolResult { output, .. } = &result[4].blocks[0] {
            assert!(
                !output.starts_with(STUB_MARKER),
                "second tool result should not be stubbed"
            );
        } else {
            panic!("expected ToolResult at index 4");
        }
    }

    // ── small_results_are_skipped ───────────────────────────────────────────

    #[test]
    fn small_results_are_skipped() {
        let msgs = vec![
            ConversationMessage::user_text("start"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "tool".into(),
                input: "{}".into(),
            }]),
            // Only 200 chars — below MIN_EVICTABLE_CHARS (512).
            ConversationMessage::tool_result("t1", "tool", "x".repeat(200), false),
        ];

        assert!(evict_stale_tool_outputs(&msgs, 100_000, 60).is_none());
    }

    // ── already_stubbed_results_are_skipped ──────────────────────────────────

    #[test]
    fn already_stubbed_results_are_skipped() {
        let msgs = vec![
            ConversationMessage::user_text("start"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "tool".into(),
                input: "{}".into(),
            }]),
            // Already stubbed — starts with STUB_MARKER.
            ConversationMessage::tool_result("t1", "tool", "x".repeat(2048), false),
        ];

        // Replace the output with a stub to simulate prior eviction.
        let mut msgs = msgs;
        if let ContentBlock::ToolResult { .. } = &msgs[2].blocks[0] {
            msgs[2].blocks[0] = ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                tool_name: "tool".into(),
                output: stub_body("tool", 2048),
                is_error: false,
            };
        }

        // The stubbed result should not be re-evicted.
        assert!(evict_stale_tool_outputs(&msgs, 100_000, 60).is_none());
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
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var(EVICT_ENV, "1");

        let mut msgs = vec![ConversationMessage::user_text("start")];
        for i in 0..3 {
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

        let original_msg_count = msgs.len();
        let original_block_count: usize = msgs.iter().map(|m| m.blocks.len()).sum();

        let result = evict_stale_tool_outputs(&msgs, 100_000, 60);
        assert!(result.is_some());
        let result = result.unwrap();

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
