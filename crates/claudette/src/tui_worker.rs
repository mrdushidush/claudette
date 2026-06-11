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
    PermissionPromptDecision, PermissionPrompter, PermissionRequest, Session,
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
/// `WorkspaceWrite` tools pass through. `DangerFullAccess` tools prompt the
/// user via [`TuiPrompter`] (a confirmation modal in the render loop).
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
    // Same graceful iteration-cap landing as the REPL chokepoint in
    // `run::build_runtime` — cap hits end in a state-of-work summary, not
    // a discarded turn.
    .with_graceful_iteration_cap()
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

/// TUI analogue of [`crate::run::CliPrompter`] — the permission prompter
/// for `DangerFullAccess` tools (bash, edit_file, git mutations, …).
///
/// Runs on the worker thread, which is parked inside `run_turn` while a
/// decision is pending, so the answer cannot arrive over the regular
/// `UserInput` channel (the worker owns that receiver but is not reading
/// it mid-turn). Instead, each `decide()` creates a fresh rendezvous
/// channel and ships its sender to the render loop inside
/// [`TuiEvent::PermissionRequest`]:
///
/// - per-request channel ⇒ a stale/buffered answer from an earlier prompt
///   can never satisfy a later one, by construction;
/// - every render-loop exit path (quit, error `?`, panic) drops the sender
///   ⇒ `recv()` returns `Disconnected` ⇒ the tool is denied, never hung.
pub struct TuiPrompter {
    tui_tx: SyncSender<TuiEvent>,
}

impl TuiPrompter {
    pub fn new(tui_tx: SyncSender<TuiEvent>) -> Self {
        Self { tui_tx }
    }
}

impl PermissionPrompter for TuiPrompter {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
        // Rendezvous (capacity 0): the render loop's send completes only
        // when this thread is at `recv()` — which it is, nanoseconds after
        // the event send below. FIFO ordering on `tui_tx` guarantees the
        // render loop sees the request before any answer is expected.
        let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel::<bool>(0);

        let sent = self.tui_tx.send(TuiEvent::PermissionRequest {
            tool_name: request.tool_name.clone(),
            input: request.input.clone(),
            required_mode: request.required_mode.as_str().to_string(),
            resp_tx,
        });
        if sent.is_err() {
            // Render loop is gone — fail closed, same reason CliPrompter
            // uses when it cannot read an answer.
            return PermissionPromptDecision::Deny {
                reason: "could not read user input".to_string(),
            };
        }

        match resp_rx.recv() {
            Ok(true) => PermissionPromptDecision::Allow,
            Ok(false) => PermissionPromptDecision::Deny {
                reason: "user denied permission".to_string(),
            },
            Err(_) => PermissionPromptDecision::Deny {
                reason: "could not read user input".to_string(),
            },
        }
    }
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

/// Format the outcome of `try_rehydrate_active_mission()` as a plain-text
/// line for `TuiEvent::Info`. Returns `None` for `RehydrateOutcome::None`
/// so a fresh session stays quiet. Mirrors `run::print_rehydrate_outcome`
/// but writes to a string instead of stderr because the TUI swallows
/// stderr in alt-screen mode.
fn format_rehydrate_outcome_for_tui(outcome: &crate::missions::RehydrateOutcome) -> Option<String> {
    use crate::missions::RehydrateOutcome;
    match outcome {
        RehydrateOutcome::None => None,
        RehydrateOutcome::Rehydrated(m) => Some(format!(
            "resumed mission: {} ({})\n\
             clear it with /mission_exit (or mission_state action=exit) if you didn't intend this",
            m.slug,
            m.path.display(),
        )),
        RehydrateOutcome::Cleared { reason, path } => Some(format!(
            "cleared stale active-mission pointer at {} — {reason}",
            path.display(),
        )),
    }
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
        let mut prompter = TuiPrompter::new(tui_tx.clone());

        // Pre-flight the recall embedder so a missing embed model surfaces
        // a clear warn line before the first turn instead of as per-turn
        // noise. Mirrors the REPL startup probe — same sticky-disable
        // semantics afterwards.
        probe_recall_at_startup();

        // Rehydrate any persisted non-ephemeral mission (F8a fix). Mirrors
        // the REPL startup in run_secretary_repl. Outcome is surfaced via
        // TuiEvent::Info so the user sees it in the chat history rather
        // than the terminal stderr (which the TUI swallows).
        let outcome = crate::missions::try_rehydrate_active_mission();
        if let Some(line) = format_rehydrate_outcome_for_tui(&outcome) {
            let _ = tui_tx.send(TuiEvent::Info(line));
        }

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
                        runtime.run_turn(&text, Some(&mut prompter))
                    } else {
                        runtime.run_turn_with_images(&text, image_pairs, Some(&mut prompter))
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

                            if summary.hit_iteration_cap {
                                let _ = tui_tx.send(TuiEvent::Info(
                                    "⚠ turn hit the iteration cap — the reply \
                                     above is a state-of-work summary; the \
                                     task may be unfinished"
                                        .to_string(),
                                ));
                            }

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
    use super::{strip_ansi_escapes, TuiPrompter};
    use crate::tui_events::TuiEvent;
    use crate::{
        PermissionMode, PermissionOutcome, PermissionPolicy, PermissionPromptDecision,
        PermissionPrompter, PermissionRequest,
    };
    use std::sync::mpsc;

    fn danger_request(input: &str) -> PermissionRequest {
        PermissionRequest {
            tool_name: "bash".to_string(),
            input: input.to_string(),
            current_mode: PermissionMode::WorkspaceWrite,
            required_mode: PermissionMode::DangerFullAccess,
        }
    }

    /// Pretend to be the render loop: receive one PermissionRequest event
    /// and answer it (or drop the channel when `answer` is None).
    fn spawn_render_stub(
        tui_rx: mpsc::Receiver<TuiEvent>,
        answer: Option<bool>,
    ) -> std::thread::JoinHandle<(String, String, String)> {
        std::thread::spawn(move || match tui_rx.recv().expect("no event") {
            TuiEvent::PermissionRequest {
                tool_name,
                input,
                required_mode,
                resp_tx,
            } => {
                if let Some(ans) = answer {
                    resp_tx.send(ans).expect("worker hung up");
                }
                // None → resp_tx drops here, simulating a render-loop exit.
                (tool_name, input, required_mode)
            }
            other => panic!("unexpected event: {other:?}"),
        })
    }

    #[test]
    fn tui_prompter_allows_on_true_and_ships_full_input() {
        let (tui_tx, tui_rx) = mpsc::sync_channel(8);
        let stub = spawn_render_stub(tui_rx, Some(true));
        let mut p = TuiPrompter::new(tui_tx);

        let decision = p.decide(&danger_request("rm -rf ./target && echo done"));
        assert_eq!(decision, PermissionPromptDecision::Allow);

        let (tool, input, mode) = stub.join().unwrap();
        assert_eq!(tool, "bash");
        // Full input — no truncation anywhere on the prompt path.
        assert_eq!(input, "rm -rf ./target && echo done");
        assert_eq!(mode, "danger-full-access");
    }

    #[test]
    fn tui_prompter_denies_on_false_with_cli_parity_reason() {
        let (tui_tx, tui_rx) = mpsc::sync_channel(8);
        let stub = spawn_render_stub(tui_rx, Some(false));
        let mut p = TuiPrompter::new(tui_tx);

        let decision = p.decide(&danger_request("git push --force"));
        assert_eq!(
            decision,
            PermissionPromptDecision::Deny {
                reason: "user denied permission".to_string(),
            }
        );
        stub.join().unwrap();
    }

    #[test]
    fn tui_prompter_denies_when_render_loop_drops_the_answer() {
        let (tui_tx, tui_rx) = mpsc::sync_channel(8);
        // Render stub receives the request but exits without answering —
        // the per-request sender drops, which must read as a deny.
        let stub = spawn_render_stub(tui_rx, None);
        let mut p = TuiPrompter::new(tui_tx);

        let decision = p.decide(&danger_request("bash payload"));
        assert_eq!(
            decision,
            PermissionPromptDecision::Deny {
                reason: "could not read user input".to_string(),
            }
        );
        stub.join().unwrap();
    }

    #[test]
    fn tui_prompter_denies_when_render_loop_is_gone_entirely() {
        let (tui_tx, tui_rx) = mpsc::sync_channel(8);
        drop(tui_rx); // no render loop at all
        let mut p = TuiPrompter::new(tui_tx);

        let decision = p.decide(&danger_request("anything"));
        assert_eq!(
            decision,
            PermissionPromptDecision::Deny {
                reason: "could not read user input".to_string(),
            }
        );
    }

    #[test]
    fn authorize_consults_tui_prompter_for_danger_tools() {
        // End-to-end through the real policy: a DangerFullAccess tool under
        // a WorkspaceWrite active mode is no longer hard-denied — it asks
        // the prompter, and the user's modal answer decides.
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);

        let (tui_tx, tui_rx) = mpsc::sync_channel(8);
        let stub = spawn_render_stub(tui_rx, Some(true));
        let mut p = TuiPrompter::new(tui_tx);
        let outcome = policy.authorize("bash", "cargo test", Some(&mut p));
        assert_eq!(outcome, PermissionOutcome::Allow);
        stub.join().unwrap();

        let (tui_tx, tui_rx) = mpsc::sync_channel(8);
        let stub = spawn_render_stub(tui_rx, Some(false));
        let mut p = TuiPrompter::new(tui_tx);
        let outcome = policy.authorize("bash", "cargo test", Some(&mut p));
        assert_eq!(
            outcome,
            PermissionOutcome::Deny {
                reason: "user denied permission".to_string(),
            }
        );
        stub.join().unwrap();
    }

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
