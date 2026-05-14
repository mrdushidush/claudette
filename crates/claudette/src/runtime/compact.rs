use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionConfig {
    pub preserve_recent_messages: usize,
    pub max_estimated_tokens: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            preserve_recent_messages: 4,
            max_estimated_tokens: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionResult {
    pub summary: String,
    pub formatted_summary: String,
    pub compacted_session: Session,
    pub removed_message_count: usize,
}

#[must_use]
pub fn estimate_session_tokens(session: &Session) -> usize {
    session.messages.iter().map(estimate_message_tokens).sum()
}

#[must_use]
pub fn should_compact(session: &Session, config: CompactionConfig) -> bool {
    session.messages.len() > config.preserve_recent_messages
        && estimate_session_tokens(session) >= config.max_estimated_tokens
}

#[must_use]
pub fn format_compact_summary(summary: &str) -> String {
    let without_analysis = strip_tag_block(summary, "analysis");
    let formatted = if let Some(content) = extract_tag_block(&without_analysis, "summary") {
        without_analysis.replace(
            &format!("<summary>{content}</summary>"),
            &format!("Summary:\n{}", content.trim()),
        )
    } else {
        without_analysis
    };

    collapse_blank_lines(&formatted).trim().to_string()
}

#[must_use]
pub fn get_compact_continuation_message(
    summary: &str,
    suppress_follow_up_questions: bool,
    recent_messages_preserved: bool,
) -> String {
    let mut base = format!(
        "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n{}",
        format_compact_summary(summary)
    );

    if recent_messages_preserved {
        base.push_str("\n\nRecent messages are preserved verbatim.");
    }

    if suppress_follow_up_questions {
        base.push_str("\nContinue the conversation from where it left off without asking the user any further questions. Resume directly — do not acknowledge the summary, do not recap what was happening, and do not preface with continuation text.");
    }

    base
}

#[must_use]
pub fn compact_session(session: &Session, config: CompactionConfig) -> CompactionResult {
    if !should_compact(session, config) {
        return CompactionResult {
            summary: String::new(),
            formatted_summary: String::new(),
            compacted_session: session.clone(),
            removed_message_count: 0,
        };
    }

    let keep_from = session
        .messages
        .len()
        .saturating_sub(config.preserve_recent_messages);
    let removed = &session.messages[..keep_from];
    let preserved = session.messages[keep_from..].to_vec();
    let preserved = strip_orphaned_tool_results(preserved);
    let preserved = evict_older_image_bytes(preserved);
    let summary = summarize_messages(removed);
    let formatted_summary = format_compact_summary(&summary);
    let continuation = get_compact_continuation_message(&summary, true, !preserved.is_empty());

    let mut compacted_messages = vec![ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text { text: continuation }],
        usage: None,
    }];
    compacted_messages.extend(preserved);

    CompactionResult {
        summary,
        formatted_summary,
        compacted_session: Session {
            version: session.version,
            messages: compacted_messages,
        },
        removed_message_count: removed.len(),
    }
}

/// Remove orphaned tool_result messages that appear before any assistant message
/// with a matching tool_use. This prevents the LLM API from rejecting the
/// conversation with "unexpected tool_use_id found in tool_result blocks".
fn strip_orphaned_tool_results(mut messages: Vec<ConversationMessage>) -> Vec<ConversationMessage> {
    // Collect all tool_use IDs present in the preserved messages
    let mut tool_use_ids = std::collections::HashSet::new();
    for msg in &messages {
        if msg.role == MessageRole::Assistant {
            for block in &msg.blocks {
                if let ContentBlock::ToolUse { id, .. } = block {
                    tool_use_ids.insert(id.clone());
                }
            }
        }
    }

    // Remove any Tool-role messages whose tool_use_id isn't in our set
    messages.retain(|msg| {
        if msg.role == MessageRole::Tool {
            return msg.blocks.iter().all(|block| match block {
                ContentBlock::ToolResult { tool_use_id, .. } => tool_use_ids.contains(tool_use_id),
                _ => true,
            });
        }
        // Also strip orphaned tool_result blocks from assistant messages
        true
    });

    // Additionally, strip orphaned ToolResult blocks from within messages
    for msg in &mut messages {
        if msg.role == MessageRole::Assistant {
            msg.blocks.retain(|block| match block {
                ContentBlock::ToolResult { tool_use_id, .. } => tool_use_ids.contains(tool_use_id),
                _ => true,
            });
        }
    }

    // Remove any now-empty messages
    messages.retain(|msg| !msg.blocks.is_empty());

    messages
}

/// Replace `ContentBlock::Image` blocks with text placeholders in every
/// preserved message EXCEPT the most recent one that actually carries an
/// image. The most-recent visual context is kept verbatim (so a follow-up
/// like "look at that image again" still works); older base64 payloads
/// become short descriptions like `[image elided after compaction: image/png,
/// ~247KB base64 — earlier visual context summarised]`.
///
/// The reasoning, per [[image-attachment-context-bloat]]: once the assistant
/// has produced a reply about an image, the bytes are dead weight on every
/// subsequent turn until compaction evicts them. The earlier estimator-fix
/// slice made compaction TRIGGER correctly on image-heavy sessions; this
/// slice makes the compaction itself actually shrink the wire payload.
///
/// Returns the input unchanged if no message in the preserved tail carries
/// an image. A no-op for plain-text sessions.
fn evict_older_image_bytes(mut messages: Vec<ConversationMessage>) -> Vec<ConversationMessage> {
    let last_image_idx = messages.iter().enumerate().rev().find_map(|(i, m)| {
        if m.blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. }))
        {
            Some(i)
        } else {
            None
        }
    });

    let Some(keep_from) = last_image_idx else {
        return messages;
    };

    for (i, msg) in messages.iter_mut().enumerate() {
        if i >= keep_from {
            break;
        }
        for block in &mut msg.blocks {
            if let ContentBlock::Image {
                media_type,
                data_b64,
            } = block
            {
                let size_kb = data_b64.len() / 1024;
                *block = ContentBlock::Text {
                    text: format!(
                        "[image elided after compaction: {media_type}, ~{size_kb}KB base64 — \
                         earlier visual context summarised]"
                    ),
                };
            }
        }
    }

    messages
}

fn summarize_messages(messages: &[ConversationMessage]) -> String {
    let user_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .count();
    let assistant_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .count();
    let tool_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .count();

    let mut tool_names = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            ContentBlock::ToolResult { tool_name, .. } => Some(tool_name.as_str()),
            ContentBlock::Text { .. } | ContentBlock::Image { .. } => None,
        })
        .collect::<Vec<_>>();
    tool_names.sort_unstable();
    tool_names.dedup();

    let mut lines = vec![
        "<summary>".to_string(),
        "Conversation summary:".to_string(),
        format!(
            "- Scope: {} earlier messages compacted (user={}, assistant={}, tool={}).",
            messages.len(),
            user_messages,
            assistant_messages,
            tool_messages
        ),
    ];

    if !tool_names.is_empty() {
        lines.push(format!("- Tools mentioned: {}.", tool_names.join(", ")));
    }

    let recent_user_requests = collect_recent_role_summaries(messages, MessageRole::User, 3);
    if !recent_user_requests.is_empty() {
        lines.push("- Recent user requests:".to_string());
        lines.extend(
            recent_user_requests
                .into_iter()
                .map(|request| format!("  - {request}")),
        );
    }

    let pending_work = infer_pending_work(messages);
    if !pending_work.is_empty() {
        lines.push("- Pending work:".to_string());
        lines.extend(pending_work.into_iter().map(|item| format!("  - {item}")));
    }

    let key_files = collect_key_files(messages);
    if !key_files.is_empty() {
        lines.push(format!("- Key files referenced: {}.", key_files.join(", ")));
    }

    if let Some(current_work) = infer_current_work(messages) {
        lines.push(format!("- Current work: {current_work}"));
    }

    lines.push("- Key timeline:".to_string());
    for message in messages {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let content = message
            .blocks
            .iter()
            .map(summarize_block)
            .collect::<Vec<_>>()
            .join(" | ");
        lines.push(format!("  - {role}: {content}"));
    }
    lines.push("</summary>".to_string());
    lines.join("\n")
}

fn summarize_block(block: &ContentBlock) -> String {
    let raw = match block {
        ContentBlock::Text { text } => text.clone(),
        ContentBlock::Image { media_type, .. } => format!("[image {media_type}]"),
        ContentBlock::ToolUse { name, input, .. } => format!("tool_use {name}({input})"),
        ContentBlock::ToolResult {
            tool_name,
            output,
            is_error,
            ..
        } => format!(
            "tool_result {tool_name}: {}{output}",
            if *is_error { "error " } else { "" }
        ),
    };
    truncate_summary(&raw, 160)
}

fn collect_recent_role_summaries(
    messages: &[ConversationMessage],
    role: MessageRole,
    limit: usize,
) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message.role == role)
        .rev()
        .filter_map(|message| first_text_block(message))
        .take(limit)
        .map(|text| truncate_summary(text, 160))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn infer_pending_work(messages: &[ConversationMessage]) -> Vec<String> {
    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .filter(|text| {
            let lowered = text.to_ascii_lowercase();
            lowered.contains("todo")
                || lowered.contains("next")
                || lowered.contains("pending")
                || lowered.contains("follow up")
                || lowered.contains("remaining")
        })
        .take(3)
        .map(|text| truncate_summary(text, 160))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn collect_key_files(messages: &[ConversationMessage]) -> Vec<String> {
    let mut files = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .map(|block| match block {
            ContentBlock::Text { text } => text.as_str(),
            ContentBlock::ToolUse { input, .. } => input.as_str(),
            ContentBlock::ToolResult { output, .. } => output.as_str(),
            ContentBlock::Image { .. } => "",
        })
        .flat_map(extract_file_candidates)
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    files.into_iter().take(8).collect()
}

fn infer_current_work(messages: &[ConversationMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .find(|text| !text.trim().is_empty())
        .map(|text| truncate_summary(text, 200))
}

fn first_text_block(message: &ConversationMessage) -> Option<&str> {
    message.blocks.iter().find_map(|block| match block {
        ContentBlock::Text { text } if !text.trim().is_empty() => Some(text.as_str()),
        ContentBlock::ToolUse { .. }
        | ContentBlock::ToolResult { .. }
        | ContentBlock::Image { .. }
        | ContentBlock::Text { .. } => None,
    })
}

fn has_interesting_extension(candidate: &str) -> bool {
    std::path::Path::new(candidate)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["rs", "ts", "tsx", "js", "json", "md"]
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
}

fn extract_file_candidates(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter_map(|token| {
            let candidate = token.trim_matches(|char: char| {
                matches!(char, ',' | '.' | ':' | ';' | ')' | '(' | '"' | '\'' | '`')
            });
            if candidate.contains('/') && has_interesting_extension(candidate) {
                Some(candidate.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn truncate_summary(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut truncated = content.chars().take(max_chars).collect::<String>();
    truncated.push('…');
    truncated
}

fn estimate_message_tokens(message: &ConversationMessage) -> usize {
    message
        .blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.len() / 4 + 1,
            ContentBlock::ToolUse { name, input, .. } => (name.len() + input.len()) / 4 + 1,
            ContentBlock::ToolResult {
                tool_name, output, ..
            } => (tool_name.len() + output.len()) / 4 + 1,
            ContentBlock::Image { data_b64, .. } => estimate_image_tokens(data_b64),
        })
        .sum()
}

/// Estimate the per-turn token cost of one attached image.
///
/// The original v0.3.0 heuristic was a flat 256 tokens — meant as a
/// conservative ceiling for the model's vision-encoder cost on a single
/// 224×224 tile (Qwen, LLaVA, etc.). What it missed: the base64 payload
/// itself is sent on every subsequent turn until compaction evicts it, and
/// a typical screenshot's base64 is ~50–200 KB of JSON-resident chars. The
/// model's `num_ctx` budget is billed against those bytes too, so a flat
/// 256 wildly underestimates the wire cost on a 3-image session.
///
/// New estimate: `max(256, data_b64.len() / 4 + 1)`. The 256 floor still
/// covers the vision-encoder minimum; the `len/4` term tracks the wire
/// cost the same way text blocks already do (since base64 is just chars in
/// the request JSON).
#[inline]
fn estimate_image_tokens(data_b64: &str) -> usize {
    const IMAGE_TOKEN_FLOOR: usize = 256;
    let wire_cost = data_b64.len() / 4 + 1;
    if wire_cost > IMAGE_TOKEN_FLOOR {
        wire_cost
    } else {
        IMAGE_TOKEN_FLOOR
    }
}

fn extract_tag_block(content: &str, tag: &str) -> Option<String> {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    let start_index = content.find(&start)? + start.len();
    let end_index = content[start_index..].find(&end)? + start_index;
    Some(content[start_index..end_index].to_string())
}

fn strip_tag_block(content: &str, tag: &str) -> String {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    if let (Some(start_index), Some(end_index_rel)) = (content.find(&start), content.find(&end)) {
        let end_index = end_index_rel + end.len();
        let mut stripped = String::new();
        stripped.push_str(&content[..start_index]);
        stripped.push_str(&content[end_index..]);
        stripped
    } else {
        content.to_string()
    }
}

fn collapse_blank_lines(content: &str) -> String {
    let mut result = String::new();
    let mut last_blank = false;
    for line in content.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && last_blank {
            continue;
        }
        result.push_str(line);
        result.push('\n');
        last_blank = is_blank;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        collect_key_files, compact_session, estimate_image_tokens, estimate_session_tokens,
        evict_older_image_bytes, format_compact_summary, infer_pending_work, should_compact,
        CompactionConfig,
    };
    use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

    fn user_image(media: &str, b64_size_kb: usize) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Image {
                media_type: media.to_string(),
                data_b64: "A".repeat(b64_size_kb * 1024),
            }],
            usage: None,
        }
    }

    fn user_text(text: &str) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            usage: None,
        }
    }

    #[test]
    fn evict_older_image_bytes_passthrough_when_no_images() {
        let msgs = vec![user_text("hello"), user_text("how are you")];
        let out = evict_older_image_bytes(msgs.clone());
        assert_eq!(out, msgs);
    }

    #[test]
    fn evict_older_image_bytes_keeps_only_most_recent_image() {
        let msgs = vec![
            user_image("image/png", 100),
            user_text("about that screenshot"),
            user_image("image/jpeg", 200),
            user_text("any thoughts?"),
        ];
        let out = evict_older_image_bytes(msgs);

        // First message's image → placeholder text.
        assert!(matches!(
            &out[0].blocks[0],
            ContentBlock::Text { text } if text.starts_with("[image elided")
                && text.contains("image/png")
                && text.contains("100KB")
        ));
        // Most recent image (index 2) → still raw Image bytes.
        assert!(matches!(
            &out[2].blocks[0],
            ContentBlock::Image { media_type, .. } if media_type == "image/jpeg"
        ));
        // Plain-text messages untouched.
        assert!(matches!(
            &out[1].blocks[0],
            ContentBlock::Text { text } if text == "about that screenshot"
        ));
        assert!(matches!(
            &out[3].blocks[0],
            ContentBlock::Text { text } if text == "any thoughts?"
        ));
    }

    #[test]
    fn evict_older_image_bytes_keeps_single_image_intact() {
        // With only one image-bearing message, eviction is a no-op — that
        // image IS the most recent visual context and stays intact.
        let msgs = vec![
            user_text("look at this"),
            user_image("image/png", 50),
            user_text("what do you see?"),
        ];
        let out = evict_older_image_bytes(msgs.clone());
        assert_eq!(out, msgs);
    }

    #[test]
    fn evict_older_image_bytes_estimator_drops_after_eviction() {
        // The whole point of this helper: after compaction-time eviction,
        // the per-message token estimate for older image turns plummets
        // from the wire cost down to the placeholder string's length.
        let msgs = vec![user_image("image/png", 200), user_image("image/jpeg", 200)];
        let before: usize = msgs.iter().map(super::estimate_message_tokens).sum();
        let out = evict_older_image_bytes(msgs);
        let after: usize = out.iter().map(super::estimate_message_tokens).sum();
        // 200KB image = 51201 tokens via the estimator. Two of them = 102_402.
        // After eviction the older becomes a short placeholder (~24 tokens),
        // so total drops to ~51_225 — roughly half. The dominant savings
        // come from each evicted image, so this scales with how many older
        // image turns are in the session.
        assert!(
            after <= before / 2 + 100,
            "eviction should ~halve the token estimate: before={before} after={after}"
        );
    }

    #[test]
    fn image_token_estimator_floors_small_images_at_256() {
        // Tiny placeholder bytes — wire cost (12/4+1 = 4) is well below
        // the 256-token vision-encoder floor.
        assert_eq!(estimate_image_tokens("YWJjZGVmZ2hpams="), 256);
        // Empty payload still pays the floor: keeps callers safe from
        // accidentally producing zero-cost image blocks.
        assert_eq!(estimate_image_tokens(""), 256);
    }

    #[test]
    fn image_token_estimator_charges_wire_cost_for_real_payloads() {
        // 100 KB of base64 — a small PNG screenshot. Wire cost is
        // 100_000/4 + 1 = 25_001, well above the 256 floor. Pre-fix this
        // was 256 tokens — a ~97× undercount that hid image-heavy
        // sessions from the compaction trigger.
        let big = "A".repeat(100_000);
        assert_eq!(estimate_image_tokens(&big), 100_000 / 4 + 1);
    }

    #[test]
    fn image_token_estimator_transitions_at_the_floor() {
        // Right at the floor crossover point: 1024 chars → 256 +1 = 257
        // tokens at len/4+1, which beats the floor by 1. Pin the boundary
        // so future tuning doesn't accidentally swap the comparison.
        assert_eq!(estimate_image_tokens(&"A".repeat(1024)), 257);
        // One char under the crossover: floor wins.
        assert_eq!(estimate_image_tokens(&"A".repeat(1020)), 256);
    }

    #[test]
    fn formats_compact_summary_like_upstream() {
        let summary = "<analysis>scratch</analysis>\n<summary>Kept work</summary>";
        assert_eq!(format_compact_summary(summary), "Summary:\nKept work");
    }

    #[test]
    fn leaves_small_sessions_unchanged() {
        let session = Session {
            version: 1,
            messages: vec![ConversationMessage::user_text("hello")],
        };

        let result = compact_session(&session, CompactionConfig::default());
        assert_eq!(result.removed_message_count, 0);
        assert_eq!(result.compacted_session, session);
        assert!(result.summary.is_empty());
        assert!(result.formatted_summary.is_empty());
    }

    #[test]
    fn compacts_older_messages_into_a_system_summary() {
        let session = Session {
            version: 1,
            messages: vec![
                ConversationMessage::user_text("one ".repeat(200)),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "two ".repeat(200),
                }]),
                ConversationMessage::tool_result("1", "bash", "ok ".repeat(200), false),
                ConversationMessage {
                    role: MessageRole::Assistant,
                    blocks: vec![ContentBlock::Text {
                        text: "recent".to_string(),
                    }],
                    usage: None,
                },
            ],
        };

        let result = compact_session(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
            },
        );

        assert_eq!(result.removed_message_count, 2);
        assert_eq!(
            result.compacted_session.messages[0].role,
            MessageRole::System
        );
        assert!(matches!(
            &result.compacted_session.messages[0].blocks[0],
            ContentBlock::Text { text } if text.contains("Summary:")
        ));
        assert!(result.formatted_summary.contains("Scope:"));
        assert!(result.formatted_summary.contains("Key timeline:"));
        assert!(should_compact(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
            }
        ));
        assert!(
            estimate_session_tokens(&result.compacted_session) < estimate_session_tokens(&session)
        );
    }

    #[test]
    fn truncates_long_blocks_in_summary() {
        let summary = super::summarize_block(&ContentBlock::Text {
            text: "x".repeat(400),
        });
        assert!(summary.ends_with('…'));
        assert!(summary.chars().count() <= 161);
    }

    #[test]
    fn extracts_key_files_from_message_content() {
        let files = collect_key_files(&[ConversationMessage::user_text(
            "Update rust/crates/runtime/src/compact.rs and rust/crates/runtime/src/main.rs next.",
        )]);
        assert!(files.contains(&"rust/crates/runtime/src/compact.rs".to_string()));
        assert!(files.contains(&"rust/crates/runtime/src/main.rs".to_string()));
    }

    #[test]
    fn infers_pending_work_from_recent_messages() {
        let pending = infer_pending_work(&[
            ConversationMessage::user_text("done"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "Next: update tests and follow up on remaining CLI polish.".to_string(),
            }]),
        ]);
        assert_eq!(pending.len(), 1);
        assert!(pending[0].contains("Next: update tests"));
    }
}
