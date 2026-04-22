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
    /// Which scope bundle to request during `--auth-google`. None means
    /// "use the default" (Calendar) for backwards compatibility with
    /// phase-1 invocations.
    auth_google_scope: Option<String>,
    /// `--briefing`: create (or replace) the scheduled morning-briefing
    /// entry using the default BRIEFING_PROMPT plus the --time / --days
    /// modifiers, then exit.
    briefing: bool,
    briefing_time: Option<String>,
    briefing_days: Option<String>,
    /// `--help` / `-h`: print the flag reference and exit before touching
    /// the Ollama probe or any subsystem. Short-circuits in `main()`.
    help: bool,
    /// `--version` / `-V`: print `claudette <semver>` and exit. Uses the
    /// `CARGO_PKG_VERSION` stamped at compile time.
    version: bool,
    /// `--chat any` sentinel: explicit opt-in to serving every incoming
    /// Telegram chat (no allowlist). Required now that bot mode
    /// default-denies when the chat_ids list is empty — prevents the
    /// previous "run --telegram and accidentally expose the bot to
    /// anyone who guesses the username" footgun.
    allow_any_chat: bool,
}

/// Help text printed on `--help` / `-h`. One source of truth; the README
/// CLI flag table is synced to this block. Keep lines under 80 columns so
/// it fits an 80-wide terminal without wrapping awkwardly.
const HELP_TEXT: &str = "\
claudette — a local-first AI personal secretary, powered by Ollama.

USAGE:
    claudette [FLAGS] [PROMPT...]

MODES (pick one; default is interactive REPL):
    (none)               Start the interactive REPL. Type /help once inside
                         to see the slash-command list.
    \"<prompt>\"            Single-shot: print one reply and exit.
    --resume, -r         Continue the most recent saved session. Works in
                         REPL and single-shot.
    --telegram, -t       Run as a Telegram bot. Requires TELEGRAM_BOT_TOKEN.
    --tui                Launch the fullscreen ratatui TUI (Chat / Tools /
                         Notes / Todos / HW tabs).

TELEGRAM OPTIONS:
    --chat <id>          Restrict the Telegram bot to chat ID <id>.
                         Repeatable; can also be set via CLAUDETTE_TELEGRAM_CHAT
                         (comma-separated list). The bot default-denies when
                         no allowlist is provided.
    --chat any           Explicit accept-all: serve every incoming chat.
                         Required to start the bot with no allowlist. Prints
                         a loud warning since anyone who guesses the bot
                         username can DM and get a full assistant.

ONE-SHOT SETUP COMMANDS (each exits after doing its one job):
    --auth-google [scope]
                         Run the loopback OAuth flow for Google APIs. <scope>
                         is 'calendar' (default) or 'gmail'. Stores tokens
                         under ~/.claudette/secrets/.
    --revoke             Pair with --auth-google to revoke consent + delete
                         the local token file for that scope.
    --briefing           Write a recurring morning-briefing entry to
                         ~/.claudette/schedule.jsonl and exit. The Telegram
                         bot picks it up next time it starts.
    --time HH:MM         Modifier for --briefing. Default: 07:00.
    --days <spec>        Modifier for --briefing. One of 'weekdays' (default),
                         'daily', or a single weekday name ('monday', etc).

MISC:
    --help, -h           Show this help and exit.
    --version, -V        Show the claudette version and exit.

ENVIRONMENT:
    See README.md for the full env-var reference. Frequently used:
      OLLAMA_HOST              Ollama API endpoint (default localhost:11434).
      CLAUDETTE_MODEL          Override the brain model.
      CLAUDETTE_CODER_MODEL    Override the Codet coder model.
      CLAUDETTE_SESSION        Override the session-file path.
      TELEGRAM_BOT_TOKEN       Required for --telegram.

EXAMPLES:
    claudette                            # start the REPL
    claudette \"what time is it?\"         # one-shot
    claudette -r                         # resume last session
    claudette --tui                      # fullscreen TUI
    claudette --auth-google calendar     # OAuth once
    claudette --briefing --time 08:30    # weekday briefings at 08:30
    claudette --telegram --chat 12345    # bot restricted to one chat

DOCS:
    README.md              Full feature / configuration reference
    examples/              Scenario walkthroughs
    CONTRIBUTING.md        How to contribute
    SECURITY.md            Vulnerability reporting
";

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

    // ── --help / --version short-circuit ──────────────────────────────
    // These exit before the Ollama probe runs so `claudette --help` works
    // on a machine that has never pulled a model. They're also before
    // dotenv reload effects anything observable — pure printf and exit.
    if args.help {
        print!("{HELP_TEXT}");
        return ExitCode::SUCCESS;
    }
    if args.version {
        println!("claudette {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let CliArgs {
        resume,
        telegram,
        mut chat_ids,
        prompt_words: prompt_args,
        tui: tui_mode,
        auth_google,
        auth_google_revoke,
        auth_google_scope,
        briefing,
        briefing_time,
        briefing_days,
        help: _,
        version: _,
        allow_any_chat,
    } = args;

    // ── Google OAuth flow ─────────────────────────────────────────────
    // Runs before the Ollama probe because it's a one-shot setup command —
    // the user doesn't need the brain running to grant OAuth consent.
    if auth_google {
        let ctx = match auth_google_scope.as_deref() {
            None => google_auth::AuthContext::Calendar, // backwards compat
            Some(s) => match google_auth::AuthContext::parse(s) {
                Some(c) => c,
                None => {
                    eprintln!(
                        "{} {}",
                        theme::error(theme::ERR_GLYPH),
                        theme::error(&format!(
                            "unknown --auth-google scope '{s}'. Try 'calendar' or 'gmail'."
                        ))
                    );
                    return ExitCode::FAILURE;
                }
            },
        };
        let result = if auth_google_revoke {
            google_auth::revoke(ctx)
        } else {
            google_auth::run_auth_flow(ctx)
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
        match telegram_mode::run_telegram_bot(chat_ids, allow_any_chat, resume) {
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

    // Also check env var for chat IDs. The literal string "any" (any
    // casing) sets the accept-all sentinel, identical to `--chat any`.
    if let Ok(val) = std::env::var("CLAUDETTE_TELEGRAM_CHAT") {
        for part in val.split(',') {
            let trimmed = part.trim();
            if trimmed.eq_ignore_ascii_case("any") {
                out.allow_any_chat = true;
            } else if let Ok(id) = trimmed.parse::<i64>() {
                out.chat_ids.push(id);
            }
        }
    }

    for arg in args {
        match expect {
            ExpectNext::ChatId => {
                if arg.eq_ignore_ascii_case("any") {
                    // Explicit accept-all sentinel. Bypasses the
                    // allowlist without silently default-allowing.
                    out.allow_any_chat = true;
                } else if let Ok(id) = arg.parse::<i64>() {
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
            ExpectNext::AuthGoogleScope => {
                // Peek: if the token looks like a known scope keyword,
                // capture it as the scope. Otherwise treat it as a
                // regular prompt word so `--auth-google somebody typed
                // more words` still parses the way the user expected.
                expect = ExpectNext::Nothing;
                if claudette::google_auth::AuthContext::parse(arg).is_some() {
                    out.auth_google_scope = Some(arg.clone());
                    continue;
                }
                // Fall through to the normal arg-matching below.
            }
            ExpectNext::Nothing => {}
        }
        match arg.as_str() {
            "--help" | "-h" => out.help = true,
            "--version" | "-V" => out.version = true,
            "--resume" | "-r" => out.resume = true,
            "--telegram" | "-t" => out.telegram = true,
            "--tui" => out.tui = true,
            "--chat" => expect = ExpectNext::ChatId,
            "--auth-google" => {
                out.auth_google = true;
                expect = ExpectNext::AuthGoogleScope;
            }
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
    /// Optional scope keyword immediately after `--auth-google`. If the
    /// next token isn't a recognised scope we fall back to normal arg
    /// matching rather than consuming it.
    AuthGoogleScope,
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
        "mon" | "monday" | "tue" | "tuesday" | "wed" | "wednesday" | "thu" | "thursday" | "fri"
        | "friday" | "sat" | "saturday" | "sun" | "sunday" => {
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
                    entry
                        .next_fire_at
                        .with_timezone(&chrono::Local)
                        .to_rfc3339()
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
        assert_eq!(a.prompt_words, vec!["what".to_string(), "time".to_string()]);
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
        assert!(!a.allow_any_chat);
    }

    #[test]
    fn parse_args_chat_any_sets_accept_all() {
        // `--chat any` opts in to accept-all mode without consuming a
        // numeric chat ID. Bot refuses to start without either an
        // explicit allowlist or this flag, so the default-deny posture
        // depends on `any` being parsed correctly.
        let a = parse_args(&["--telegram".into(), "--chat".into(), "any".into()]);
        assert!(a.telegram);
        assert!(a.allow_any_chat);
        assert!(a.chat_ids.is_empty());
    }

    #[test]
    fn parse_args_chat_any_case_insensitive() {
        let a = parse_args(&["--telegram".into(), "--chat".into(), "ANY".into()]);
        assert!(a.allow_any_chat);
    }

    #[test]
    fn parse_args_chat_env_any_sets_accept_all() {
        // CLAUDETTE_TELEGRAM_CHAT also accepts the "any" sentinel. Set
        // and restore around the test to stay polite with parallel runs
        // (other tests don't touch this var).
        let prev = std::env::var("CLAUDETTE_TELEGRAM_CHAT").ok();
        std::env::set_var("CLAUDETTE_TELEGRAM_CHAT", "ANY");
        let a = parse_args(&["--telegram".into()]);
        assert!(a.allow_any_chat);
        assert!(a.chat_ids.is_empty());
        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_TELEGRAM_CHAT", v),
            None => std::env::remove_var("CLAUDETTE_TELEGRAM_CHAT"),
        }
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
        assert_eq!(a.auth_google_scope, None);
        assert!(a.prompt_words.is_empty());
    }

    #[test]
    fn parse_args_auth_google_revoke() {
        let a = parse_args(&["--auth-google".into(), "--revoke".into()]);
        assert!(a.auth_google);
        assert!(a.auth_google_revoke);
        assert_eq!(a.auth_google_scope, None);
    }

    #[test]
    fn parse_args_auth_google_with_gmail_scope() {
        let a = parse_args(&["--auth-google".into(), "gmail".into()]);
        assert!(a.auth_google);
        assert_eq!(a.auth_google_scope.as_deref(), Some("gmail"));
        assert!(a.prompt_words.is_empty());
    }

    #[test]
    fn parse_args_auth_google_with_calendar_scope_then_revoke() {
        let a = parse_args(&["--auth-google".into(), "calendar".into(), "--revoke".into()]);
        assert!(a.auth_google);
        assert_eq!(a.auth_google_scope.as_deref(), Some("calendar"));
        assert!(a.auth_google_revoke);
    }

    #[test]
    fn parse_args_auth_google_unknown_next_treated_as_prompt() {
        // If the next token isn't a known scope keyword we shouldn't eat
        // it — leave it for the prompt-words bucket so the rest of the
        // parser still gets to see other flags.
        let a = parse_args(&["--auth-google".into(), "nonsense".into(), "-r".into()]);
        assert!(a.auth_google);
        assert_eq!(a.auth_google_scope, None);
        assert!(
            a.resume,
            "resume flag after unknown scope should still register"
        );
        assert!(a.prompt_words.contains(&"nonsense".to_string()));
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

    #[test]
    fn parse_args_help_long() {
        let a = parse_args(&["--help".into()]);
        assert!(a.help);
        assert!(!a.version);
        assert!(a.prompt_words.is_empty());
    }

    #[test]
    fn parse_args_help_short() {
        let a = parse_args(&["-h".into()]);
        assert!(a.help);
    }

    #[test]
    fn parse_args_version_long() {
        let a = parse_args(&["--version".into()]);
        assert!(a.version);
        assert!(!a.help);
    }

    #[test]
    fn parse_args_version_short() {
        let a = parse_args(&["-V".into()]);
        assert!(a.version);
    }

    #[test]
    fn help_text_mentions_every_flag() {
        // Guardrail: if someone adds a new flag to parse_args they'll break
        // this test until HELP_TEXT documents it. Keeps the two in sync.
        for flag in [
            "--resume",
            "--telegram",
            "--tui",
            "--chat",
            "--auth-google",
            "--revoke",
            "--briefing",
            "--time",
            "--days",
            "--help",
            "--version",
        ] {
            assert!(
                HELP_TEXT.contains(flag),
                "HELP_TEXT missing documentation for {flag}"
            );
        }
    }
}
