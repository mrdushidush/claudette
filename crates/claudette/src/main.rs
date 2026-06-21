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
    probe_ollama, run_agent, run_agent_repl, run_forge_mission, theme, try_load_session,
    workspace_startup_diagnostics, SessionOptions,
};
use claudette::{ContentBlock, Session};
// External-cloud integrations: only compiled into an `integrations` build.
// See Cargo.toml `[features]` and the `dispatch_google_auth` / `dispatch_telegram`
// / `run_briefing_setup` helpers below, which carry the coding-only fallback.
// `clock` + `scheduler` are here too: their only main.rs use is the real
// `run_briefing_setup`, which is integrations-only.
#[cfg(feature = "integrations")]
use claudette::{briefing, clock, google_auth, scheduler, secrets, telegram_mode};

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
    /// `--forge`: run the trailing prompt in forge-mode inside the active
    /// brownfield mission. Errors if no mission is active. v0a is single-
    /// stage: one model turn against a pre-enabled toolset, ending at
    /// `mission_submit` (auto-PR). The prompt text is taken from
    /// `prompt_words`.
    forge: bool,
    /// `--doctor`: run flat diagnostic probes (Ollama reachable, brain
    /// pulled, recall embed model loaded, OAuth tokens valid, voice deps,
    /// secrets dir). Exits before any interactive mode starts. Non-zero
    /// exit code if any probe was a hard failure (warnings tolerated).
    doctor: bool,
}

/// Help text printed on `--help` / `-h`. One source of truth; the README
/// CLI flag table is synced to this block. Keep lines under 80 columns so
/// it fits an 80-wide terminal without wrapping awkwardly.
const HELP_TEXT: &str = "\
claudette — a local-first AI coding agent, powered by Ollama.

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
    --forge \"<prompt>\"   Run the prompt in forge-mode inside the active
                         brownfield mission. Errors if no mission is active —
                         start one with /brownfield <repo> first. v0a runs a
                         single brain turn with file/search/git/advanced/github
                         tools pre-enabled and exits at mission_submit (auto-PR).

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
    --doctor             Probe every dependency (Ollama, embed model, OAuth
                         tokens, voice deps, secrets dir) and print a
                         green/red diagnostic report. Useful when a tool
                         fails inside the REPL and you don't yet know why.
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
    --offline            Enforce the air-gap: hard-block every outbound network
                         call except the local model backend + loopback. Same
                         as CLAUDETTE_OFFLINE=1. Blocks web_search/web_fetch,
                         Gmail/Calendar, weather/wikipedia, GitHub, and
                         the Telegram bridge; the local brain + recall still
                         work. See `--offline` in --doctor for the allow-list.
    --faceless           Drop the persona overlay (Eva for the assistant,
                         CodeX-7 for the forge Coder) and run with a plain,
                         name-free prompt. Same as CLAUDETTE_FACELESS=1.
    --help, -h           Show this help and exit.
    --version, -V        Show the claudette version and exit.

ENVIRONMENT:
    See README.md for the full env-var reference. Frequently used:
      OLLAMA_HOST              Ollama API endpoint (default localhost:11434).
      CLAUDETTE_MODEL          Override the brain model.
      CLAUDETTE_CODER_MODEL    Override the Codet coder model.
      CLAUDETTE_SESSION        Override the session-file path.
      CLAUDETTE_OFFLINE        Set to 1 to enforce the air-gap (see --offline).
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
    // Load env vars from .env files. ONLY the canonical ~/.claudette/.env
    // is auto-loaded — we explicitly do NOT walk CWD or its parents, because
    // that lets any shared project directory smuggle CLAUDETTE_*, OLLAMA_HOST,
    // GITHUB_TOKEN, or TELEGRAM_BOT_TOKEN into the agent without the user
    // knowing. Per-project overrides should use shell `export`, direnv, or
    // an explicit `dotenvy::from_path(…)` call from a trusted wrapper.
    //
    // ~/.claudette/.env is where CLAUDETTE_MODEL, _NUM_CTX, _COMPACT_THRESHOLD,
    // and tokens like BRAVE_API_KEY / GITHUB_TOKEN belong. Missing file is
    // silently ignored — tools that actually need an env var will report
    // their own missing-key errors.
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

    // ── Workspace-roots startup probe ─────────────────────────────────
    // Catches the 2026-04-28 wrapper-forgot-CLAUDETTE_WORKSPACE class of
    // bug at startup rather than first read attempt. Prints to stderr
    // because the runtime may use stdout for one-shot replies; warnings
    // shouldn't pollute scriptable output.
    for warning in workspace_startup_diagnostics() {
        eprintln!(
            "{} {}",
            theme::warn(theme::WARN_GLYPH),
            theme::warn(&warning)
        );
    }

    let CliArgs {
        resume,
        telegram,
        chat_ids,
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
        forge,
        doctor,
    } = args;

    // ── Offline-mode banner ───────────────────────────────────────────
    // Loud, one-line confirmation that the air-gap is enforced this run.
    // `--doctor` prints its own dedicated offline section, so skip it here
    // to avoid saying it twice.
    if claudette::egress::is_offline() && !doctor {
        eprintln!(
            "{} {}",
            theme::accent("🔒 offline mode"),
            theme::dim(&format!(
                "only the local backend ({}) + loopback are reachable; all other egress is blocked",
                claudette::api::resolve_ollama_url()
            ))
        );
    }

    // ── --doctor: full diagnostic probe ──────────────────────────────
    // Runs before every other branch because it's the command the user
    // reaches for when something is already broken — it must work even
    // if the Ollama probe in [`probe_ollama`] below would otherwise
    // refuse to start. Exits with the doctor module's own status code.
    if doctor {
        let code = claudette::doctor::run();
        return if code == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    // ── Google OAuth flow ─────────────────────────────────────────────
    // Runs before the Ollama probe because it's a one-shot setup command —
    // the user doesn't need the brain running to grant OAuth consent.
    if auth_google {
        return dispatch_google_auth(auth_google_scope.as_deref(), auth_google_revoke);
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
    //
    // In an interactive terminal (and never under --offline), offer to fix
    // the failure on the spot — classify the cause and prompt `[Y/n]` to
    // `ollama pull` a missing brain. Non-interactive / piped / CI runs take
    // the exact pre-existing path: print the error, exit non-zero.
    if let Err(msg) = probe_ollama() {
        eprintln!("{} {}", theme::error(theme::ERR_GLYPH), theme::error(&msg));
        if !claudette::firstrun::offer_fix_interactive() {
            return ExitCode::FAILURE;
        }
    }

    // ── Forge-mode (single-stage v0a) ─────────────────────────────────
    // Runs the trailing prompt against the active brownfield mission and
    // exits when the brain finishes (typically after `mission_submit`
    // returns). Errors before touching the runtime if no mission is active
    // or the prompt is empty.
    if forge {
        if prompt_args.is_empty() {
            eprintln!(
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(
                    "--forge requires a prompt. Try: claudette --forge \"fix the parser bug\""
                )
            );
            return ExitCode::FAILURE;
        }
        let opts = SessionOptions {
            resume,
            autosave: resume,
        };
        let prompt = prompt_args.join(" ");
        return match run_forge_mission(&prompt, opts) {
            Ok(summary) => {
                eprintln!();
                eprintln!(
                    "{} {}",
                    theme::BOLT,
                    theme::info(&format!(
                        "forge iter={} in={} out={}",
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
        };
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
        dispatch_telegram(chat_ids, allow_any_chat, resume)
    } else if prompt_args.is_empty() {
        // No prompt → interactive REPL. REPL always autosaves.
        let opts = SessionOptions {
            resume,
            autosave: true,
        };
        match run_agent_repl(opts) {
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
        match run_agent(&prompt, opts) {
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
                // (Coding-only builds have no scope keywords — the token
                // always falls through as a normal arg.)
                expect = ExpectNext::Nothing;
                #[cfg(feature = "integrations")]
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
            "--forge" => out.forge = true,
            "--doctor" => out.doctor = true,
            "--faceless" => {
                // Persona overlay opt-out (Eva for assistant, CodeX-7 for
                // forge Coder). Set the env var so the secretary prompt
                // builder picks it up — same surface as
                // `CLAUDETTE_FACELESS=1`.
                std::env::set_var("CLAUDETTE_FACELESS", "1");
            }
            "--offline" => {
                // Enforced air-gap. Set the env var so the egress guard (and
                // any subprocess we spawn) sees it — same surface as
                // `CLAUDETTE_OFFLINE=1`. See `egress.rs`.
                std::env::set_var(claudette::egress::OFFLINE_ENV, "1");
            }
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

/// Run the Google OAuth loopback flow (or revoke). The real implementation is
/// only compiled into a default-features build; the coding-only build (built
/// `--no-default-features`) carries no Google code at all, so this stub just
/// explains that and exits non-zero.
#[cfg(feature = "integrations")]
fn dispatch_google_auth(scope: Option<&str>, revoke: bool) -> ExitCode {
    let ctx = match scope {
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
    let result = if revoke {
        google_auth::revoke(ctx)
    } else {
        google_auth::run_auth_flow(ctx)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{} {}", theme::error(theme::ERR_GLYPH), theme::error(&e));
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(feature = "integrations"))]
fn dispatch_google_auth(_scope: Option<&str>, _revoke: bool) -> ExitCode {
    eprintln!(
        "{} {}",
        theme::error(theme::ERR_GLYPH),
        theme::error(
            "--auth-google needs the `integrations` feature — this is a coding-only build (the \
             default), so there is no Google code in it. Reinstall with `--features integrations` \
             to use Gmail/Calendar."
        )
    );
    ExitCode::FAILURE
}

/// Run the Telegram bot. Real implementation only in a default-features build;
/// the coding-only build carries no Telegram bridge, so the stub exits non-zero
/// with an explanation.
#[cfg(feature = "integrations")]
fn dispatch_telegram(mut chat_ids: Vec<i64>, allow_any_chat: bool, resume: bool) -> ExitCode {
    // The Telegram bridge is a cloud service (api.telegram.org); it is
    // fundamentally incompatible with an enforced air-gap. Refuse up front
    // with the same vocabulary the egress guard uses, rather than letting
    // the bot start and then fail every poll.
    if claudette::egress::is_offline() {
        eprintln!(
            "{} {}",
            theme::error(theme::ERR_GLYPH),
            theme::error(
                "offline mode (--offline) blocks the Telegram bridge — it relays through \
                 api.telegram.org (cloud). Drop --offline to run the bot, or drop --telegram \
                 to stay air-gapped."
            )
        );
        return ExitCode::FAILURE;
    }
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
}

#[cfg(not(feature = "integrations"))]
fn dispatch_telegram(_chat_ids: Vec<i64>, _allow_any_chat: bool, _resume: bool) -> ExitCode {
    eprintln!(
        "{} {}",
        theme::error(theme::ERR_GLYPH),
        theme::error(
            "--telegram needs the `integrations` feature — this is a coding-only build (the \
             default), so the Telegram bridge isn't in it. Reinstall with `--features \
             integrations` to run the bot."
        )
    );
    ExitCode::FAILURE
}

/// Create (or replace) the scheduled morning-briefing entry. Used by the
/// `--briefing` CLI flag. Does not talk to Telegram at all — the running
/// bot picks the new entry up on its next startup (or immediately if
/// already running, since tools write to the same jsonl).
///
/// The briefing is part of the personal-assistant surface, so the real
/// implementation is only present in an `integrations` build; the coding-only
/// build carries the stub below.
#[cfg(feature = "integrations")]
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

/// Coding-only fallback: the morning briefing is part of the assistant surface,
/// which isn't compiled into a default (no-`integrations`) build. Explain and
/// exit non-zero rather than silently doing nothing.
#[cfg(not(feature = "integrations"))]
fn run_briefing_setup(_time: Option<&str>, _days: Option<&str>) -> ExitCode {
    eprintln!(
        "{} {}",
        theme::error(theme::ERR_GLYPH),
        theme::error(
            "--briefing needs the `integrations` feature — this is a coding-only build, so the \
             morning-briefing helper isn't in it. Reinstall with `--features integrations` to \
             schedule briefings."
        )
    );
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `parse_args` reads CLAUDETTE_TELEGRAM_CHAT, so the test that SETS it and
    // every test that asserts the default (unset) chat behaviour must serialise
    // on this lock — otherwise they race in the shared `--bins` test process and
    // a reader sees the setter's "ANY" (observed as a flaky CI failure of
    // `parse_args_telegram_with_chat`). This binary crate can't reach the lib's
    // `test_env_lock`, so it keeps its own.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `parse_args` with CLAUDETTE_TELEGRAM_CHAT pinned UNSET, under the env
    /// lock, restoring the previous value afterwards. Use for any test that
    /// asserts chat-allowlist / accept-all defaults.
    fn parse_args_clean(args: &[String]) -> CliArgs {
        let _g = lock_env();
        let prev = std::env::var("CLAUDETTE_TELEGRAM_CHAT").ok();
        std::env::remove_var("CLAUDETTE_TELEGRAM_CHAT");
        let out = parse_args(args);
        if let Some(v) = prev {
            std::env::set_var("CLAUDETTE_TELEGRAM_CHAT", v);
        }
        out
    }

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
    fn parse_args_offline_sets_env_and_is_not_a_prompt_word() {
        let _g = lock_env();
        let prev = std::env::var(claudette::egress::OFFLINE_ENV).ok();
        std::env::remove_var(claudette::egress::OFFLINE_ENV);

        // The flag is consumed (not echoed into the prompt) and flips the env
        // var the egress guard reads — same mechanism as --faceless.
        let a = parse_args(&["--offline".into(), "fix".into(), "bug".into()]);
        assert!(
            claudette::egress::is_offline(),
            "--offline enables offline mode"
        );
        assert_eq!(a.prompt_words, vec!["fix".to_string(), "bug".to_string()]);

        match prev {
            Some(v) => std::env::set_var(claudette::egress::OFFLINE_ENV, v),
            None => std::env::remove_var(claudette::egress::OFFLINE_ENV),
        }
    }

    #[test]
    fn parse_args_telegram_with_chat() {
        let a = parse_args_clean(&[
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
        let a = parse_args_clean(&["--telegram".into(), "--chat".into(), "any".into()]);
        assert!(a.telegram);
        assert!(a.allow_any_chat);
        assert!(a.chat_ids.is_empty());
    }

    #[test]
    fn parse_args_chat_any_case_insensitive() {
        let a = parse_args_clean(&["--telegram".into(), "--chat".into(), "ANY".into()]);
        assert!(a.allow_any_chat);
    }

    #[test]
    fn parse_args_chat_env_any_sets_accept_all() {
        // CLAUDETTE_TELEGRAM_CHAT also accepts the "any" sentinel. Hold the env
        // lock across set→parse→restore so a concurrent reader (e.g.
        // parse_args_telegram_with_chat) can't observe the "ANY" mid-flight.
        let _g = lock_env();
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

    // Scope keywords (`gmail` / `calendar`) are only recognised when the
    // `integrations` feature compiles in `google_auth::AuthContext`; the
    // coding-only build deliberately lets the token fall through as a prompt
    // word (see the parse loop's `ExpectNext::AuthGoogleScope` arm).
    #[cfg(feature = "integrations")]
    #[test]
    fn parse_args_auth_google_with_gmail_scope() {
        let a = parse_args(&["--auth-google".into(), "gmail".into()]);
        assert!(a.auth_google);
        assert_eq!(a.auth_google_scope.as_deref(), Some("gmail"));
        assert!(a.prompt_words.is_empty());
    }

    #[cfg(not(feature = "integrations"))]
    #[test]
    fn parse_args_auth_google_scope_falls_through_in_coding_only_build() {
        // No Google scope keywords exist here, so `gmail` is just a prompt word.
        let a = parse_args(&["--auth-google".into(), "gmail".into()]);
        assert!(a.auth_google);
        assert_eq!(a.auth_google_scope, None);
        assert_eq!(a.prompt_words, vec!["gmail".to_string()]);
    }

    #[cfg(feature = "integrations")]
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
        // Guardrail: every long flag parse_args matches must be documented in
        // HELP_TEXT. This list must mirror the `"--*"` arms in parse_args
        // (`grep -oE '"--[a-z-]+"' main.rs | sort -u`) — adding a flag without
        // a HELP_TEXT line breaks this test on purpose. Previously it checked
        // only 11 of 15 and silently let --forge/--doctor/--faceless/--offline
        // ship undocumented.
        for flag in [
            "--resume",
            "--telegram",
            "--tui",
            "--forge",
            "--doctor",
            "--faceless",
            "--offline",
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

    /// Doc-drift guard (roast 2026-06-21, Wave 2.3): every `CLAUDETTE_*` env
    /// var read in `src/` must be documented somewhere under `docs/`. New knob
    /// without a doc line → this test fails until you document it (or add it to
    /// `ALLOW` with a reason). Keeps `configuration.md`'s "every env var"
    /// promise honest.
    #[test]
    fn every_env_var_is_documented() {
        use std::collections::BTreeSet;
        use std::path::Path;

        /// Recursively append every `*.<ext>` file's contents under `dir`.
        fn slurp(dir: &Path, ext: &str, out: &mut String) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    slurp(&path, ext, out);
                } else if path.extension().and_then(|s| s.to_str()) == Some(ext) {
                    if let Ok(s) = std::fs::read_to_string(&path) {
                        out.push_str(&s);
                        out.push('\n');
                    }
                }
            }
        }

        /// Pull every `CLAUDETTE_[A-Z0-9_]+` token out of `hay`.
        fn vars(hay: &str) -> BTreeSet<String> {
            let bytes = hay.as_bytes();
            let mut set = BTreeSet::new();
            let mut search_from = 0;
            while let Some(rel) = hay[search_from..].find("CLAUDETTE_") {
                let start = search_from + rel;
                let mut end = start + "CLAUDETTE_".len();
                while end < bytes.len()
                    && (bytes[end].is_ascii_uppercase()
                        || bytes[end].is_ascii_digit()
                        || bytes[end] == b'_')
                {
                    end += 1;
                }
                set.insert(hay[start..end].to_string());
                search_from = end;
            }
            set
        }

        // Vars the guard intentionally ignores (not user-facing config).
        const ALLOW: &[&str] = &[
            "CLAUDETTE_ZZZ_TEST_NONEXISTENT_ABC_TOKEN", // secrets.rs test sentinel
            "CLAUDETTE_FORGE_X",                        // sample code in a repomap test fixture
            "CLAUDETTE_FORGE_Y",                        // ditto
        ];

        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut code = String::new();
        slurp(&manifest.join("src"), "rs", &mut code);
        let mut docs = String::new();
        slurp(&manifest.join("../../docs"), "md", &mut docs);

        // If the doc tree isn't on disk (e.g. a packaged build with tests/
        // and docs/ stripped), there's nothing to check against — skip.
        if docs.is_empty() {
            return;
        }

        let documented = vars(&docs);
        let mut missing: Vec<String> = vars(&code)
            .into_iter()
            // Captures ending in `_` are dynamic prefixes (e.g. the
            // `CLAUDETTE_CODER_`/`CLAUDETTE_FORGE_` string fragments used to
            // build var names), not real vars.
            .filter(|v| !v.ends_with('_'))
            .filter(|v| !ALLOW.contains(&v.as_str()))
            .filter(|v| !documented.contains(v))
            .collect();
        missing.sort();
        assert!(
            missing.is_empty(),
            "these CLAUDETTE_* env vars are read in src/ but documented in no docs/*.md \
             (add them to docs/configuration.md, or to ALLOW with a reason):\n{}",
            missing.join("\n")
        );
    }
}
