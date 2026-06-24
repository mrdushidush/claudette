//! Interactive agent REPL loop (Wave C6 — split out of run.rs).
//!
//! `run_agent_repl` drives a single long-lived ConversationRuntime: read a
//! stdin line, dispatch slash commands or run it as a turn, stream the reply,
//! autosave, index for recall, and auto-compact. `use super::*` pulls in the
//! run.rs items it orchestrates (now clean submodules after C1-C5); the explicit
//! `use`s below are the external crate paths.
use super::*;

use std::io::{self, Write};

use crate::commands::{dispatch_slash_command, parse_slash_command, ReplState, SlashOutcome};
use crate::theme;

/// Run an interactive REPL against a single long-lived `ConversationRuntime`.
/// Reads lines from stdin, runs each as a turn, prints the assistant's reply.
/// Lines starting with `/` are interpreted as slash commands (see
/// `commands.rs`) and never reach the model. Exits on EOF, the `/exit`
/// command, or the bare words `exit`/`quit`/`:q` (kept for muscle memory).
/// Always autosaves after every model turn when `opts.autosave` is set.
#[allow(clippy::too_many_lines)]
pub fn run_agent_repl(opts: SessionOptions) -> Result<()> {
    theme::init();

    let session = if opts.resume {
        match try_load_session()? {
            Some(s) => {
                eprintln!(
                    "{} {} {}",
                    theme::SAVE,
                    theme::ok("resumed session"),
                    theme::dim(&format!(
                        "from {} ({} messages)",
                        default_session_path().display(),
                        s.messages.len()
                    ))
                );
                s
            }
            None => {
                eprintln!(
                    "{} {}",
                    theme::dim("○"),
                    theme::dim(&format!(
                        "no saved session at {} — starting fresh",
                        default_session_path().display()
                    ))
                );
                Session::default()
            }
        }
    } else {
        Session::default()
    };

    let mut runtime = build_runtime_streaming(session, false);
    let mut state = ReplState::default();
    let mut prompter = CliPrompter;

    // Activity indicator: a live `thinking…` / `running <tool>…` spinner during
    // the dead air a local backend creates (prompt-processing / JIT reload), so
    // the user can tell a working turn from a hang without watching the LM
    // Studio log. TTY-only (piped / scripted / CI runs stay clean); opt out via
    // CLAUDETTE_NO_SPINNER. No-op everywhere it isn't enabled.
    {
        use std::io::IsTerminal as _;
        if std::io::stderr().is_terminal() && std::env::var_os("CLAUDETTE_NO_SPINNER").is_none() {
            crate::status::global().enable();
        }
    }

    eprintln!(
        "{} {} {}",
        theme::ROBOT,
        theme::brand("claudette"),
        theme::dim("— your local coding agent")
    );
    eprintln!(
        "{} {}",
        theme::SPARKLES,
        theme::dim("type /help for commands, /exit (or Ctrl-D) to leave")
    );
    eprintln!(
        "{} {}",
        theme::SAVE,
        theme::dim(&format!("session: {}", default_session_path().display()))
    );

    // Pre-flight the recall embedder so a missing embed model (the typical
    // LM Studio first-run state) surfaces a clean warn line here, not as
    // per-turn noise after the user starts asking questions. Honors
    // CLAUDETTE_RECALL_DISABLE — opting out skips the probe too.
    probe_recall_at_startup();

    // Rehydrate any persisted non-ephemeral mission so /brownfield → exit
    // → restart → /forge keeps targeting the cloned tree instead of
    // silently falling back to cwd auto-bootstrap (F8a safety fix).
    print_rehydrate_outcome(crate::missions::try_rehydrate_active_mission());

    eprintln!();

    loop {
        // Print prompt.
        {
            let stderr = io::stderr();
            let mut err = stderr.lock();
            write!(err, "{} ", theme::accent(theme::PROMPT_ARROW))?;
            err.flush()?;
        }

        // Read one line WITHOUT holding the stdin lock across run_turn.
        // The CliPrompter needs stdin access for [y/N] confirmation
        // prompts, so we must drop the lock before entering the runtime.
        let line = {
            let stdin = io::stdin();
            let mut buf = String::new();
            match stdin.read_line(&mut buf) {
                Ok(0) => {
                    eprintln!();
                    break; // EOF
                }
                Ok(_) => buf,
                Err(e) => {
                    eprintln!("stdin error: {e}");
                    break;
                }
            }
        };
        // stdin lock is now dropped — safe for the prompter to read.

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if matches!(trimmed, "exit" | "quit" | ":q") {
            break;
        }

        if let Some(cmd) = parse_slash_command(trimmed) {
            let stderr = std::io::stderr();
            let mut err = stderr.lock();
            let rebuild = |s: Session| build_runtime_streaming(s, false);
            match dispatch_slash_command(cmd, &mut runtime, &state, &mut err, &rebuild) {
                SlashOutcome::Continue => continue,
                SlashOutcome::Exit => break,
            }
        }

        crate::tools::set_current_turn_paths(crate::tools::extract_user_prompt_paths(trimmed));

        // Vision: if the line contains image-file path tokens (drag-drop
        // typically pastes them via Windows Terminal), attach them and
        // route directly to `run_turn_with_images`, bypassing the brain
        // selector. The fallback logic is for "stuck" detection on text
        // turns and doesn't apply when we're sending an image.
        let extracted = crate::image_attach::extract_image_attachments_from_input(trimmed);
        if extracted.extension_matches > 0 && extracted.attached.is_empty() {
            if let Some(reason) = &extracted.first_failure {
                eprintln!(
                    "{} {}",
                    theme::WARN_GLYPH,
                    theme::warn(&format!(
                        "image-path detected but couldn't attach: {reason}"
                    ))
                );
            }
        }

        crate::status::global().on_turn_start();
        let turn_result: Result<TurnSummary, String> = if extracted.attached.is_empty() {
            // Sprint 14: route through brain_selector so Auto-preset turns get
            // the 4b → 9b escalation when stuck signals fire. On Fast/Smart
            // (no fallback configured) this collapses to the existing
            // run_turn_with_retry behaviour — no overhead.
            let mut prompter_opt: Option<&mut dyn PermissionPrompter> = Some(&mut prompter);
            crate::brain_selector::run_turn_with_fallback(&mut runtime, trimmed, &mut prompter_opt)
        } else {
            let count = extracted.attached.len();
            eprintln!(
                "{} {}",
                theme::SAVE,
                theme::dim(&format!("📎 attached {count} image(s) — routing to vision"))
            );
            let images: Vec<(String, String)> = extracted
                .attached
                .into_iter()
                .map(|a| (a.media_type, a.data_b64))
                .collect();
            runtime
                .run_turn_with_images(trimmed, images, Some(&mut prompter))
                .map_err(|e| e.to_string())
        };
        crate::status::global().on_turn_end();

        match turn_result {
            Ok(summary) => {
                // No post-turn re-print: streaming has already pushed every
                // text delta to stdout via `stdout_text_callback`. The model's
                // text terminator newline is also fired by the callback at
                // end-of-stream, so the status line below lands on its own row.

                state.record_turn(summary.usage.input_tokens, summary.usage.output_tokens);
                let ctx_gauge = format_ctx_gauge(
                    estimate_session_tokens(runtime.session()),
                    crate::api::current_num_ctx(),
                );
                eprintln!(
                    "{} {} {}",
                    theme::BOLT,
                    theme::info(&format!(
                        "turn iter={} in={} out={}",
                        summary.iterations, summary.usage.input_tokens, summary.usage.output_tokens,
                    )),
                    theme::dim(&ctx_gauge),
                );
                if summary.hit_iteration_cap {
                    eprintln!(
                        "{} {}",
                        theme::WARN_GLYPH,
                        theme::warn(
                            "turn hit the iteration cap — the reply above is a \
                             state-of-work summary; the task may be unfinished"
                        )
                    );
                }

                // Cross-session recall: enqueue the user input + the
                // assistant text from this turn for the async indexer
                // thread (see [`index_turn_for_recall`] / [`recall_index_sender`]).
                // Best-effort — the FIRST failure on the worker thread
                // (e.g. a missing embed model in LM Studio) emits one
                // warn line and then sticky-disables indexing for the
                // rest of this process, so the user isn't spammed turn-
                // after-turn. They can run `/recall reprobe` to retry
                // after loading the embed model. The hard kill-switch
                // `CLAUDETTE_RECALL_DISABLE=1` still wins.
                if recall_index_allowed() {
                    index_turn_for_recall(trimmed, &runtime);
                }
            }
            Err(e) => {
                eprintln!(
                    "{} {}",
                    theme::error(theme::ERR_GLYPH),
                    theme::error(&format!("turn failed: {e}"))
                );
            }
        }

        // Post-turn housekeeping: runs regardless of success/failure so a
        // bloated session doesn't keep paying its context tax across retries.
        // Pre-2026-05-12 this was inside the Ok arm; a `Brain HTTP 400 Model
        // reloaded` error during sprint Test 8 demonstrated that skipping
        // compaction on failure makes the next attempt strictly more likely
        // to OOM. The runtime's built-in trigger is disabled (see
        // `build_runtime_inner`) — this is the live trigger.
        if let Some(outcome) = maybe_compact_session(&mut runtime, false) {
            eprintln!(
                "{} {}",
                theme::SAVE,
                theme::ok(&format!(
                    "auto-compacted {} older message(s) — {} tier crossed at {} tokens",
                    outcome.removed,
                    outcome.tier.name(),
                    outcome.threshold,
                ))
            );
        }

        if opts.autosave {
            if let Err(e) = save_session(runtime.session()) {
                // Surface the error but don't drop the REPL — the session
                // in memory is still valid; only persistence is broken.
                eprintln!(
                    "{} {}",
                    theme::warn(theme::WARN_GLYPH),
                    theme::warn(&format!("session save failed: {e:#}"))
                );
            }
        }
    }

    Ok(())
}
