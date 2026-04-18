//! Top-level entry points — single-shot and REPL.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use crate::{
    compact_session, estimate_session_tokens, CompactionConfig, ConversationRuntime,
    PermissionMode, PermissionPolicy, PermissionPromptDecision, PermissionPrompter,
    PermissionRequest, Session, TurnSummary,
};

use crate::api::{stdout_text_callback, OllamaApiClient};
use crate::commands::{dispatch_slash_command, parse_slash_command, ReplState, SlashOutcome};
use crate::executor::SecretaryToolExecutor;
use crate::memory::try_load_memory;
use crate::model_config;
use crate::prompt::secretary_system_prompt_with_memory;
use crate::theme;
use crate::tool_groups::ToolRegistry;

// Brain default now lives in `model_config::ModelConfig::from_preset`. The
// Auto preset (qwen3.5:4b brain + qwen3.5:9b fallback, shipped Sprint 14)
// replaces the `DEFAULT_MODEL = "qwen3:8b"` constant that used to live
// here — callers should use `current_model()` or `model_config::active()`.

/// Estimated-tokens threshold at which the REPL fires its own compaction
/// pass (heuristic summarisation of the oldest messages).
///
/// **Why the metric changed (2026-04-09):** previously we used
/// the runtime's built-in trigger which fires on
/// `cumulative_input_tokens`. That metric grows monotonically — with Ollama
/// sending the entire history every turn, cumulative input crosses any
/// fixed threshold within ~3 turns and then NEVER falls back below it,
/// because the usage tracker doesn't subtract removed-message tokens after
/// a compact. Result: every subsequent turn fired auto-compaction even
/// though the session itself was small. a user's transcript on 2026-04-09
/// caught this — six consecutive turns each removing 5 messages.
///
/// The fix: bypass the runtime's trigger (set its threshold to
/// `u32::MAX` in [`build_runtime`]) and roll our own in
/// [`maybe_compact_session`], using `estimate_session_tokens(session)` —
/// a metric that's actually bounded by the current session size and
/// drops back below the threshold after a successful compact.
///
/// Default `12_000` ≈ 73% of the 16 K `num_ctx` window, leaving headroom
/// for the system prompt (~500 tokens) and tool schemas (~2-4K tokens
/// depending on enabled groups). Override via `CLAUDETTE_COMPACT_THRESHOLD`.
pub const DEFAULT_COMPACT_THRESHOLD: usize = 12_000;

/// Resolve the compaction threshold the REPL is currently using — honors
/// the `CLAUDETTE_COMPACT_THRESHOLD` env var, falls back to
/// [`DEFAULT_COMPACT_THRESHOLD`]. Public so the `get_capabilities` tool
/// and the `/status` slash command can report the same value the REPL
/// is actually checking against.
#[must_use]
pub fn compact_threshold() -> usize {
    std::env::var("CLAUDETTE_COMPACT_THRESHOLD")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_COMPACT_THRESHOLD)
}

/// Resolve the model name the runtime is currently using. Sprint 14: this
/// now delegates to `model_config::active().brain.model`, so once a
/// `/preset` or `/brain` slash command mutates the active config, every
/// caller (`/status`, `/capabilities`, `get_capabilities` tool) immediately
/// sees the new value. The preset resolution still honors
/// `CLAUDETTE_MODEL` env var because `ModelConfig::resolve` merges env
/// into the default Auto preset at first access.
#[must_use]
pub fn current_model() -> String {
    model_config::active().brain.model
}

/// Caller-supplied options for session persistence. Kept as a struct (rather
/// than a pile of bool args) so adding e.g. `session_path: Option<PathBuf>`
/// later is non-breaking.
#[derive(Debug, Clone, Default)]
pub struct SessionOptions {
    /// If true, attempt to load the saved session before the first turn.
    /// Errors out if the session file is missing.
    pub resume: bool,
    /// If true, persist the session to disk after every turn.
    /// REPL mode sets this unconditionally; single-shot only sets it when
    /// `--resume` was passed (so a one-off invocation can't clobber a long
    /// REPL conversation).
    pub autosave: bool,
}

/// Resolve where the secretary's session file lives. Honors the
/// `CLAUDETTE_SESSION` env var (full path); otherwise falls back to
/// `~/.claudette/sessions/last.json`. We use a single fixed path so
/// `--resume` is unambiguous; named sessions can come later if useful.
#[must_use]
pub fn default_session_path() -> PathBuf {
    if let Ok(custom) = std::env::var("CLAUDETTE_SESSION") {
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    sessions_dir().join("last.json")
}

/// Resolve the directory holding all session JSON files. `pub(crate)` so the
/// slash-command dispatcher can list / save / load named sessions under it.
pub(crate) fn sessions_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claudette").join("sessions")
}

/// Try to load a saved session from the default path. Returns
/// `Ok(Some(session))` if it loaded, `Ok(None)` if the file doesn't exist,
/// `Err` if it exists but is corrupt.
pub fn try_load_session() -> Result<Option<Session>> {
    try_load_session_at(&default_session_path())
}

/// Same as `try_load_session` but reads from a caller-supplied path. Lets
/// tests avoid touching `CLAUDETTE_SESSION` (which is process-global and
/// races between parallel tests).
pub fn try_load_session_at(path: &std::path::Path) -> Result<Option<Session>> {
    if !path.exists() {
        return Ok(None);
    }
    let session = Session::load_from_path(path)
        .with_context(|| format!("failed to load session from {}", path.display()))?;
    Ok(Some(session))
}

/// Persist `session` to the default path, creating the parent directory if
/// needed. Best-effort: returns the error to the caller so the REPL can
/// surface it once and continue.
pub fn save_session(session: &Session) -> Result<()> {
    save_session_at(session, &default_session_path())
}

/// Same as `save_session` but writes to a caller-supplied path.
pub fn save_session_at(session: &Session, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    session
        .save_to_path(path)
        .with_context(|| format!("failed to save session to {}", path.display()))?;
    Ok(())
}

/// Run a single user turn through the secretary agent loop and return the
/// turn summary. With `opts.resume = true`, loads the saved session first.
/// With `opts.autosave = true`, writes the session back after the turn.
pub fn run_secretary(user_input: &str, opts: SessionOptions) -> Result<TurnSummary> {
    let session = if opts.resume {
        try_load_session()?
            .ok_or_else(|| anyhow::anyhow!("no saved session at {}", default_session_path().display()))?
    } else {
        Session::default()
    };

    let mut runtime = build_runtime(session);
    // Stash any file paths from the raw user prompt — bypasses the brain's
    // tendency to drop them when constructing tool-call arguments.
    crate::tools::set_current_turn_paths(crate::tools::extract_user_prompt_paths(user_input));

    // Sprint 14: even single-shot runs go through the fallback wrapper so
    // brain100 / brownfield benchmarks can measure Auto-preset escalation
    // behaviour. On Fast / Smart presets (no fallback configured) this
    // reduces to the prior `run_turn` + empty-response retry.
    let mut no_prompter: Option<&mut dyn PermissionPrompter> = None;
    let summary = crate::brain_selector::run_turn_with_fallback(
        &mut runtime,
        user_input,
        &mut no_prompter,
    )
    .map_err(|e| anyhow::anyhow!("secretary turn failed: {e}"))?;

    // Same session-size trigger as the REPL — fire after the turn so the
    // session we autosave (when --resume is set) is already trimmed.
    if let Some(removed) = maybe_compact_session(&mut runtime, false) {
        eprintln!("[auto-compacted {removed} older message(s)]");
    }

    if opts.autosave {
        save_session(runtime.session())?;
    }
    Ok(summary)
}

/// Run an interactive REPL against a single long-lived `ConversationRuntime`.
/// Reads lines from stdin, runs each as a turn, prints the assistant's reply.
/// Lines starting with `/` are interpreted as slash commands (see
/// `commands.rs`) and never reach the model. Exits on EOF, the `/exit`
/// command, or the bare words `exit`/`quit`/`:q` (kept for muscle memory).
/// Always autosaves after every model turn when `opts.autosave` is set.
pub fn run_secretary_repl(opts: SessionOptions) -> Result<()> {
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

    eprintln!(
        "{} {} {}",
        theme::ROBOT,
        theme::brand("claudette"),
        theme::dim("— your local secretary")
    );
    eprintln!(
        "{} {}",
        theme::SPARKLES,
        theme::dim("type /help for commands, /exit (or Ctrl-D) to leave")
    );
    eprintln!(
        "{} {}",
        theme::SAVE,
        theme::dim(&format!(
            "session: {}",
            default_session_path().display()
        ))
    );
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
            match dispatch_slash_command(cmd, &mut runtime, &state) {
                SlashOutcome::Continue => continue,
                SlashOutcome::Exit => break,
            }
        }

        crate::tools::set_current_turn_paths(crate::tools::extract_user_prompt_paths(trimmed));

        // Sprint 14: route through brain_selector so Auto-preset turns get
        // the 4b → 9b escalation when stuck signals fire. On Fast/Smart
        // (no fallback configured) this collapses to the existing
        // run_turn_with_retry behaviour — no overhead.
        let mut prompter_opt: Option<&mut dyn PermissionPrompter> = Some(&mut prompter);
        match crate::brain_selector::run_turn_with_fallback(
            &mut runtime,
            trimmed,
            &mut prompter_opt,
        ) {
            Ok(summary) => {
                // No post-turn re-print: streaming has already pushed every
                // text delta to stdout via `stdout_text_callback`. The model's
                // text terminator newline is also fired by the callback at
                // end-of-stream, so the status line below lands on its own row.

                state.record_turn(summary.usage.input_tokens, summary.usage.output_tokens);
                eprintln!(
                    "{} {}",
                    theme::BOLT,
                    theme::info(&format!(
                        "turn iter={} in={} out={}",
                        summary.iterations,
                        summary.usage.input_tokens,
                        summary.usage.output_tokens,
                    ))
                );

                // the runtime's built-in trigger is disabled (see
                // build_runtime_inner) — we fire our own session-size trigger
                // here instead, AFTER the turn so the model never sees a
                // mid-turn rebuild.
                if let Some(removed) = maybe_compact_session(&mut runtime, false) {
                    eprintln!(
                        "{} {}",
                        theme::SAVE,
                        theme::ok(&format!(
                            "auto-compacted {removed} older message(s) — session was over {}-token threshold",
                            compact_threshold(),
                        ))
                    );
                }

                if opts.autosave {
                    if let Err(e) = save_session(runtime.session()) {
                        // Surface the error but don't drop the REPL — the
                        // session in memory is still valid; only persistence
                        // is broken.
                        eprintln!(
                            "{} {}",
                            theme::warn(theme::WARN_GLYPH),
                            theme::warn(&format!("session save failed: {e:#}"))
                        );
                    }
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
    }

    Ok(())
}

/// Assemble a `ConversationRuntime` with the secretary's model, tools,
/// executor, prompt, and a permissive policy, around the given session
/// (fresh or restored). Loads `~/.claudette/CLAUDETTE.MD` (if present)
/// and appends it to the system prompt as background memory.
///
/// **No streaming callback installed** — use this from single-shot mode and
/// tests, where the assistant's text is printed via `summary.assistant_messages`
/// after the turn completes. The REPL should call [`build_runtime_streaming`]
/// instead.
///
/// `pub(crate)` so the slash-command dispatcher can rebuild the runtime
/// in-place when the user runs `/reload` (which re-reads the memory file
/// without dropping the conversation history).
pub(crate) fn build_runtime(
    session: Session,
) -> ConversationRuntime<OllamaApiClient, SecretaryToolExecutor> {
    build_runtime_inner(session, false, false)
}

/// Same as [`build_runtime`] but installs the stdout streaming callback so
/// text deltas appear in the terminal as they arrive. Used by the REPL and
/// by every slash command that rebuilds the runtime in place
/// (`/clear`, `/load`, `/reload`, `/compact`).
pub(crate) fn build_runtime_streaming(
    session: Session,
    telegram: bool,
) -> ConversationRuntime<OllamaApiClient, SecretaryToolExecutor> {
    build_runtime_inner(session, true, telegram)
}

fn build_runtime_inner(
    session: Session,
    streaming: bool,
    telegram: bool,
) -> ConversationRuntime<OllamaApiClient, SecretaryToolExecutor> {
    // Sprint 14: pull brain model + limits from the process-global
    // `model_config::active()` snapshot. Slash commands (`/preset`,
    // `/brain`) mutate the active config; the next `build_runtime_*`
    // call (e.g. after `/clear`, `/reload`, or after a fallback turn)
    // picks up the new values.
    let brain = model_config::active().brain;
    build_runtime_with_brain(session, &brain, streaming, telegram)
}

/// Sprint 14: explicit-brain variant of [`build_runtime_streaming`].
/// Used by `brain_selector` to spin up a fallback runtime against a
/// different model (e.g. qwen3.5:9b) while reusing the same session +
/// permission policy + system prompt. `pub(crate)` so it stays internal.
pub(crate) fn build_runtime_with_brain(
    session: Session,
    brain: &crate::model_config::RoleConfig,
    streaming: bool,
    telegram: bool,
) -> ConversationRuntime<OllamaApiClient, SecretaryToolExecutor> {
    // Sprint 8: one shared ToolRegistry is the single source of truth for
    // the `tools` field on every request. The API client reads from it
    // (via ToolsProvider::Dynamic) and the executor mutates it when the
    // model calls `enable_tools`. Both halves hold a clone of the Arc so
    // the mutations are immediately visible on the next chat turn.
    let mut reg = ToolRegistry::new();

    // In Telegram mode, pre-enable the most-used groups so the model can
    // call tools directly without the enable_tools → tool two-step dance.
    // Cost: ~3K tokens of schema (~18% of 16K context). Worth it for the
    // single-iteration tool calls.
    if telegram {
        use crate::tool_groups::ToolGroup;
        reg.enable(ToolGroup::Markets);
        reg.enable(ToolGroup::Facts);
        reg.enable(ToolGroup::Advanced);
        reg.enable(ToolGroup::Git);
        reg.enable(ToolGroup::Search);
    }

    let registry = Arc::new(Mutex::new(reg));

    let mut api_client = OllamaApiClient::with_registry(brain.model.clone(), registry.clone())
        .with_context(brain.num_ctx)
        .with_max_predict(brain.num_predict);
    if streaming {
        api_client = api_client.with_text_callback(stdout_text_callback());
    }
    let executor = SecretaryToolExecutor::with_registry(registry);
    let policy = build_permission_policy();
    let memory = try_load_memory();

    ConversationRuntime::new(
        session,
        api_client,
        executor,
        policy,
        secretary_system_prompt_with_memory(memory.as_deref(), telegram),
    )
    // Tools in optional groups need 3+ iterations (enable_tools → tool call
    // → respond). With the empty-response retry nudge, 8 was too tight for
    // single-shot search/grep/git chains. 15 matches TUI and Telegram.
    .with_max_iterations(15)
    .with_auto_compaction_input_tokens_threshold(u32::MAX)
}

// ────────────────────────────────────────────────────────────────────────────
// Permission system
// ────────────────────────────────────────────────────────────────────────────

/// Build the per-tool permission policy. Active mode is `WorkspaceWrite`:
/// read-only and workspace-write tools pass through silently, but tools
/// tagged `DangerFullAccess` trigger the CLI prompter for `[y/N]`
/// confirmation before executing.
pub(crate) fn build_permission_policy() -> PermissionPolicy {
    use PermissionMode::{DangerFullAccess, ReadOnly, WorkspaceWrite};

    PermissionPolicy::new(WorkspaceWrite)
        // ── Read-only (auto-allowed) ────────────────────────────────
        .with_tool_requirement("get_current_time", ReadOnly)
        .with_tool_requirement("note_list", ReadOnly)
        .with_tool_requirement("note_read", ReadOnly)
        .with_tool_requirement("todo_list", ReadOnly)
        // enable_tools: meta-tool, pure in-memory state change, no IO
        .with_tool_requirement("enable_tools", ReadOnly)
        .with_tool_requirement("read_file", ReadOnly)
        .with_tool_requirement("list_dir", ReadOnly)
        .with_tool_requirement("get_capabilities", ReadOnly)
        .with_tool_requirement("glob_search", ReadOnly)
        .with_tool_requirement("grep_search", ReadOnly)
        .with_tool_requirement("git_status", ReadOnly)
        .with_tool_requirement("git_diff", ReadOnly)
        .with_tool_requirement("git_log", ReadOnly)
        .with_tool_requirement("git_branch", ReadOnly)
        // ── Workspace-write (auto-allowed) ──────────────────────────
        .with_tool_requirement("note_create", WorkspaceWrite)
        .with_tool_requirement("note_delete", WorkspaceWrite)
        .with_tool_requirement("todo_add", WorkspaceWrite)
        .with_tool_requirement("todo_complete", WorkspaceWrite)
        .with_tool_requirement("todo_uncomplete", WorkspaceWrite)
        .with_tool_requirement("todo_delete", WorkspaceWrite)
        .with_tool_requirement("write_file", WorkspaceWrite)
        .with_tool_requirement("generate_code", WorkspaceWrite)
        .with_tool_requirement("web_search", WorkspaceWrite)
        .with_tool_requirement("web_fetch", WorkspaceWrite)
        .with_tool_requirement("open_in_editor", WorkspaceWrite)
        .with_tool_requirement("reveal_in_explorer", WorkspaceWrite)
        .with_tool_requirement("open_url", WorkspaceWrite)
        .with_tool_requirement("add_numbers", WorkspaceWrite)
        .with_tool_requirement("spawn_agent", WorkspaceWrite)
        // ── Sprint 9 Phase 0a: facts group (read-only REST calls) ───
        .with_tool_requirement("wikipedia_search", ReadOnly)
        .with_tool_requirement("wikipedia_summary", ReadOnly)
        .with_tool_requirement("weather_current", ReadOnly)
        .with_tool_requirement("weather_forecast", ReadOnly)
        // ── Sprint 9 Phase 0a: registry group (read-only) ────────────
        .with_tool_requirement("crate_info", ReadOnly)
        .with_tool_requirement("crate_search", ReadOnly)
        .with_tool_requirement("npm_info", ReadOnly)
        .with_tool_requirement("npm_search", ReadOnly)
        // ── Sprint 9 Phase 0a: github group ──────────────────────────
        // Reads: auto-allowed. Writes: WorkspaceWrite (hit the network
        // on the user's behalf but don't touch the filesystem).
        .with_tool_requirement("gh_list_my_prs", ReadOnly)
        .with_tool_requirement("gh_list_assigned_issues", ReadOnly)
        .with_tool_requirement("gh_get_issue", ReadOnly)
        .with_tool_requirement("gh_search_code", ReadOnly)
        .with_tool_requirement("gh_create_issue", WorkspaceWrite)
        .with_tool_requirement("gh_comment_issue", WorkspaceWrite)
        // ── Sprint 9 Phase 0b: markets group (all read-only) ─────────
        .with_tool_requirement("tv_get_quote", ReadOnly)
        .with_tool_requirement("tv_technical_rating", ReadOnly)
        .with_tool_requirement("tv_search_symbol", ReadOnly)
        .with_tool_requirement("tv_economic_calendar", ReadOnly)
        .with_tool_requirement("vestige_asa_info", ReadOnly)
        .with_tool_requirement("vestige_search_asa", ReadOnly)
        .with_tool_requirement("vestige_top_movers", ReadOnly)
        // ── Sprint 10: telegram group ────────────────────────────────
        // Reads: auto-allowed. Sends: WorkspaceWrite (posts messages on
        // the user's behalf but doesn't touch the filesystem).
        .with_tool_requirement("tg_get_updates", ReadOnly)
        .with_tool_requirement("tg_send", WorkspaceWrite)
        .with_tool_requirement("tg_send_photo", WorkspaceWrite)
        // ── Dangerous (ALWAYS prompts for [y/N] confirmation) ────��──
        .with_tool_requirement("bash", DangerFullAccess)
        .with_tool_requirement("edit_file", DangerFullAccess)
        .with_tool_requirement("git_add", DangerFullAccess)
        .with_tool_requirement("git_commit", DangerFullAccess)
        .with_tool_requirement("git_push", DangerFullAccess)
        .with_tool_requirement("git_checkout", DangerFullAccess)
}

/// Interactive CLI prompter. Prints tool name + a preview of the input,
/// asks `[y/N]`, reads one line from stdin. Used by the REPL and by
/// spawned agents in normal mode (dangerous tools bubble up to the user).
/// The single-shot path passes `None` (no prompter → dangerous tools denied).
pub struct CliPrompter;

impl PermissionPrompter for CliPrompter {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
        let stderr = io::stderr();
        let mut err = stderr.lock();
        let _ = writeln!(err);
        let _ = writeln!(
            err,
            "  {} {} wants to run: {}",
            theme::warn(theme::WARN_GLYPH),
            theme::accent(&request.tool_name),
            theme::dim(
                &request
                    .input
                    .chars()
                    .take(200)
                    .collect::<String>()
            )
        );
        let _ = write!(err, "  Allow? [y/N] ");
        let _ = err.flush();

        let stdin = io::stdin();
        let mut buf = String::new();
        match stdin.read_line(&mut buf) {
            Ok(_) => {
                let answer = buf.trim().to_lowercase();
                if answer == "y" || answer == "yes" {
                    PermissionPromptDecision::Allow
                } else {
                    PermissionPromptDecision::Deny {
                        reason: "user denied permission".to_string(),
                    }
                }
            }
            Err(_) => PermissionPromptDecision::Deny {
                reason: "could not read user input".to_string(),
            },
        }
    }
}

/// The nudge message appended when the model returns an empty response.
/// Tells the model to use `enable_tools` instead of giving up.
const EMPTY_RESPONSE_NUDGE: &str =
    "Your response was empty. If you need a tool that isn't available, \
     call enable_tools(group) to load it first, then call the tool. \
     Otherwise, answer the question directly with text.";

/// Run a turn with auto-retry on empty response. When the model returns
/// "no content" (common when qwen3:8b wants a tool not in the current
/// schema), this injects a nudge message and retries once. Both the REPL
/// and Telegram mode use this.
pub(crate) fn run_turn_with_retry(
    runtime: &mut ConversationRuntime<OllamaApiClient, SecretaryToolExecutor>,
    input: &str,
    prompter: Option<&mut dyn PermissionPrompter>,
) -> Result<TurnSummary, String> {
    // Stash any file paths from the raw user input — covers Telegram (its
    // single call site) plus any future caller of run_turn_with_retry.
    crate::tools::set_current_turn_paths(crate::tools::extract_user_prompt_paths(input));

    // First attempt.
    match runtime.run_turn(input, prompter) {
        Ok(summary) => return Ok(summary),
        Err(e) => {
            let msg = e.to_string();
            if !msg.contains("no content") {
                return Err(msg);
            }
            // Empty response — retry with a nudge.
            eprintln!(
                "  {} {}",
                theme::dim("▸"),
                theme::dim("empty response — retrying with enable_tools hint...")
            );
        }
    }
    // Retry: feed the nudge as a new user turn so the model gets another chance.
    // No prompter on retry — the nudge is a system-level message, not user input.
    runtime
        .run_turn(EMPTY_RESPONSE_NUDGE, None)
        .map_err(|e| e.to_string())
}

/// Check whether the runtime's session is over the
/// [`compact_threshold`] and, if so, compact it in place. Returns
/// `Some(removed)` if compaction happened, `None` otherwise.
///
/// Called from [`run_secretary_repl`] after every model turn. The metric
/// is `crate::estimate_session_tokens` (a char-count heuristic that
/// scales with the actual session size), not the cumulative input-token
/// counter that grows monotonically.
pub(crate) fn maybe_compact_session(
    runtime: &mut ConversationRuntime<OllamaApiClient, SecretaryToolExecutor>,
    telegram: bool,
) -> Option<usize> {
    let estimated = estimate_session_tokens(runtime.session());
    if estimated < compact_threshold() {
        return None;
    }
    let result = compact_session(
        runtime.session(),
        CompactionConfig {
            preserve_recent_messages: 4,
            // 0 means "force the should_compact gate" — we're already past
            // the size threshold so we want compaction to actually fire.
            max_estimated_tokens: 0,
        },
    );
    if result.removed_message_count == 0 {
        return None;
    }
    let removed = result.removed_message_count;
    *runtime = build_runtime_streaming(result.compacted_session, telegram);
    Some(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentBlock, ConversationMessage, MessageRole};
    use std::sync::Mutex;

    /// `std::env::set_var` is process-global and races between parallel
    /// tests. Only the env-var-touching test takes this lock; the rest use
    /// explicit paths via `save_session_at` / `try_load_session_at`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Build a unique temp file path for this test invocation. Caller is
    /// responsible for cleaning it up.
    fn temp_session_file(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("claudette-test-sessions");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!(
            "{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn default_session_path_honors_env_var() {
        let _guard = ENV_LOCK.lock().unwrap();
        let path = temp_session_file("env-var");
        let prev = std::env::var("CLAUDETTE_SESSION").ok();
        std::env::set_var("CLAUDETTE_SESSION", &path);

        let resolved = default_session_path();
        assert_eq!(resolved, path);

        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_SESSION", v),
            None => std::env::remove_var("CLAUDETTE_SESSION"),
        }
    }

    #[test]
    fn save_then_load_round_trip() {
        let path = temp_session_file("round-trip");
        let mut session = Session::default();
        session.messages.push(ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "remember this".to_string(),
            }],
            usage: None,
        });

        save_session_at(&session, &path).expect("save should succeed");
        let loaded = try_load_session_at(&path)
            .expect("load should not error")
            .expect("session should be present");

        assert_eq!(loaded.messages.len(), 1);
        if let ContentBlock::Text { text } = &loaded.messages[0].blocks[0] {
            assert_eq!(text, "remember this");
        } else {
            panic!("expected text block");
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn try_load_returns_none_when_missing() {
        let path = temp_session_file("missing");
        let _ = std::fs::remove_file(&path); // belt-and-braces
        let result = try_load_session_at(&path).expect("missing file should not error");
        assert!(result.is_none());
    }

    #[test]
    fn compact_threshold_default_when_env_var_unset() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CLAUDETTE_COMPACT_THRESHOLD").ok();
        std::env::remove_var("CLAUDETTE_COMPACT_THRESHOLD");

        assert_eq!(compact_threshold(), DEFAULT_COMPACT_THRESHOLD);

        if let Some(v) = prev {
            std::env::set_var("CLAUDETTE_COMPACT_THRESHOLD", v);
        }
    }

    #[test]
    fn compact_threshold_honors_env_var() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CLAUDETTE_COMPACT_THRESHOLD").ok();
        std::env::set_var("CLAUDETTE_COMPACT_THRESHOLD", "12345");

        assert_eq!(compact_threshold(), 12345);

        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_COMPACT_THRESHOLD", v),
            None => std::env::remove_var("CLAUDETTE_COMPACT_THRESHOLD"),
        }
    }

    #[test]
    fn compact_threshold_falls_back_on_garbage() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CLAUDETTE_COMPACT_THRESHOLD").ok();
        std::env::set_var("CLAUDETTE_COMPACT_THRESHOLD", "not-a-number");

        assert_eq!(compact_threshold(), DEFAULT_COMPACT_THRESHOLD);

        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_COMPACT_THRESHOLD", v),
            None => std::env::remove_var("CLAUDETTE_COMPACT_THRESHOLD"),
        }
    }

    #[test]
    fn maybe_compact_session_no_op_when_under_threshold() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CLAUDETTE_COMPACT_THRESHOLD").ok();
        std::env::set_var("CLAUDETTE_COMPACT_THRESHOLD", "1000000");

        // Build a runtime around a tiny session — well under 1M tokens.
        let mut session = Session::default();
        session.messages.push(ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "tiny".to_string(),
            }],
            usage: None,
        });
        let messages_before = session.messages.len();
        let mut runtime = build_runtime(session);

        let result = maybe_compact_session(&mut runtime, false);
        assert!(
            result.is_none(),
            "should not compact when session is under threshold"
        );
        assert_eq!(runtime.session().messages.len(), messages_before);

        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_COMPACT_THRESHOLD", v),
            None => std::env::remove_var("CLAUDETTE_COMPACT_THRESHOLD"),
        }
    }

    #[test]
    fn maybe_compact_session_fires_when_over_threshold() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CLAUDETTE_COMPACT_THRESHOLD").ok();
        // Threshold of 10 tokens — every realistic session crosses this.
        std::env::set_var("CLAUDETTE_COMPACT_THRESHOLD", "10");

        // Build a session with enough messages to hit the
        // CompactionConfig::preserve_recent_messages = 4 floor; we need
        // strictly more than 4 messages or compact_session is a no-op.
        let mut session = Session::default();
        for i in 0..8 {
            session.messages.push(ConversationMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text {
                    text: format!("turn {i} content padded long enough to register"),
                }],
                usage: None,
            });
        }
        let mut runtime = build_runtime(session);
        let messages_before = runtime.session().messages.len();

        let result = maybe_compact_session(&mut runtime, false);
        let removed = result.expect("expected compaction to fire");
        assert!(removed > 0, "should remove at least one message");
        // After compaction the runtime is rebuilt around the compacted
        // session. The replacement carries the System summary message
        // plus the preserved tail, so total < before.
        assert!(runtime.session().messages.len() < messages_before);

        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_COMPACT_THRESHOLD", v),
            None => std::env::remove_var("CLAUDETTE_COMPACT_THRESHOLD"),
        }
    }

    #[test]
    fn save_creates_parent_directory() {
        let path = temp_session_file("nested")
            .parent()
            .unwrap()
            .join("nested-subdir")
            .join("session.json");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());

        save_session_at(&Session::default(), &path).expect("save should create parents");
        assert!(path.exists());

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
