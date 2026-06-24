//! Cross-session recall hooks (Wave C2 — split out of run.rs).
//!
//! The post-turn recall indexer (a background mpsc thread that embeds each
//! turn's user/assistant snippets), the startup embed probe, and the sticky
//! "indexing broke" flag. Foreground entry point: `index_turn_for_recall`; the
//! runtime-mutating REPL calls it after each successful turn.

use crate::theme;
use crate::ConversationRuntime;

/// Whether the post-turn recall indexing is disabled. Off-by-default
/// privacy/perf escape hatch: `CLAUDETTE_RECALL_DISABLE=1`. Anything else
/// (unset, "0", garbage) leaves indexing enabled.
pub(crate) fn recall_disabled() -> bool {
    matches!(
        std::env::var("CLAUDETTE_RECALL_DISABLE").as_deref(),
        Ok("1")
    )
}

/// Sticky session-scoped flag: once recall indexing fails (e.g. LM Studio
/// has no embed model loaded), every subsequent turn would re-fail with
/// identical noise. After the first failure we set this and silently skip
/// the indexing call until the process restarts. The user gets ONE warning
/// at the first failure with instructions for fixing it (load the model
/// or set `CLAUDETTE_RECALL_DISABLE=1`).
static RECALL_INDEX_BROKEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(crate) fn recall_index_allowed() -> bool {
    !recall_disabled() && !RECALL_INDEX_BROKEN.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn mark_recall_index_broken() {
    RECALL_INDEX_BROKEN.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Clear the sticky `RECALL_INDEX_BROKEN` flag and re-run the startup
/// embed probe. Exposed via the `/recall reprobe` slash command so the
/// user can recover from a mid-session embed failure (e.g. LM Studio
/// just loaded the embed model) without restarting the process. Returns
/// the probe's own `Result` so the slash handler can format a success/
/// failure message.
pub fn reprobe_recall() -> Result<(), String> {
    RECALL_INDEX_BROKEN.store(false, std::sync::atomic::Ordering::Relaxed);
    crate::recall::probe()
}

/// Pre-flight the recall embedder by running a tiny embed call at REPL/TUI
/// startup. On failure (e.g. LM Studio's "No models loaded" 400), set the
/// sticky-disable flag and print one clear warn line. Silent on success so
/// healthy startups stay quiet.
///
/// Called once per process. Honors `CLAUDETTE_RECALL_DISABLE=1` — if recall
/// is already opted out, the probe is a no-op (we don't want to wake the
/// store at all in privacy mode).
pub(crate) fn probe_recall_at_startup() {
    if recall_disabled() {
        return;
    }
    if let Err(e) = crate::recall::probe() {
        mark_recall_index_broken();
        eprintln!(
            "{} {}",
            theme::warn(theme::WARN_GLYPH),
            theme::warn(&format!(
                "recall: probe failed — {e}. Indexing disabled for this session \
                 (load an embed model and restart, or set CLAUDETTE_RECALL_DISABLE=1 to silence)."
            ))
        );
    }
}

/// Extract the (user, assistant) snippets for one turn — pure CPU,
/// returns owned strings so callers can enqueue them onto the async
/// indexer channel without holding a borrow on the runtime. Empty
/// snippets stay empty (the indexer thread skips them).
///
/// Why we pass `user_input` directly instead of walking back to find the
/// "latest user message": on retries, the runtime injects a synthetic
/// nudge user-message into the session (see [`run_turn_with_retry`]). The
/// raw `trimmed` REPL line is what the human actually typed, so we
/// index that and skip the synthetic.
fn extract_turn_snippets<C, T>(
    user_input: &str,
    runtime: &ConversationRuntime<C, T>,
) -> (String, String)
where
    C: crate::ApiClient,
    T: crate::ToolExecutor,
{
    use crate::ContentBlock;
    let user_text = user_input.trim().to_string();
    let mut asst_text = String::new();
    if let Some(msg) = runtime
        .session()
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, crate::MessageRole::Assistant))
    {
        for block in &msg.blocks {
            if let ContentBlock::Text { text: t } = block {
                if !asst_text.is_empty() {
                    asst_text.push('\n');
                }
                asst_text.push_str(t);
            }
        }
    }
    (user_text, asst_text)
}

/// One job for the background recall indexer.
struct IndexJob {
    role: crate::recall::Role,
    snippet: String,
}

/// Lazily-spawned mpsc channel for the recall indexer thread. The Sender
/// is cloned on every push; the Receiver is owned by the one worker
/// thread spawned on first use. Channel-close (last Sender dropped at
/// process exit) terminates the thread cleanly.
fn recall_index_sender() -> &'static std::sync::mpsc::Sender<IndexJob> {
    use std::sync::OnceLock;
    static SENDER: OnceLock<std::sync::mpsc::Sender<IndexJob>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<IndexJob>();
        std::thread::Builder::new()
            .name("recall-indexer".to_string())
            .spawn(move || {
                // Drain until the channel closes. Each failed embed call
                // sets the sticky-disable flag and logs once; subsequent
                // jobs that slip through (in flight before the flag flipped)
                // also fail-fast on the same flag check.
                while let Ok(job) = rx.recv() {
                    if !recall_index_allowed() {
                        continue;
                    }
                    if job.snippet.trim().is_empty() {
                        continue;
                    }
                    if let Err(e) = crate::recall::global_index(job.role, &job.snippet) {
                        mark_recall_index_broken();
                        eprintln!(
                            "{} {}",
                            theme::warn(theme::WARN_GLYPH),
                            theme::warn(&format!(
                                "recall: {e} — disabling recall indexing for this session \
                                 (run /recall reprobe to retry after loading the embed model)"
                            ))
                        );
                    }
                }
            })
            .expect("spawn recall-indexer thread");
        tx
    })
}

/// Enqueue this turn's (user, assistant) snippets for async indexing.
/// Cheap (one channel push per snippet) — the embed call itself happens
/// on the background thread spawned by [`recall_index_sender`]. This is
/// the foreground entry point the REPL/TUI/Telegram all hit after a
/// successful turn.
///
/// Pre-2026-05-15 the embed call ran synchronously here, blocking the
/// REPL ~100 ms typical and seconds on a cold embed model. Moving it
/// behind a channel restores per-turn latency to what the user sees on
/// the streamed brain text.
pub(crate) fn index_turn_for_recall<C, T>(user_input: &str, runtime: &ConversationRuntime<C, T>)
where
    C: crate::ApiClient,
    T: crate::ToolExecutor,
{
    let (user_text, asst_text) = extract_turn_snippets(user_input, runtime);
    let tx = recall_index_sender();
    if !user_text.is_empty() {
        let _ = tx.send(IndexJob {
            role: crate::recall::Role::User,
            snippet: user_text,
        });
    }
    if !asst_text.trim().is_empty() {
        let _ = tx.send(IndexJob {
            role: crate::recall::Role::Assistant,
            snippet: asst_text,
        });
    }
}
