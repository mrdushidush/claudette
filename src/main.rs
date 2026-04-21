//! claudette CLI binary.
//!
//! Usage:
//!     claudette                            # interactive REPL (fresh)
//!     claudette --resume                   # interactive REPL (continue saved session)
//!     claudette "<prompt...>"              # single-shot, prints reply and exits
//!     claudette --resume "<prompt...>"     # single-shot, continuing the saved session
//!
//! Examples:
//!     claudette "what time is it"
//!     claudette "add 47 and 38"
//!     claudette                            # then chat freely
//!     claudette -r                         # resume the last conversation
//!
//! Sessions live at ~/.claudette/sessions/last.json (override with the
//! `CLAUDETTE_SESSION` env var). REPL mode auto-saves after every turn;
//! single-shot mode only saves when --resume is passed, so a one-off
//! invocation can't clobber a long REPL conversation.

use std::process::ExitCode;

use claudette::{
    briefing, clock, google_auth, probe_ollama, run_secretary, run_secretary_repl, scheduler,
    secrets, telegram_mode, theme, try_load_session, SessionOptions,
};
use claudette::{ContentBlock, Session};

/// Parsed CLI invocation. Any flag below that doesn't make sense with the
/// selected mode is quietly ignored (the old tuple contract) — the parser
/// is deliberately lenient.
#[derive(Debug, Default)]
struct CliArgs {
    resume: bool,
    telegram: bool,
    chat_ids: Vec<i64>,
    prompt_words: Vec<String>,
    tui: bool,
    auth_google: bool,
    auth_google_revoke: bool,
    /// `--briefing`: create (or replace) the scheduled morning-briefing
    /// entry using the default BRIEFING_PROMPT plus the --time / --days
    /// modifiers, then exit.
    briefing: bool,
    briefing_time: Option<String>,
    briefing_days: Option<String>,
}

fn main() -> ExitCode {
    // Load env vars from .env files. Two locations, in this order so CWD
    // wins on conflict:
    //   1. CWD/.env (or any parent) — for project-local overrides
    //   2. ~/.claudette/.env — the canonical user data home, so the same
    //      config applies regardless of which directory the binary is run
    //      from. This is where CLAUDETTE_MODEL, _NUM_CTX, _COMPACT_THRESHOLD,
    //      and tokens like BRAVE_API_KEY / GITHUB_TOKEN belong.
    // Both calls are best-effort; missing files are silently ignored — tools
    // that actually need an env var will report their own missing-key errors.
    let _ = dotenvy::dotenv();
    if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        let path = std::path::PathBuf::from(home)
            .join(".claudette")
            .join(".env");
        let _ = dotenvy::from_path(&path);
    }
    theme::init();

    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(&raw_args);
    let CliArgs {
        resume,
        telegram,
        mut chat_ids,
        prompt_words: prompt_args,
        tui: tui_mode,
        auth_google,
        auth_google_revoke,
        briefing,
        briefing_time,
        briefing_days,
    } = args;

    // ── Google OAuth flow ─────────────────────────────────────────────
    // Runs before the Ollama probe because it's a one-shot setup command —
    // the user doesn't need the brain running to grant OAuth consent.
    if auth_google {
        let result = if auth_google_revoke {
            google_auth::revoke()
        } else {
            google_auth::run_auth_flow()
        };
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{} {}", theme::error(theme::ERR_GLYPH), theme::error(&e));
                ExitCode::FAILURE
            }
        };
    }

    // ── Scheduled-briefing setup ──────────────────────────────────────
    // `claudette --briefing` is a one-shot CLI that writes a recurring
    // entry into ~/.claudette/schedule.jsonl using the canonical
    // BRIEFING_PROMPT + the chosen time / days, then exits. The
    // Telegram bot picks it up automatically next time it starts.
    if briefing {
        return run_briefing_setup(briefing_time.as_deref(), briefing_days.as_deref());
    }

    // Fail fast with a readable message if Ollama isn't running, instead of
    // surfacing a raw reqwest connection error inside the first chat turn.
    // Bypass with CLAUDETTE_SKIP_OLLAMA_PROBE=1 for offline / CI scenarios.
    if let Err(msg) = probe_ollama() {
        eprintln!("{} {}", theme::error(theme::ERR_GLYPH), theme::error(&msg));
        return ExitCode::FAILURE;
    }

    // ── TUI mode ──────────────────────────────────────────────────────
    if tui_mode {
        let session = if resume {
            match try_load_session() {
                Ok(Some(s)) => s,
                Ok(None) => Session::default(),
                Err(e) => {
                    eprintln!("Failed to load session: {e:#}");
                    Session::default()
                }
            }
        } else {
            Session::default()
        };
        return match claudette::tui::run_tui(session) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!(
                    "{} {}",
                    theme::error(theme::ERR_GLYPH),
                    theme::error(&format!("{e:#}"))
                );
                ExitCode::FAILURE
            }
        };
    }

    // ── Telegram bot mode ──────────────────────────────────────────────
    if telegram {
        // Merge persisted chat IDs (from previous runs) with CLI flags.
        for id in secrets::load_chat_ids() {
            if !chat_ids.contains(&id) {
                chat_ids.push(id);
            }
        }
        match telegram_mode::run_telegram_bot(chat_ids, resume) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!(
                    "{} {}",
                    theme::error(theme::ERR_GLYPH),
                    theme::error(&format!("{e:#}"))
                );
                ExitCode::FAILURE
            }
        }
    } else if prompt_args.is_empty() {
        // No prompt → interactive REPL. REPL always autosaves.
        let opts = SessionOptions {
            resume,
            autosave: true,
        };
        match run_secretary_repl(opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!(
                    "{} {}",
                    theme::error(theme::ERR_GLYPH),
                    theme::error(&format!("{e:#}"))
                );
                ExitCode::FAILURE
            }
        }
    } else {
        // Prompt → single-shot. Only save when continuing an existing session,
        // so `claudette "ad hoc question"` can't clobber the REPL session.
        let opts = SessionOptions {
            resume,
            autosave: resume,
        };
        let prompt = prompt_args.join(" ");
        match run_secretary(&prompt, opts) {
            Ok(summary) => {
                if let Some(last) = summary.assistant_messages.last() {
                    for block in &last.blocks {
                        if let ContentBlock::Text { text } = block {
                            println!("{text}");
                        }
                    }
                }
                eprintln!();
                eprintln!(
                    "{} {}",
                    theme::BOLT,
                    theme::info(&format!(
                        "iter={} in={} out={}",
                        summary.iterations, summary.usage.input_tokens, summary.usage.output_tokens,
                    ))
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!(
                    "{} {}",
                    theme::error(theme::ERR_GLYPH),
                    theme::error(&format!("{e:#}"))
                );
                ExitCode::FAILURE
            }
        }
    }
}

/// Bare-bones flag parser.
///
/// Supported flags:
/// - `--resume` / `-r` — resume saved session
/// - `--telegram` / `-t` — run as Telegram bot
/// - `--tui` — launch the ratatui TUI
/// - `--chat <id>` — restrict to this chat ID (repeatable)
/// - `--auth-google` — run the Google OAuth loopback flow and save tokens
/// - `--revoke` — paired with `--auth-google`, revokes tokens and deletes
///   the local file
/// - `--briefing` — write a recurring morning-briefing schedule entry and
///   exit. Optional `--time HH:MM` (default 07:00) and `--days weekdays|daily`
///   (default weekdays).
/// - `--time HH:MM` / `--days <spec>` — modifiers for `--briefing`
fn parse_args(args: &[String]) -> CliArgs {
    let mut out = CliArgs::default();
    let mut expect = ExpectNext::Nothing;

    // Also check env var for chat IDs.
    if let Ok(val) = std::env::var("CLAUDETTE_TELEGRAM_CHAT") {
        for part in val.split(',') {
            if let Ok(id) = part.trim().parse::<i64>() {
                out.chat_ids.push(id);
            }
        }
    }

    for arg in args {
        match expect {
            ExpectNext::ChatId => {
                if let Ok(id) = arg.parse::<i64>() {
                    out.chat_ids.push(id);
                }
                expect = ExpectNext::Nothing;
                continue;
            }
            ExpectNext::Time => {
                out.briefing_time = Some(arg.clone());
                expect = ExpectNext::Nothing;
                continue;
            }
            ExpectNext::Days => {
                out.briefing_days = Some(arg.clone());
                expect = ExpectNext::Nothing;
                continue;
            }
            ExpectNext::Nothing => {}
        }
        match arg.as_str() {
            "--resume" | "-r" => out.resume = true,
            "--telegram" | "-t" => out.telegram = true,
            "--tui" => out.tui = true,
            "--chat" => expect = ExpectNext::ChatId,
            "--auth-google" => out.auth_google = true,
            "--revoke" => out.auth_google_revoke = true,
            "--briefing" => out.briefing = true,
            "--time" => expect = ExpectNext::Time,
            "--days" => expect = ExpectNext::Days,
            _ => out.prompt_words.push(arg.clone()),
        }
    }
    out
}

enum ExpectNext {
    Nothing,
    ChatId,
    Time,
    Days,
}

/// Create (or replace) the scheduled morning-briefing entry. Used by the
/// `--briefing` CLI flag. Does not talk to Telegram at all — the running
/// bot picks the new entry up on its next startup (or immediately if
/// already running, since tools write to the same jsonl).
fn run_briefing_setup(time: Option<&str>, days: Option<&str>) -> ExitCode {
    let time_str = time.unwrap_or("07:00");
    let days_spec = days.unwrap_or("weekdays").to_lowercase();

    let when = match days_spec.as_str() {
        "weekdays" | "weekday" => format!("every weekday at {time_str}"),
        "daily" | "everyday" | "every-day" => format!("daily at {time_str}"),
        "mon" | "monday" | "tue" | "tuesday" | "wed" | "wednesday" | "thu" | "thursday"
        | "fri" | "friday" | "sat" | "saturday" | "sun" | "sunday" => {
            format!("every {days_spec} at {time_str}")
        }
        other => {
            eprintln!(
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!(
                    "--days '{other}' not recognised. Try 'weekdays', 'daily', or a weekday name."
                ))
            );
            return ExitCode::FAILURE;
        }
    };

    // Install the global scheduler against the default jsonl path so the
    // add() call persists to disk. We use a system clock because this
    // command runs outside the Telegram consumer's clock context.
    let path = scheduler::default_path();
    let clk: std::sync::Arc<dyn clock::Clock> = std::sync::Arc::new(clock::SystemClock);
    match scheduler::Scheduler::load(path.clone(), clk.clone()) {
        Ok((loaded, _firings)) => scheduler::install(loaded),
        Err(e) => {
            eprintln!(
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("scheduler load failed: {e}"))
            );
            return ExitCode::FAILURE;
        }
    }

    let mut guard = match scheduler::global().lock() {
        Ok(g) => g,
        Err(e) => {
            eprintln!(
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("scheduler lock failed: {e}"))
            );
            return ExitCode::FAILURE;
        }
    };

    // Replace any existing briefing so --briefing is idempotent. We
    // identify a "briefing entry" by the stored prompt matching the
    // canonical BRIEFING_PROMPT; if a future release changes the prompt,
    // old entries stop matching and are left in place — which is the
    // conservative choice (don't silently drop the user's custom one-off
    // that happened to share a prompt).
    let existing: Vec<String> = guard
        .list()
        .iter()
        .filter(|e| e.prompt == briefing::BRIEFING_PROMPT)
        .map(|e| e.id.clone())
        .collect();
    for id in &existing {
        let _ = guard.cancel(id);
    }

    match guard.add(
        &when,
        briefing::BRIEFING_PROMPT.to_string(),
        None, // chat_id resolved at fire time via default_scheduled_chat
        Some(scheduler::CatchUp::Skip),
    ) {
        Ok(entry) => {
            drop(guard);
            let replaced_note = if existing.is_empty() {
                String::new()
            } else {
                format!(" (replaced {} previous entry/ies)", existing.len())
            };
            eprintln!(
                "{} {}",
                theme::SPARKLES,
                theme::ok(&format!(
                    "scheduled briefing '{}' — {}{}",
                    entry.id, entry.original_expr, replaced_note
                ))
            );
            eprintln!(
                "  {} {}",
                theme::dim("▸"),
                theme::dim(&format!(
                    "next fire: {}",
                    entry.next_fire_at.with_timezone(&chrono::Local).to_rfc3339()
                ))
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!(
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("could not schedule briefing: {e}"))
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_no_flags() {
        let a = parse_args(&["hello".into(), "world".into()]);
        assert!(!a.resume);
        assert!(!a.telegram);
        assert!(!a.tui);
        assert!(!a.auth_google);
        assert!(!a.auth_google_revoke);
        assert!(!a.briefing);
        assert_eq!(
            a.prompt_words,
            vec!["hello".to_string(), "world".to_string()]
        );
    }

    #[test]
    fn parse_args_resume_long() {
        let a = parse_args(&["--resume".into(), "what".into(), "time".into()]);
        assert!(a.resume);
        assert_eq!(
            a.prompt_words,
            vec!["what".to_string(), "time".to_string()]
        );
    }

    #[test]
    fn parse_args_resume_short() {
        let a = parse_args(&["-r".into()]);
        assert!(a.resume);
        assert!(a.prompt_words.is_empty());
    }

    #[test]
    fn parse_args_resume_anywhere() {
        let a = parse_args(&["go".into(), "-r".into(), "now".into()]);
        assert!(a.resume);
        assert_eq!(a.prompt_words, vec!["go".to_string(), "now".to_string()]);
    }

    #[test]
    fn parse_args_telegram_mode() {
        let a = parse_args(&["--telegram".into()]);
        assert!(a.telegram);
        assert!(a.prompt_words.is_empty());
    }

    #[test]
    fn parse_args_telegram_with_chat() {
        let a = parse_args(&[
            "--telegram".into(),
            "--resume".into(),
            "--chat".into(),
            "123456789".into(),
        ]);
        assert!(a.telegram);
        assert!(a.resume);
        assert!(a.chat_ids.contains(&123456789));
    }

    #[test]
    fn parse_args_telegram_short() {
        let a = parse_args(&["-t".into()]);
        assert!(a.telegram);
    }

    #[test]
    fn parse_args_tui_flag() {
        let a = parse_args(&["--tui".into()]);
        assert!(a.tui);
    }

    #[test]
    fn parse_args_tui_with_resume() {
        let a = parse_args(&["--tui".into(), "--resume".into()]);
        assert!(a.tui);
        assert!(a.resume);
    }

    #[test]
    fn parse_args_auth_google_flag() {
        let a = parse_args(&["--auth-google".into()]);
        assert!(a.auth_google);
        assert!(!a.auth_google_revoke);
        assert!(a.prompt_words.is_empty());
    }

    #[test]
    fn parse_args_auth_google_revoke() {
        let a = parse_args(&["--auth-google".into(), "--revoke".into()]);
        assert!(a.auth_google);
        assert!(a.auth_google_revoke);
    }

    #[test]
    fn parse_args_briefing_defaults() {
        let a = parse_args(&["--briefing".into()]);
        assert!(a.briefing);
        assert_eq!(a.briefing_time, None);
        assert_eq!(a.briefing_days, None);
    }

    #[test]
    fn parse_args_briefing_with_time_and_days() {
        let a = parse_args(&[
            "--briefing".into(),
            "--time".into(),
            "07:30".into(),
            "--days".into(),
            "weekdays".into(),
        ]);
        assert!(a.briefing);
        assert_eq!(a.briefing_time.as_deref(), Some("07:30"));
        assert_eq!(a.briefing_days.as_deref(), Some("weekdays"));
    }
}
