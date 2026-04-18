//! Sprint 14 — tiered brain fallback.
//!
//! Wraps a single turn against a `ConversationRuntime` with the logic:
//!
//! 1. Snapshot the session before the turn.
//! 2. Run the primary brain via the existing `run_turn_with_retry` (so the
//!    empty-response `EMPTY_RESPONSE_NUDGE` still fires).
//! 3. Inspect the outcome for three strict "stuck" signals:
//!    - `Err("no content")` even after the retry nudge
//!    - Ok summary with zero assistant text blocks at/near `max_iterations`
//!    - `≥3` consecutive `is_error = true` entries inside `tool_results`
//! 4. If stuck and `model_config::active().fallback_brain.is_some()`:
//!    - Build a fresh runtime around the fallback model + the pre-turn
//!      session snapshot
//!    - Replay the same user input on the fallback
//!    - Swap the caller's runtime pointer to the fallback-advanced session
//!      (per-turn revert: the next turn goes back to the primary)
//!    - Append a JSONL record to `~/.claudette/fallback.jsonl`
//! 5. Otherwise return the primary result verbatim.
//!
//! Why the strict signals:
//! The 4b brain is fast and VRAM-cheap but occasionally stalls on
//! multi-step tool chains. Every fallback costs a `~5-10s` model swap
//! (4b → 9b → 4b revert). False positives waste swap time; false negatives
//! leak bad output. The three signals above are the ones the brain200
//! transcripts showed produce true-positive escalation candidates.

use std::io::{Read, Write};
use std::path::PathBuf;

use crate::{ContentBlock, ConversationRuntime, PermissionPrompter, Session, TurnSummary};

use crate::api::OllamaApiClient;
use crate::executor::SecretaryToolExecutor;
use crate::model_config;
use crate::run::{build_runtime_streaming, build_runtime_with_brain, run_turn_with_retry};

type SecretaryRuntime = ConversationRuntime<OllamaApiClient, SecretaryToolExecutor>;

/// Why we decided a primary-brain turn was stuck. Logged to
/// `fallback.jsonl` so we can tune the thresholds against real data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StuckReason {
    /// `run_turn_with_retry` returned `Err` whose message contains
    /// "no content" — the model produced an empty response AND the
    /// nudge-retry also produced an empty response.
    EmptyResponse,
    /// The summary came back with no text content blocks and a high
    /// iteration count — the tool loop burnt through max iterations
    /// without the model ever answering in natural language.
    NoTextAtMaxIter,
    /// Three or more tool calls in a row returned `is_error = true`.
    /// Sign that the brain can't recover from a bad tool call.
    ToolErrorStreak,
}

impl StuckReason {
    /// Short tag used in the JSONL log `trigger` field.
    fn tag(self) -> &'static str {
        match self {
            StuckReason::EmptyResponse => "empty_response",
            StuckReason::NoTextAtMaxIter => "no_text_at_max_iter",
            StuckReason::ToolErrorStreak => "tool_error_streak",
        }
    }
}

/// Iteration-count heuristic for the "no text at max iter" signal. If the
/// primary runtime ran at least this many iterations AND produced no text,
/// treat it as stuck. `11` is two short of the configured `max_iterations
/// = 15` in `build_runtime_with_brain` — catches the real stalls
/// (long tool chains that never emit final text) without firing on
/// ordinary single-tool turns.
const MAX_ITER_STUCK_THRESHOLD: usize = 11;

/// Minimum streak of consecutive `is_error` tool results before we treat
/// it as "the brain can't recover". Three is the threshold the Sprint 14
/// plan locked in — two in a row happens during normal path-guessing.
const TOOL_ERROR_STREAK_THRESHOLD: usize = 3;

/// Run a turn with automatic 4b → fallback → revert escalation.
///
/// When `model_config::active().fallback_brain` is `None` (presets Fast
/// and Smart, or after `/brain <pin>`), this is a straight passthrough to
/// `run_turn_with_retry` — no overhead.
///
/// When fallback is enabled (preset Auto, the default), runs the primary,
/// inspects for stuck signals, and escalates if needed. The caller's
/// `runtime` pointer is mutated in place so the next turn starts from
/// whatever session state we ended up with.
pub fn run_turn_with_fallback(
    runtime: &mut SecretaryRuntime,
    input: &str,
    prompter: &mut Option<&mut dyn PermissionPrompter>,
) -> Result<TurnSummary, String> {
    let fallback = model_config::active().fallback_brain;
    let Some(fallback_cfg) = fallback else {
        // No fallback configured — straight passthrough. Saves the
        // session clone on the hot path when fallback is disabled.
        return run_turn_with_retry(runtime, input, prompter_reborrow(prompter));
    };

    // Snapshot BEFORE letting the primary mutate the session. If we
    // escalate, we rewind from here so the fallback doesn't see a
    // duplicated user message or a stuck assistant turn.
    let pre_turn_session: Session = runtime.session().clone();

    // Capture the primary model name BEFORE the turn — the same value the
    // runtime was built against, and the one we want to record in the
    // fallback log. The active config is the source of truth because
    // `ConversationRuntime` doesn't expose its api_client.
    let primary_model = model_config::active().brain.model.clone();

    // Scope each reborrow to a short-lived block so its lifetime ends
    // before the next `run_turn_with_retry` call needs a fresh one.
    // Passing the outer `&mut Option<&mut dyn P>` lets us reborrow the
    // inner reference twice (once for primary, once for fallback).
    let primary_result = run_turn_with_retry(runtime, input, prompter_reborrow(prompter));

    let stuck = diagnose(&primary_result);

    let Some(reason) = stuck else {
        return primary_result;
    };

    eprintln!(
        "  \u{25B8} brain stuck ({tag}) on {model} — escalating to {fallback}...",
        tag = reason.tag(),
        model = primary_model,
        fallback = fallback_cfg.model,
    );

    let mut fallback_runtime =
        build_runtime_with_brain(pre_turn_session, &fallback_cfg, true, false);
    let fallback_result = run_turn_with_retry(
        &mut fallback_runtime,
        input,
        prompter_reborrow(prompter),
    );

    // Release the fallback model from Ollama's VRAM/RAM budget before
    // handing control back to the primary. Without this, Ollama keeps 9b
    // resident past its default keep_alive (5m) even under
    // OLLAMA_MAX_LOADED_MODELS=1, and subsequent 30b coder loads on
    // 8 GB VRAM / 32 GB RAM boxes fail with "model requires more system
    // memory (11.7 GiB) than is available".
    unload_ollama_model(&fallback_cfg.model);

    // Per-turn revert: swap `runtime` back to the primary brain so the
    // *next* turn starts fresh on 4b. We pass the fallback's advanced
    // session forward so conversation continuity is preserved.
    let forward_session = fallback_runtime.session().clone();
    *runtime = build_runtime_streaming(forward_session, false);

    append_fallback_event(FallbackEvent {
        prompt: input,
        trigger: reason.tag(),
        primary_model: &primary_model,
        fallback_model: &fallback_cfg.model,
        succeeded: fallback_result.is_ok(),
    });

    fallback_result
}

/// Inspect a primary-brain turn result for stuck signals. Returns `Some`
/// if the fallback should fire. Pure function — no side effects, so it
/// can be unit-tested without a real Ollama in the loop.
#[must_use]
pub fn diagnose(result: &Result<TurnSummary, String>) -> Option<StuckReason> {
    match result {
        Err(msg) if msg.contains("no content") => Some(StuckReason::EmptyResponse),
        Err(_) => None, // Transport errors, permission denials — don't escalate.
        Ok(summary) => diagnose_summary(summary),
    }
}

fn diagnose_summary(summary: &TurnSummary) -> Option<StuckReason> {
    let text_blocks = count_text_blocks(&summary.assistant_messages);
    if text_blocks == 0 && summary.iterations >= MAX_ITER_STUCK_THRESHOLD {
        return Some(StuckReason::NoTextAtMaxIter);
    }
    if max_consecutive_tool_errors(&summary.tool_results) >= TOOL_ERROR_STREAK_THRESHOLD {
        return Some(StuckReason::ToolErrorStreak);
    }
    None
}

fn count_text_blocks(msgs: &[crate::ConversationMessage]) -> usize {
    msgs.iter()
        .flat_map(|m| &m.blocks)
        .filter(|b| {
            if let ContentBlock::Text { text } = b {
                !text.trim().is_empty()
            } else {
                false
            }
        })
        .count()
}

/// Reborrow the outer `&mut Option<&mut dyn PermissionPrompter>` to a
/// short-lived `Option<&mut dyn PermissionPrompter>` suitable for a
/// single `run_turn_with_retry` call. Two lifetimes are required to
/// decouple the outer borrow (`'a`, per-call) from the inner reference's
/// lifetime (`'b`, the caller's). Each call to `prompter_reborrow` takes
/// a short `'a`-scoped borrow so the next call is free.
fn prompter_reborrow<'a, 'b>(
    p: &'a mut Option<&'b mut dyn PermissionPrompter>,
) -> Option<&'a mut dyn PermissionPrompter>
where
    'b: 'a,
{
    match p {
        Some(r) => {
            let shortened: &'a mut dyn PermissionPrompter = &mut **r;
            Some(shortened)
        }
        None => None,
    }
}

fn max_consecutive_tool_errors(msgs: &[crate::ConversationMessage]) -> usize {
    let mut consec = 0usize;
    let mut max_run = 0usize;
    for msg in msgs {
        for block in &msg.blocks {
            if let ContentBlock::ToolResult { is_error, .. } = block {
                if *is_error {
                    consec += 1;
                    if consec > max_run {
                        max_run = consec;
                    }
                } else {
                    consec = 0;
                }
            }
        }
    }
    max_run
}


// ─── Fallback event logging ─────────────────────────────────────────────────

struct FallbackEvent<'a> {
    prompt: &'a str,
    trigger: &'a str,
    primary_model: &'a str,
    fallback_model: &'a str,
    succeeded: bool,
}

/// Path for the fallback event log: `~/.claudette/fallback.jsonl`.
#[must_use]
pub fn fallback_log_path() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".claudette")
        .join("fallback.jsonl")
}

fn append_fallback_event(ev: FallbackEvent<'_>) {
    let path = fallback_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Hand-rolled JSON — everything we write is ASCII-safe. Avoids a
    // serde_json::to_string call for a one-line record. Quotes and
    // backslashes are escaped for safety even though `trigger` and
    // model names never contain them.
    let ts = chrono::Utc::now().to_rfc3339();
    let line = format!(
        "{{\"ts\":\"{}\",\"prompt_hash\":\"{}\",\"trigger\":\"{}\",\"fallback_succeeded\":{},\"primary_model\":\"{}\",\"fallback_model\":\"{}\"}}\n",
        ts,
        prompt_hash(ev.prompt),
        escape_json(ev.trigger),
        ev.succeeded,
        escape_json(ev.primary_model),
        escape_json(ev.fallback_model),
    );

    // Best-effort append. If the write fails we've already surfaced the
    // fallback result to the user — eat the error rather than polluting
    // their turn output with a noisy log warning.
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Stable short hash for the prompt — used by the JSONL log so we can
/// group "how often does THIS prompt trigger fallback". `DefaultHasher`
/// is not cryptographically stable across Rust releases, but we only
/// need stability within a single binary build, so the convenience wins.
fn prompt_hash(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Best-effort `POST /api/chat` with `keep_alive: 0` to tell Ollama to
/// evict `model` from memory immediately. Mirrors `voice.rs`'s unload
/// trick. Silently ignores failures — if Ollama is down the next chat
/// turn will surface a clearer error than this helper could.
fn unload_ollama_model(model: &str) {
    let host = std::env::var("OLLAMA_HOST")
        .unwrap_or_else(|_| "http://localhost:11434".to_string());
    let _ = reqwest::blocking::Client::new()
        .post(format!("{host}/api/chat"))
        .json(&serde_json::json!({
            "model": model,
            "keep_alive": 0,
        }))
        .send();
}

fn escape_json(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Read the last N lines from the fallback log, newest last. Used by
/// future diagnostic commands (not wired yet) and by tests.
#[must_use]
pub fn read_tail(limit: usize) -> Vec<String> {
    let path = fallback_log_path();
    let Ok(mut file) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return Vec::new();
    }
    let mut lines: Vec<String> = buf
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect();
    if lines.len() > limit {
        lines = lines.split_off(lines.len() - limit);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentBlock, ConversationMessage, MessageRole, TokenUsage};

    fn make_summary(
        assistant: Vec<ContentBlock>,
        tool_results: Vec<ContentBlock>,
        iterations: usize,
    ) -> TurnSummary {
        TurnSummary {
            assistant_messages: vec![ConversationMessage {
                role: MessageRole::Assistant,
                blocks: assistant,
                usage: None,
            }],
            tool_results: tool_results
                .into_iter()
                .map(|b| ConversationMessage {
                    role: MessageRole::Tool,
                    blocks: vec![b],
                    usage: None,
                })
                .collect(),
            iterations,
            usage: TokenUsage::default(),
            auto_compaction: None,
        }
    }

    fn tool_err(is_error: bool) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: "id".into(),
            tool_name: "note_list".into(),
            output: "whatever".into(),
            is_error,
        }
    }

    #[test]
    fn diagnose_empty_response_from_err_message() {
        let r: Result<TurnSummary, String> = Err("no content in response".to_string());
        assert_eq!(diagnose(&r), Some(StuckReason::EmptyResponse));
    }

    #[test]
    fn diagnose_transport_error_does_not_escalate() {
        let r: Result<TurnSummary, String> = Err("connection refused".to_string());
        assert_eq!(diagnose(&r), None);
    }

    #[test]
    fn diagnose_text_response_passes_through() {
        let summary = make_summary(
            vec![ContentBlock::Text {
                text: "here is your answer".into(),
            }],
            vec![],
            4,
        );
        assert_eq!(diagnose(&Ok(summary)), None);
    }

    #[test]
    fn diagnose_empty_text_at_max_iter_escalates() {
        let summary = make_summary(vec![], vec![], 13);
        assert_eq!(diagnose(&Ok(summary)), Some(StuckReason::NoTextAtMaxIter));
    }

    #[test]
    fn diagnose_empty_text_under_threshold_does_not_escalate() {
        let summary = make_summary(vec![], vec![], 8);
        assert_eq!(diagnose(&Ok(summary)), None);
    }

    #[test]
    fn diagnose_whitespace_only_text_counts_as_no_text() {
        let summary = make_summary(
            vec![ContentBlock::Text {
                text: "   \n ".into(),
            }],
            vec![],
            12,
        );
        assert_eq!(diagnose(&Ok(summary)), Some(StuckReason::NoTextAtMaxIter));
    }

    #[test]
    fn diagnose_three_consecutive_tool_errors_escalates() {
        let summary = make_summary(
            vec![ContentBlock::Text {
                text: "trying tools".into(),
            }],
            vec![tool_err(true), tool_err(true), tool_err(true)],
            4,
        );
        assert_eq!(diagnose(&Ok(summary)), Some(StuckReason::ToolErrorStreak));
    }

    #[test]
    fn diagnose_two_errors_then_success_does_not_escalate() {
        let summary = make_summary(
            vec![ContentBlock::Text {
                text: "okay".into(),
            }],
            vec![tool_err(true), tool_err(true), tool_err(false), tool_err(true)],
            4,
        );
        assert_eq!(diagnose(&Ok(summary)), None);
    }

    #[test]
    fn diagnose_interleaved_errors_resets_streak() {
        let summary = make_summary(
            vec![ContentBlock::Text { text: "ok".into() }],
            vec![
                tool_err(true),
                tool_err(true),
                tool_err(false), // resets
                tool_err(true),
                tool_err(true), // only 2 in a row after reset
            ],
            4,
        );
        assert_eq!(diagnose(&Ok(summary)), None);
    }

    #[test]
    fn escape_json_handles_specials() {
        assert_eq!(escape_json("hello"), "hello");
        assert_eq!(escape_json("a\"b"), "a\\\"b");
        assert_eq!(escape_json("a\\b"), "a\\\\b");
        assert_eq!(escape_json("a\nb"), "a\\nb");
    }

    #[test]
    fn prompt_hash_is_stable_for_same_input() {
        let a = prompt_hash("what time is it?");
        let b = prompt_hash("what time is it?");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn prompt_hash_differs_for_different_inputs() {
        assert_ne!(prompt_hash("a"), prompt_hash("b"));
    }
}
