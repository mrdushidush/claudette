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
    google_auth, probe_ollama, run_secretary, run_secretary_repl, secrets, telegram_mode, theme,
    try_load_session, SessionOptions,
};
use claudette::{ContentBlock, Session};

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
    let (resume, telegram, mut chat_ids, prompt_args, tui_mode, auth_google, auth_google_revoke) =
        parse_args(&raw_args);

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

/// Bare-bones flag parser. Returns
/// `(resume, telegram, chat_ids, prompt_words, tui, auth_google, auth_google_revoke)`.
///
/// Supported flags:
/// - `--resume` / `-r` — resume saved session
/// - `--telegram` / `-t` — run as Telegram bot
/// - `--tui` — launch the ratatui TUI
/// - `--chat <id>` — restrict to this chat ID (repeatable)
/// - `--auth-google` — run the Google OAuth loopback flow and save tokens
/// - `--revoke` — paired with `--auth-google`, revokes tokens with Google
///   and deletes the local file
fn parse_args(args: &[String]) -> (bool, bool, Vec<i64>, Vec<String>, bool, bool, bool) {
    let mut resume = false;
    let mut telegram = false;
    let mut tui = false;
    let mut auth_google = false;
    let mut auth_google_revoke = false;
    let mut chat_ids: Vec<i64> = Vec::new();
    let mut prompt = Vec::with_capacity(args.len());
    let mut expect_chat_id = false;

    // Also check env var for chat IDs.
    if let Ok(val) = std::env::var("CLAUDETTE_TELEGRAM_CHAT") {
        for part in val.split(',') {
            if let Ok(id) = part.trim().parse::<i64>() {
                chat_ids.push(id);
            }
        }
    }

    for arg in args {
        if expect_chat_id {
            if let Ok(id) = arg.parse::<i64>() {
                chat_ids.push(id);
            }
            expect_chat_id = false;
            continue;
        }
        match arg.as_str() {
            "--resume" | "-r" => resume = true,
            "--telegram" | "-t" => telegram = true,
            "--tui" => tui = true,
            "--chat" => expect_chat_id = true,
            "--auth-google" => auth_google = true,
            "--revoke" => auth_google_revoke = true,
            _ => prompt.push(arg.clone()),
        }
    }
    (
        resume,
        telegram,
        chat_ids,
        prompt,
        tui,
        auth_google,
        auth_google_revoke,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_no_flags() {
        let (resume, telegram, chat_ids, prompt, tui, auth_google, auth_revoke) =
            parse_args(&["hello".into(), "world".into()]);
        assert!(!resume);
        assert!(!telegram);
        assert!(!tui);
        assert!(!auth_google);
        assert!(!auth_revoke);
        assert!(chat_ids.is_empty() || !chat_ids.is_empty()); // env var may set some
        assert_eq!(prompt, vec!["hello".to_string(), "world".to_string()]);
    }

    #[test]
    fn parse_args_resume_long() {
        let (resume, _, _, prompt, _, _, _) =
            parse_args(&["--resume".into(), "what".into(), "time".into()]);
        assert!(resume);
        assert_eq!(prompt, vec!["what".to_string(), "time".to_string()]);
    }

    #[test]
    fn parse_args_resume_short() {
        let (resume, _, _, prompt, _, _, _) = parse_args(&["-r".into()]);
        assert!(resume);
        assert!(prompt.is_empty());
    }

    #[test]
    fn parse_args_resume_anywhere() {
        let (resume, _, _, prompt, _, _, _) =
            parse_args(&["go".into(), "-r".into(), "now".into()]);
        assert!(resume);
        assert_eq!(prompt, vec!["go".to_string(), "now".to_string()]);
    }

    #[test]
    fn parse_args_telegram_mode() {
        let (_, telegram, _, prompt, _, _, _) = parse_args(&["--telegram".into()]);
        assert!(telegram);
        assert!(prompt.is_empty());
    }

    #[test]
    fn parse_args_telegram_with_chat() {
        let (resume, telegram, chat_ids, _, _, _, _) = parse_args(&[
            "--telegram".into(),
            "--resume".into(),
            "--chat".into(),
            "123456789".into(),
        ]);
        assert!(telegram);
        assert!(resume);
        assert!(chat_ids.contains(&123456789));
    }

    #[test]
    fn parse_args_telegram_short() {
        let (_, telegram, _, _, _, _, _) = parse_args(&["-t".into()]);
        assert!(telegram);
    }

    #[test]
    fn parse_args_tui_flag() {
        let (_, _, _, _, tui, _, _) = parse_args(&["--tui".into()]);
        assert!(tui);
    }

    #[test]
    fn parse_args_tui_with_resume() {
        let (resume, _, _, _, tui, _, _) = parse_args(&["--tui".into(), "--resume".into()]);
        assert!(tui);
        assert!(resume);
    }

    #[test]
    fn parse_args_auth_google_flag() {
        let (_, _, _, prompt, _, auth, revoke) = parse_args(&["--auth-google".into()]);
        assert!(auth);
        assert!(!revoke);
        assert!(prompt.is_empty());
    }

    #[test]
    fn parse_args_auth_google_revoke() {
        let (_, _, _, _, _, auth, revoke) =
            parse_args(&["--auth-google".into(), "--revoke".into()]);
        assert!(auth);
        assert!(revoke);
    }
}
