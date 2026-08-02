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
use crate::executor::AgentToolExecutor;
use crate::model_config;
use crate::run::{build_runtime_streaming, build_runtime_with_brain, run_turn_with_retry};

type AgentRuntime = ConversationRuntime<OllamaApiClient, AgentToolExecutor>;

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

/// How far short of the iteration cap the "no text at max iter" signal
/// fires. The intent has always been "nearly out of iterations and still
/// nothing to say", expressed as a margin below the cap rather than an
/// absolute count — the original `11` was written against
/// `max_iterations = 15` and silently became a 27%-of-budget tripwire when
/// the cap moved to 40, diagnosing ordinary long tool chains as stalls and
/// replaying them wholesale on the bigger brain.
const MAX_ITER_STUCK_MARGIN: usize = 4;

/// Absolute floor for the stuck threshold, so a small `CLAUDETTE_MAX_ITERATIONS`
/// can't drive it down to "escalate on the second tool call".
const MAX_ITER_STUCK_FLOOR: usize = 11;

/// Iteration count at or above which a text-free turn counts as stuck,
/// derived from the cap actually in force.
fn max_iter_stuck_threshold() -> usize {
    crate::run::max_iterations()
        .saturating_sub(MAX_ITER_STUCK_MARGIN)
        .max(MAX_ITER_STUCK_FLOOR)
}

/// Minimum streak of consecutive `is_error` tool results before we treat
/// it as "the brain can't recover". Three is the threshold the Sprint 14
/// plan locked in — two in a row happens during normal path-guessing.
const TOOL_ERROR_STREAK_THRESHOLD: usize = 3;

/// Why an otherwise-configured fallback brain was not used for this turn.
///
/// The tiered-brain design assumes Ollama on a small card: escalating pulls
/// a second model into memory for one turn, then evicts it. That assumption
/// breaks badly on the setup the README now recommends for 16 GB — LM Studio
/// with one large model resident — because escalating asks the server to
/// JIT-load a *second* model, which evicts the brain the user deliberately
/// loaded. `~/.claudette/models.toml` does not exist on a fresh install, so
/// `Preset::Auto` supplies `fallback_brain = qwen3.5:9b` to every new user
/// whether or not they have ever heard of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FallbackSkip {
    /// The fallback resolves to the same model as the primary. Escalating
    /// would replay the whole turn against an identical brain for an
    /// identical result, at double the latency.
    SameAsPrimary,
    /// The backend does not serve the fallback model. Escalating would make
    /// the server fetch it: a JIT load (LM Studio — evicts the resident
    /// model) or a multi-gigabyte pull (Ollama). Neither is something to do
    /// behind the user's back mid-turn.
    NotServed,
}

impl FallbackSkip {
    fn explain(self, fallback: &str) -> String {
        match self {
            FallbackSkip::SameAsPrimary => {
                format!("fallback brain '{fallback}' is the same model as the primary")
            }
            FallbackSkip::NotServed => {
                format!("fallback brain '{fallback}' is not loaded on the backend")
            }
        }
    }
}

/// Decide whether escalation must be skipped. Pure — `served` is passed in
/// so this is unit-testable with no backend in the loop.
///
/// `served == None` (probe failed) and `served == Some(&[])` (backend
/// answered but listed nothing) both mean "can't tell", and both fail
/// **open**: an unreachable probe must not silently disable a working
/// fallback. The startup probe already refuses to launch against an
/// LM Studio with no model loaded, so the empty case is not load-bearing
/// here.
fn fallback_skip_reason(
    primary: &str,
    fallback: &str,
    served: Option<&[String]>,
) -> Option<FallbackSkip> {
    // Reuse doctor's loose matcher so `qwen3.5:9b` and `qwen3.5:9b:latest`
    // count as the same model on both sides of the comparison.
    if crate::doctor::model_present(&[primary.to_string()], fallback) {
        return Some(FallbackSkip::SameAsPrimary);
    }
    match served {
        Some(names) if !names.is_empty() && !crate::doctor::model_present(names, fallback) => {
            Some(FallbackSkip::NotServed)
        }
        _ => None,
    }
}

/// Model ids the backend is currently serving, fetched once per process.
///
/// Cached because this sits on the escalation path, which can fire on many
/// turns in a row. A cache that goes stale mid-session can only produce a
/// *skipped* escalation, never a surprise model load — the safe direction.
fn served_model_names() -> Option<&'static Vec<String>> {
    static CACHE: std::sync::OnceLock<Option<Vec<String>>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let openai_compat = crate::api::resolve_openai_compat();
            let base = crate::api::resolve_ollama_url();
            let url = if openai_compat {
                format!("{base}/v1/models")
            } else {
                format!("{base}/api/tags")
            };
            let client = crate::egress::local_http_builder()
                .timeout(std::time::Duration::from_secs(4))
                .build()
                .ok()?;
            let body = client
                .get(&url)
                .send()
                .ok()?
                .json::<serde_json::Value>()
                .ok()?;
            Some(crate::doctor::extract_model_names(&body, openai_compat))
        })
        .as_ref()
}

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
    runtime: &mut AgentRuntime,
    input: &str,
    prompter: &mut Option<&mut dyn PermissionPrompter>,
) -> Result<TurnSummary, String> {
    let fallback = model_config::active().fallback_brain;
    let Some(fallback_cfg) = fallback else {
        // No fallback configured — straight passthrough. Saves the
        // session clone on the hot path when fallback is disabled.
        return run_turn_with_retry(runtime, input, prompter_reborrow(prompter));
    };

    // Capture the primary model name BEFORE the turn — the same value the
    // runtime was built against, and the one we want to record in the
    // fallback log. The active config is the source of truth because
    // `ConversationRuntime` doesn't expose its api_client.
    let primary_model = model_config::active().brain.model.clone();

    // Decide up front whether escalation is even usable. Doing it here — and
    // not at the stuck-signal site — means an unusable fallback also skips
    // the pre-turn session clone below, so the passthrough stays as cheap as
    // the no-fallback-configured path.
    if let Some(skip) = fallback_skip_reason(
        &primary_model,
        &fallback_cfg.model,
        served_model_names().map(Vec::as_slice),
    ) {
        warn_fallback_disabled_once(skip, &fallback_cfg.model);
        return run_turn_with_retry(runtime, input, prompter_reborrow(prompter));
    }

    // Snapshot BEFORE letting the primary mutate the session. If we
    // escalate, we rewind from here so the fallback doesn't see a
    // duplicated user message or a stuck assistant turn.
    let pre_turn_session: Session = runtime.session().clone();

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
    let fallback_result =
        run_turn_with_retry(&mut fallback_runtime, input, prompter_reborrow(prompter));

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

/// Tell the user once per process that the configured fallback brain is
/// inert, and how to silence it. Once, not per turn: this fires on the
/// escalation path, which a genuinely struggling model can hit repeatedly,
/// and a repeated warning would bury the turn output it is attached to.
fn warn_fallback_disabled_once(skip: FallbackSkip, fallback: &str) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    eprintln!(
        "  \u{25B8} tiered brain off: {reason} — staying on the primary. \
         Set CLAUDETTE_FALLBACK_BRAIN_MODEL to a loaded model, or /preset fast to silence this.",
        reason = skip.explain(fallback),
    );
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
    // A graceful iteration-cap landing already carries a state-of-work
    // summary. Before it existed, the same turn was an `Err` that never
    // escalated — keep that: don't burn the fallback brain re-running a
    // 40-iteration turn whose tool-error streak is incidental to the cap.
    if summary.hit_iteration_cap {
        return None;
    }
    let text_blocks = count_text_blocks(&summary.assistant_messages);
    if text_blocks == 0 && summary.iterations >= max_iter_stuck_threshold() {
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
    crate::env_config::home_dir()
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
///
/// Skipped in OpenAI-compat mode: LM Studio (and other OpenAI-format
/// servers) don't honour the `keep_alive` extension. Eviction there is a
/// GUI/CLI action (`lms unload <model>`), out of scope for this helper.
fn unload_ollama_model(model: &str) {
    if crate::api::resolve_openai_compat() {
        return;
    }
    ollama_evict_model(model);
}

/// Unconditional Ollama eviction (no openai_compat short-circuit). Used by
/// the fallback-brain swap path where the caller has already decided the
/// brain is on Ollama.
fn ollama_evict_model(model: &str) {
    let host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".to_string());
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
            hit_iteration_cap: false,
            synthesized_reply: None,
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

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn fallback_to_the_same_model_is_skipped() {
        assert_eq!(
            fallback_skip_reason("qwen3.5:9b", "qwen3.5:9b", None),
            Some(FallbackSkip::SameAsPrimary),
        );
        // The loose matcher makes the `:latest` spelling the same model too.
        assert_eq!(
            fallback_skip_reason("qwen3.5:9b", "qwen3.5:9b:latest", None),
            Some(FallbackSkip::SameAsPrimary),
        );
    }

    #[test]
    fn fallback_not_loaded_on_the_backend_is_skipped() {
        // The shipped hazard: a fresh install has no ~/.claudette/models.toml,
        // so Preset::Auto hands every new user fallback_brain = qwen3.5:9b.
        // On the 16 GB LM Studio setup the README recommends, escalating
        // would JIT-load a second model and evict the resident champion.
        let served = names(&["qwen3.6-35b-a3b-mtp"]);
        assert_eq!(
            fallback_skip_reason("qwen3.6-35b-a3b-mtp", "qwen3.5:9b", Some(&served)),
            Some(FallbackSkip::NotServed),
        );
    }

    #[test]
    fn fallback_that_is_loaded_still_escalates() {
        // A user who deliberately loaded both models wants the tiered brain.
        let served = names(&["qwen3.5:4b", "qwen3.5:9b"]);
        assert_eq!(
            fallback_skip_reason("qwen3.5:4b", "qwen3.5:9b", Some(&served)),
            None,
        );
    }

    #[test]
    fn unknown_backend_inventory_fails_open() {
        // A probe that failed (None) or answered with nothing (empty) must
        // not silently disable a working fallback — the safe default here is
        // the previous behavior, since the caller only reaches this path
        // after a real stuck signal.
        assert_eq!(fallback_skip_reason("qwen3.5:4b", "qwen3.5:9b", None), None);
        assert_eq!(
            fallback_skip_reason("qwen3.5:4b", "qwen3.5:9b", Some(&[])),
            None,
        );
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

    // These pin the *relationship* to the iteration cap, not a magic number.
    // Hardcoding `13`/`8` is what let the threshold rot: they were written
    // against `max_iterations = 15` and kept passing after the cap moved to
    // 40, while the production heuristic had quietly become a 27%-of-budget
    // tripwire.
    #[test]
    fn diagnose_empty_text_at_max_iter_escalates() {
        let summary = make_summary(vec![], vec![], max_iter_stuck_threshold());
        assert_eq!(diagnose(&Ok(summary)), Some(StuckReason::NoTextAtMaxIter));
    }

    #[test]
    fn diagnose_empty_text_under_threshold_does_not_escalate() {
        let summary = make_summary(vec![], vec![], max_iter_stuck_threshold() - 1);
        assert_eq!(diagnose(&Ok(summary)), None);
    }

    #[test]
    fn stuck_threshold_tracks_the_iteration_cap() {
        let _guard = crate::test_env_lock();
        let prev = std::env::var("CLAUDETTE_MAX_ITERATIONS").ok();

        std::env::set_var("CLAUDETTE_MAX_ITERATIONS", "40");
        assert_eq!(max_iter_stuck_threshold(), 36, "cap 40 - margin 4");

        std::env::set_var("CLAUDETTE_MAX_ITERATIONS", "15");
        assert_eq!(
            max_iter_stuck_threshold(),
            11,
            "the historical cap/threshold pair"
        );

        // The floor keeps a tiny cap from turning the heuristic into
        // "escalate on the second tool call".
        std::env::set_var("CLAUDETTE_MAX_ITERATIONS", "6");
        assert_eq!(max_iter_stuck_threshold(), MAX_ITER_STUCK_FLOOR);

        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_MAX_ITERATIONS", v),
            None => std::env::remove_var("CLAUDETTE_MAX_ITERATIONS"),
        }
    }

    #[test]
    fn long_legitimate_tool_chain_is_not_diagnosed_as_stuck_at_the_default_cap() {
        // The regression this fix exists for: a 12-round tool chain that
        // hasn't emitted text yet is ordinary work at a cap of 40, but the
        // frozen `11` diagnosed it as a stall and replayed the entire turn
        // on the bigger brain — a 5-10s model swap plus a full re-run.
        let _guard = crate::test_env_lock();
        let prev = std::env::var("CLAUDETTE_MAX_ITERATIONS").ok();
        std::env::remove_var("CLAUDETTE_MAX_ITERATIONS");

        let summary = make_summary(vec![], vec![], 12);
        assert_eq!(diagnose(&Ok(summary)), None);

        if let Some(v) = prev {
            std::env::set_var("CLAUDETTE_MAX_ITERATIONS", v);
        }
    }

    #[test]
    fn diagnose_whitespace_only_text_counts_as_no_text() {
        let summary = make_summary(
            vec![ContentBlock::Text {
                text: "   \n ".into(),
            }],
            vec![],
            max_iter_stuck_threshold(),
        );
        assert_eq!(diagnose(&Ok(summary)), Some(StuckReason::NoTextAtMaxIter));
    }

    #[test]
    fn diagnose_skips_graceful_iteration_cap_landings() {
        // Pre-landing, a cap-hit turn was an Err that never escalated; the
        // graceful Ok (even with stuck-looking signals) must not start
        // burning the fallback brain on 40-iteration turns.
        let mut summary = make_summary(
            vec![],
            vec![tool_err(true), tool_err(true), tool_err(true)],
            13,
        );
        summary.hit_iteration_cap = true;
        assert_eq!(diagnose(&Ok(summary)), None);
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
            vec![
                tool_err(true),
                tool_err(true),
                tool_err(false),
                tool_err(true),
            ],
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
