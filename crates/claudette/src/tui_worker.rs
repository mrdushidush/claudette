//! Worker thread for the TUI.
//!
//! Owns the `ConversationRuntime` (which uses blocking reqwest), receives
//! `UserInput` commands from the render loop, and fires `TuiEvent`s back.
//!
//! Written as a self-contained module so it can build its own runtime with
//! `TuiToolExecutor` injected — the existing `build_runtime_streaming` in
//! `run.rs` is typed to `SecretaryToolExecutor` and is left untouched.

use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use crate::{
    compact_session, estimate_session_tokens, CompactionConfig, ContentBlock, ConversationRuntime,
    Session,
};

use crate::api::{tui_text_callback, OllamaApiClient};
use crate::commands::{
    dispatch_slash_command, parse_slash_command, ReplState, SlashCommand, SlashOutcome,
};
use crate::executor::SecretaryToolExecutor;
use crate::memory::try_load_memory;
use crate::prompt::secretary_system_prompt_with_memory;
use crate::run::{
    build_permission_policy, compact_threshold, current_model, index_turn_for_recall,
    probe_recall_at_startup, recall_index_allowed, save_session,
};
use crate::tool_groups::{ToolGroup, ToolRegistry};
use crate::tui_events::{TuiEvent, UserInput};
use crate::tui_executor::TuiToolExecutor;

/// Short alias so function signatures stay readable.
type TuiRuntime = ConversationRuntime<OllamaApiClient, TuiToolExecutor>;

/// Build a runtime with `TuiToolExecutor` + TUI text callback.
///
/// Ships only the core tools (`enable_tools` + `get_current_time`) — every
/// other group must be opted into via `enable_tools`. Pre-rewrite this
/// auto-enabled five groups (Markets/Facts/Advanced/Git/Search), which
/// pushed the per-turn payload to ~2,500 tokens. Now ~200.
///
/// Uses the same per-tool permission policy as the REPL so `ReadOnly` +
/// `WorkspaceWrite` tools pass through. `DangerFullAccess` tools are denied
/// (no prompter yet). Sprint G will add `TuiPrompter` for confirmation modals.
fn build_tui_runtime(session: Session, tui_tx: SyncSender<TuiEvent>) -> TuiRuntime {
    let reg = ToolRegistry::new();
    let registry = Arc::new(Mutex::new(reg));

    let api_client = OllamaApiClient::with_registry(current_model(), registry.clone())
        .with_text_callback(tui_text_callback(tui_tx.clone()));

    let hinter_registry = Arc::clone(&registry);
    let inner = SecretaryToolExecutor::with_registry(registry);
    let executor = TuiToolExecutor::new(inner, tui_tx);

    let policy = build_permission_policy();
    let memory = try_load_memory();

    ConversationRuntime::new(
        session,
        api_client,
        executor,
        policy,
        secretary_system_prompt_with_memory(memory.as_deref(), false),
    )
    .with_max_iterations(crate::run::max_iterations())
    .with_auto_compaction_input_tokens_threshold(u32::MAX)
    .with_unknown_tool_hinter(move |name: &str| {
        ToolGroup::parse(name).map_or_else(Vec::new, |group| {
            let reg = match hinter_registry.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            reg.group_tool_names(group)
        })
    })
}

/// Run a slash command typed in the TUI through the shared dispatcher.
///
/// `cmd` is the keyword **without** the leading `/` (the TUI input layer
/// strips it before dispatching). We re-prepend `/` so we can reuse the
/// canonical [`parse_slash_command`] from the REPL side.
///
/// Captures the dispatcher's textual output into a buffer and ships it as a
/// [`TuiEvent::Info`] system message. For commands that swap the session
/// out from under the runtime (`/clear`, `/load`), we additionally emit
/// [`TuiEvent::SessionReset`] so the visible chat history wipes.
fn handle_tui_slash(
    cmd: &str,
    runtime: &mut TuiRuntime,
    tui_tx: &SyncSender<TuiEvent>,
) -> SlashOutcome {
    let line = format!("/{}", cmd.trim());
    let Some(parsed) = parse_slash_command(&line) else {
        // parse_slash_command only returns None for non-slash input; we just
        // prepended `/` so this branch is unreachable in practice. Still
        // worth reporting cleanly instead of panicking.
        let _ = tui_tx.send(TuiEvent::TurnError(format!(
            "could not parse slash command: {line}"
        )));
        return SlashOutcome::Continue;
    };

    let reset_history = matches!(parsed, SlashCommand::Clear | SlashCommand::Load(_));

    let tx_for_rebuild = tui_tx.clone();
    let rebuild = move |s: Session| build_tui_runtime(s, tx_for_rebuild.clone());
    let state = ReplState::default();
    let mut buf: Vec<u8> = Vec::new();

    let outcome = dispatch_slash_command(parsed, runtime, &state, &mut buf, &rebuild);

    if reset_history {
        let _ = tui_tx.send(TuiEvent::SessionReset);
    }

    if !buf.is_empty() {
        let raw = String::from_utf8_lossy(&buf);
        let text = strip_ansi_escapes(&raw);
        let _ = tui_tx.send(TuiEvent::Info(text));
    }

    outcome
}

/// Strip CSI/SGR ANSI escape sequences (`ESC [ … final-byte`) from a string.
/// The slash-command handlers in [`crate::commands`] format output with the
/// `colored` crate via [`crate::theme`], which emits ANSI escapes when the
/// process is attached to a TTY. The REPL's stderr renders those as colours;
/// the TUI captures the same bytes into a buffer and ships them as
/// [`TuiEvent::Info`], where ratatui would render the raw `\x1b[…m` codes as
/// literal text. Strip them so the TUI shows clean plain-text output.
fn strip_ansi_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // ESC was just consumed. CSI ("ESC [") is what `colored` emits; skip
        // the parameter bytes (digits/semicolons) until the final byte. Other
        // ESC-prefixed forms (e.g. ESC ] OSC, ESC ( charset) just consume one
        // more byte and stop — good enough for our purposes.
        if let Some('[') = chars.next() {
            for nc in chars.by_ref() {
                // CSI final bytes are 0x40-0x7E. Stop on the first one.
                if matches!(nc, '\x40'..='\x7E') {
                    break;
                }
            }
        }
    }
    out
}

/// Compact the runtime in-place when the session exceeds the threshold.
/// Returns the removed message count, or `None` if no compaction was needed.
///
/// Consults the same tiered policy ([`crate::run::pick_compact_plan`]) as
/// the REPL, so the TUI also benefits from `CLAUDETTE_SOFT_COMPACT_THRESHOLD`
/// — pre-fix the TUI only ever compacted at the hard ceiling.
fn maybe_compact(runtime: &mut TuiRuntime, tui_tx: &SyncSender<TuiEvent>) -> Option<usize> {
    let estimated = estimate_session_tokens(runtime.session());
    let (_, preserve, _) = crate::run::pick_compact_plan(
        estimated,
        compact_threshold(),
        crate::run::soft_compact_threshold(),
    )?;
    let result = compact_session(
        runtime.session(),
        CompactionConfig {
            preserve_recent_messages: preserve,
            max_estimated_tokens: 0,
        },
    );
    if result.removed_message_count == 0 {
        return None;
    }
    let removed = result.removed_message_count;
    *runtime = build_tui_runtime(result.compacted_session, tui_tx.clone());
    Some(removed)
}

/// Spawn the worker thread. The thread owns the runtime for its entire
/// lifetime, processing `UserInput` commands one at a time and firing
/// `TuiEvent`s for every interesting state change.
pub fn spawn_worker(
    session: Session,
    user_rx: Receiver<UserInput>,
    tui_tx: SyncSender<TuiEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut runtime = build_tui_runtime(session, tui_tx.clone());

        // Pre-flight the recall embedder so a missing embed model surfaces
        // a clear warn line before the first turn instead of as per-turn
        // noise. Mirrors the REPL startup probe — same sticky-disable
        // semantics afterwards.
        probe_recall_at_startup();

        while let Ok(input) = user_rx.recv() {
            match input {
                UserInput::Quit => break,

                UserInput::SlashCommand(cmd) => {
                    if handle_tui_slash(&cmd, &mut runtime, &tui_tx) == SlashOutcome::Exit {
                        break;
                    }
                }

                UserInput::Message { text, images } => {
                    let _ = tui_tx.send(TuiEvent::Working(true));

                    crate::tools::set_current_turn_paths(crate::tools::extract_user_prompt_paths(
                        &text,
                    ));
                    let image_pairs: Vec<(String, String)> = images
                        .into_iter()
                        .map(|att| (att.media_type, att.data_b64))
                        .collect();
                    let turn_result = if image_pairs.is_empty() {
                        runtime.run_turn(&text, None)
                    } else {
                        runtime.run_turn_with_images(&text, image_pairs, None)
                    };
                    match turn_result {
                        Ok(summary) => {
                            // Extract the last assistant text block.
                            let response = summary
                                .assistant_messages
                                .last()
                                .and_then(|m| {
                                    m.blocks.iter().find_map(|b| {
                                        if let ContentBlock::Text { text } = b {
                                            Some(text.clone())
                                        } else {
                                            None
                                        }
                                    })
                                })
                                .unwrap_or_default();

                            let _ = tui_tx.send(TuiEvent::TurnComplete {
                                text: response,
                                iterations: summary.iterations as u32,
                                in_tok: summary.usage.input_tokens,
                                out_tok: summary.usage.output_tokens,
                            });

                            // Cross-session recall indexing — non-blocking
                            // hand-off to the process-wide async indexer
                            // thread (see `run::recall_index_sender`). The
                            // worker thread holds the sticky-disable
                            // semantics; the foreground gate skips the
                            // alloc + push when broken/disabled. Errors are
                            // logged to stderr by the worker, so the TUI
                            // Info channel stays quiet (the previous per-
                            // turn TuiEvent::Info path is gone with the
                            // blocking embed call).
                            if recall_index_allowed() {
                                index_turn_for_recall(&text, &runtime);
                            }
                        }
                        Err(e) => {
                            let _ = tui_tx.send(TuiEvent::TurnError(e.to_string()));
                        }
                    }

                    // Post-turn housekeeping runs whether the turn succeeded
                    // or failed — otherwise a failing turn leaves the session
                    // bloated and the next attempt is strictly more likely to
                    // OOM. See [[compaction-v04-gaps]] gap #2.
                    if let Some(removed) = maybe_compact(&mut runtime, &tui_tx) {
                        let _ = tui_tx.send(TuiEvent::Compacted { removed });
                    }
                    let estimated = estimate_session_tokens(runtime.session());
                    let _ = tui_tx.send(TuiEvent::TokensUpdate {
                        estimated,
                        threshold: compact_threshold(),
                    });
                    if let Err(e) = save_session(runtime.session()) {
                        eprintln!("tui worker: session save failed: {e:#}");
                    } else {
                        let _ = tui_tx.send(TuiEvent::Saved);
                    }

                    let _ = tui_tx.send(TuiEvent::Working(false));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::strip_ansi_escapes;

    #[test]
    fn strip_ansi_passthrough_when_no_escapes() {
        assert_eq!(strip_ansi_escapes("hello world"), "hello world");
        assert_eq!(strip_ansi_escapes(""), "");
    }

    #[test]
    fn strip_ansi_removes_csi_sgr() {
        // `colored::Colorize::cyan` emits `\x1b[36m…\x1b[0m`.
        assert_eq!(strip_ansi_escapes("\x1b[36mhello\x1b[0m"), "hello");
        assert_eq!(
            strip_ansi_escapes("\x1b[1;31mERR\x1b[0m: oops"),
            "ERR: oops"
        );
    }

    #[test]
    fn strip_ansi_preserves_newlines_and_emoji() {
        let input = "✓ \x1b[32mok\x1b[0m\nnext line 🤖";
        assert_eq!(strip_ansi_escapes(input), "✓ ok\nnext line 🤖");
    }
}
