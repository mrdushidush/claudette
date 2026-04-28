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
use crate::executor::SecretaryToolExecutor;
use crate::memory::try_load_memory;
use crate::prompt::secretary_system_prompt_with_memory;
use crate::run::{build_permission_policy, compact_threshold, current_model, save_session};
use crate::tool_groups::{ToolGroup, ToolRegistry};
use crate::tui_events::{TuiEvent, UserInput};
use crate::tui_executor::TuiToolExecutor;

/// Short alias so function signatures stay readable.
type TuiRuntime = ConversationRuntime<OllamaApiClient, TuiToolExecutor>;

/// Build a runtime with `TuiToolExecutor` + TUI text callback.
///
/// Pre-enables the same five tool groups as Telegram mode so the model can
/// use them without an extra `enable_tools` round-trip.
///
/// Uses the same per-tool permission policy as the REPL so `ReadOnly` +
/// `WorkspaceWrite` tools pass through. `DangerFullAccess` tools are denied
/// (no prompter yet). Sprint G will add `TuiPrompter` for confirmation modals.
fn build_tui_runtime(session: Session, tui_tx: SyncSender<TuiEvent>) -> TuiRuntime {
    let mut reg = ToolRegistry::new();
    reg.enable(ToolGroup::Markets);
    reg.enable(ToolGroup::Facts);
    reg.enable(ToolGroup::Advanced);
    reg.enable(ToolGroup::Git);
    reg.enable(ToolGroup::Search);

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

/// Compact the runtime in-place when the session exceeds the threshold.
/// Returns the removed message count, or `None` if no compaction was needed.
fn maybe_compact(runtime: &mut TuiRuntime, tui_tx: &SyncSender<TuiEvent>) -> Option<usize> {
    let estimated = estimate_session_tokens(runtime.session());
    if estimated < compact_threshold() {
        return None;
    }
    let result = compact_session(
        runtime.session(),
        CompactionConfig {
            preserve_recent_messages: 4,
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

        while let Ok(input) = user_rx.recv() {
            match input {
                UserInput::Quit => break,

                UserInput::SlashCommand(cmd) => match cmd.trim() {
                    "clear" => {
                        runtime = build_tui_runtime(Session::default(), tui_tx.clone());
                        let _ = tui_tx.send(TuiEvent::SessionReset);
                    }
                    "compact" => {
                        if let Some(removed) = maybe_compact(&mut runtime, &tui_tx) {
                            let _ = tui_tx.send(TuiEvent::Compacted { removed });
                        } else {
                            let _ = tui_tx.send(TuiEvent::TurnError(
                                "Session is below compaction threshold — nothing to compact."
                                    .to_string(),
                            ));
                        }
                    }
                    other => {
                        let _ = tui_tx.send(TuiEvent::TurnError(format!(
                            "Unknown command: /{other}  (available: /clear, /compact)"
                        )));
                    }
                },

                UserInput::Message(text) => {
                    let _ = tui_tx.send(TuiEvent::Working(true));

                    crate::tools::set_current_turn_paths(crate::tools::extract_user_prompt_paths(
                        &text,
                    ));
                    match runtime.run_turn(&text, None) {
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
                        }
                        Err(e) => {
                            let _ = tui_tx.send(TuiEvent::TurnError(e.to_string()));
                        }
                    }

                    let _ = tui_tx.send(TuiEvent::Working(false));
                }
            }
        }
    })
}
