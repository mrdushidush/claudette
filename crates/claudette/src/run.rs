//! Top-level entry points — single-shot and REPL.

use std::path::PathBuf;

use crate::{
    compact_session, estimate_session_tokens, CompactionConfig, ConversationRuntime,
    PermissionPrompter, Session, TurnSummary,
};
use anyhow::{Context, Result};

use crate::api::OllamaApiClient;
use crate::executor::AgentToolExecutor;
use crate::model_config;
use crate::theme;

// Brain default now lives in `model_config::ModelConfig::from_preset`. The
// Auto preset (qwen3.5:4b brain + qwen3.5:9b fallback, shipped Sprint 14)
// replaces the `DEFAULT_MODEL = "qwen3:8b"` constant that used to live
// here — callers should use `current_model()` or `model_config::active()`.

// Compaction & per-turn iteration policy (thresholds, tier selection,
// max-iterations). Extracted to `run/compaction_policy.rs` (Wave C1) and
// re-exported so external `run::X` paths keep resolving.
mod compaction_policy;
pub use compaction_policy::{
    compact_threshold, max_iterations, soft_compact_threshold, DEFAULT_COMPACT_THRESHOLD,
    DEFAULT_MAX_ITERATIONS,
};
pub(crate) use compaction_policy::{pick_compact_plan, CompactionOutcome};

// Cross-session recall hooks (post-turn indexing + startup embed probe).
// Extracted to `run/recall_index.rs` (Wave C2); re-exported so external
// `run::X` callers keep resolving.
mod recall_index;
pub use recall_index::reprobe_recall;
pub(crate) use recall_index::{
    index_turn_for_recall, probe_recall_at_startup, recall_disabled, recall_index_allowed,
};

// Interactive CLI permission prompter + status-line helpers.
// Extracted to `run/cli_prompter.rs` (Wave C3); re-exported so external
// `run::X` callers keep resolving.
mod cli_prompter;
pub use cli_prompter::CliPrompter;
pub(crate) use cli_prompter::{format_ctx_gauge, EMPTY_RESPONSE_NUDGE};

// Runtime assembly: agent + forge ConversationRuntime builders + permission
// policy. Extracted to `run/runtime_build.rs` (Wave C4); re-exported so external
// `run::X` callers (brain_selector, executor, tui_worker) and run.rs's own forge
// mission code keep resolving.
mod runtime_build;
pub(crate) use runtime_build::{
    build_forge_role_runtime, build_forge_runtime, build_permission_policy, build_runtime,
    build_runtime_streaming, build_runtime_with_brain,
};

// Interactive agent REPL loop. Extracted to `run/repl.rs` (Wave C6);
// re-exported so `lib.rs` / `main.rs` keep resolving `run::run_agent_repl`.
mod repl;
pub use repl::run_agent_repl;

// Forge mission orchestration (Coder→Verifier fix loop, planner/verifier).
// Extracted to `run/forge_run.rs` (Wave C5); re-exported so `lib.rs` / `main.rs`
// keep resolving `run::run_forge_mission`. `forge_auto_approve_enabled` is also
// re-exported because the C4 runtime builders consult it.
mod forge_run;
pub(crate) use forge_run::forge_auto_approve_enabled;
pub use forge_run::run_forge_mission;

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
    crate::env_config::home_dir()
        .join(".claudette")
        .join("sessions")
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
pub fn run_agent(user_input: &str, opts: SessionOptions) -> Result<TurnSummary> {
    let session = if opts.resume {
        try_load_session()?.ok_or_else(|| {
            anyhow::anyhow!("no saved session at {}", default_session_path().display())
        })?
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
    let summary =
        crate::brain_selector::run_turn_with_fallback(&mut runtime, user_input, &mut no_prompter)
            .map_err(|e| anyhow::anyhow!("secretary turn failed: {e}"))?;

    // Same session-size trigger as the REPL — fire after the turn so the
    // session we autosave (when --resume is set) is already trimmed.
    if let Some(outcome) = maybe_compact_session(&mut runtime, false) {
        eprintln!(
            "[auto-compacted {} older message(s) — {} tier @ {} tokens]",
            outcome.removed,
            outcome.tier.name(),
            outcome.threshold,
        );
    }

    if opts.autosave {
        save_session(runtime.session())?;
    }
    Ok(summary)
}

/// True for the canonical truthy env values (case-insensitive). Shared by the
/// forge gate knobs. Thin shim over the crate-wide [`crate::env_config::is_enabled`]
/// so run/forge call sites keep a short local name.
fn env_flag_enabled(name: &str) -> bool {
    crate::env_config::is_enabled(name)
}

/// Opt-in: run the *interactive secretary* (REPL / one-shot / TUI) in
/// auto-approve mode — `PermissionMode::Allow`, so DangerFullAccess tools
/// (edit_file, apply_diff, bash, git writes) run without a `[y/N]` prompt.
/// This is the daily-driver "accept-edits / just-do-it" knob and the only way
/// one-shot (`claudette "fix the bug"`) can apply an edit, since one-shot has
/// no prompter to answer. OFF by default — the normal flow still prompts.
/// Enable only when you trust the prompt + workspace (`CLAUDETTE_AUTO_APPROVE=1`).
pub(crate) fn agent_auto_approve_enabled() -> bool {
    env_flag_enabled("CLAUDETTE_AUTO_APPROVE")
}

/// Concatenate the assistant text blocks from a `TurnSummary`. Forge
/// Planner/Verifier turns produce a single assistant message with text
/// content; this helper centralises the unwrapping.
pub(crate) fn extract_assistant_text(summary: &TurnSummary) -> String {
    use crate::ContentBlock;
    let mut out = String::new();
    for msg in &summary.assistant_messages {
        for block in &msg.blocks {
            if let ContentBlock::Text { text } = block {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
    }
    out
}

/// Render a one-line startup banner for the outcome of
/// `try_rehydrate_active_mission()`. Quiet on `None` (no pointer file) so
/// fresh sessions stay clean; loud on `Rehydrated` and `Cleared` so the
/// user always knows when they've just inherited a non-empty mission slot
/// or when a stale pointer was wiped.
pub(crate) fn print_rehydrate_outcome(outcome: crate::missions::RehydrateOutcome) {
    use crate::missions::RehydrateOutcome;
    match outcome {
        RehydrateOutcome::None => {}
        RehydrateOutcome::Rehydrated(m) => {
            eprintln!(
                "{} {} {} {}",
                theme::SAVE,
                theme::ok("resumed mission"),
                theme::ok(&m.slug),
                theme::dim(&format!("({})", m.path.display())),
            );
            eprintln!(
                "  {} {}",
                theme::dim("∘"),
                theme::dim("clear it with /mission_exit (or mission_state action=exit) if you didn't intend this"),
            );
        }
        RehydrateOutcome::Cleared { reason, path } => {
            eprintln!(
                "{} {}",
                theme::warn(theme::WARN_GLYPH),
                theme::warn(&format!(
                    "cleared stale active-mission pointer at {} — {reason}",
                    path.display()
                ))
            );
        }
    }
}

/// Run a turn with auto-retry on empty response. When the model returns
/// "no content" (common when qwen3:8b wants a tool not in the current
/// schema), this injects a nudge message and retries once. Both the REPL
/// and Telegram mode use this.
pub(crate) fn run_turn_with_retry(
    runtime: &mut ConversationRuntime<OllamaApiClient, AgentToolExecutor>,
    input: &str,
    prompter: Option<&mut dyn PermissionPrompter>,
) -> Result<TurnSummary, String> {
    // Stash any file paths from the raw user input — covers Telegram (its
    // single call site) plus any future caller of run_turn_with_retry.
    crate::tools::set_current_turn_paths(crate::tools::extract_user_prompt_paths(input));

    // Drain any deferred coder lease before the brain runs so the coder
    // doesn't contend with the brain for VRAM. The coalesced-swap design
    // in `codet::CoderSwapGuard` keeps the coder warm for a short window
    // after the last guard drops; this call collapses that window for any
    // turn that needs the brain back synchronously.
    crate::codet::drain_pending_coder_lease();

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

/// Check whether the runtime's session is over the compaction threshold
/// and, if so, compact it in place. Returns `Some(removed)` if compaction
/// happened, `None` otherwise.
///
/// Called from [`run_agent_repl`] after every model turn. The metric
/// is `crate::estimate_session_tokens` (a char-count heuristic that
/// scales with the actual session size), not the cumulative input-token
/// counter that grows monotonically.
///
/// **Tiered behaviour (P3, 2026-05-04 queue):**
/// - At/above [`compact_threshold`] (1M default): hard compact, preserves
///   `HARD_COMPACT_PRESERVE` recent messages.
/// - At/above [`soft_compact_threshold`] but below the hard threshold:
///   soft compact, preserves `SOFT_COMPACT_PRESERVE` recent messages.
///   Only fires when the user opts in via `CLAUDETTE_SOFT_COMPACT_THRESHOLD`.
/// - Below both: no-op.
pub(crate) fn maybe_compact_session(
    runtime: &mut ConversationRuntime<OllamaApiClient, AgentToolExecutor>,
    telegram: bool,
) -> Option<CompactionOutcome> {
    let estimated = estimate_session_tokens(runtime.session());
    let hard = compact_threshold();
    let soft = soft_compact_threshold();
    let (tier, preserve, threshold) = pick_compact_plan(estimated, hard, soft)?;
    let result = compact_session(
        runtime.session(),
        CompactionConfig {
            preserve_recent_messages: preserve,
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
    Some(CompactionOutcome {
        removed,
        tier,
        threshold,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // `CompactionTier` is not re-exported (used only here, in the
    // maybe_compact_session test); reach it through the submodule directly.
    use super::compaction_policy::CompactionTier;
    use crate::{ContentBlock, ConversationMessage, MessageRole, PermissionMode};
    // The staying forge_model_map test uses `forge::types::ModelMap` (not a
    // moved fn); the forge prod code that imported `forge` moved out.
    use crate::forge;
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
                .map_or(0, |d| d.as_nanos())
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
        // CompactionConfig::preserve_recent_messages = 12 floor; we need
        // strictly more than 12 messages or compact_session is a no-op.
        let mut session = Session::default();
        for i in 0..16 {
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
        let outcome = result.expect("expected compaction to fire");
        assert!(outcome.removed > 0, "should remove at least one message");
        assert_eq!(
            outcome.tier,
            CompactionTier::Hard,
            "10-token hard threshold should fire the hard tier"
        );
        assert_eq!(outcome.threshold, 10);
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

    /// Regression test: every tool name advertised in `agent_tools_json`
    /// must have a matching entry in `build_permission_policy()` so the
    /// unknown-tool short-circuit (added v0.2.3) does not swallow real tools
    /// before they reach the dispatcher.
    ///
    /// This is the bug class that hit v0.3.0–v0.3.1: the v0.2.0 Life Agent
    /// groups (calendar / gmail / schedule) were never registered in the
    /// permission policy, so every call returned `{"error":"unknown tool"}`
    /// and the morning briefing hallucinated to cover. Fixed in v0.3.1, but
    /// the only thing keeping it fixed without this test is hand-discipline.
    /// (Companion to `every_advertised_tool_is_classified` in
    /// `tool_groups.rs`, which catches the analogous schema↔registry gap.)
    #[test]
    fn every_advertised_tool_has_permission_requirement() {
        let policy = build_permission_policy();
        let full = crate::tools::agent_tools_json();
        let arr = full.as_array().cloned().unwrap_or_default();

        let mut missing: Vec<String> = Vec::new();
        for tool in arr {
            let Some(name) = tool
                .pointer("/function/name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            if !policy.is_known(&name) {
                missing.push(name);
            }
        }

        assert!(
            missing.is_empty(),
            "tool(s) advertised but not registered in build_permission_policy() — \
             will be swallowed by the unknown-tool short-circuit and never reach \
             the dispatcher: {missing:?}. Add a `.with_tool_requirement(name, ...)` \
             entry."
        );
    }

    /// Regression test: tools that internally invoke other DangerFullAccess
    /// primitives (or take their actions directly) must themselves be
    /// DangerFullAccess so the [y/N] confirmation reaches the user. The
    /// companion test above is name-coverage; this one is tier-correctness.
    /// Without it, downgrading a high-blast-radius tool silently lets a 4b
    /// brain take an irreversible cross-org action.
    #[test]
    fn high_blast_radius_tools_require_danger_tier() {
        let policy = build_permission_policy();
        // (tool_name, why) — each must be DangerFullAccess. Add new entries
        // here whenever a tool gains internal calls into git_push, edit_file,
        // bash, gh_create_pr, or any other already-DangerFullAccess primitive.
        let cases: &[(&str, &str)] =
            &[("mission_submit", "calls git_push + gh_create_pr internally")];
        for (name, why) in cases {
            let actual = policy.required_mode_for(name);
            assert_eq!(
                actual,
                PermissionMode::DangerFullAccess,
                "{name} must be DangerFullAccess: {why}; got {actual:?}"
            );
        }
    }

    // ─── Forge v0b: persona overlay + models.toml role-routing ────────

    /// `forge_role_model` is best-effort: a missing `~/.claudettes-forge/
    /// models.toml` returns the built-in default (qwen3-coder:30b for Coder,
    /// qwen3.5:14b for Planner/Verifier), env overrides win when set. The
    /// smoke test asserts each forge role returns *some* non-empty string
    /// in a clean environment (defaults from
    /// `forge::models_toml::default_model_map`).
    #[test]
    fn forge_model_map_has_a_default_for_each_role() {
        // The built-in ModelMap always resolves a non-empty model per role.
        // (forge_role_model itself returns None when forge models aren't
        // explicitly configured, so each role falls back to the active brain —
        // roast RC-G #4 — which is environment-dependent and not asserted here.)
        let map = forge::types::ModelMap::load().expect("default model map loads");
        for role in [
            forge::types::Role::Coder,
            forge::types::Role::Planner,
            forge::types::Role::Verifier,
        ] {
            let (_, model) = map
                .resolve(role)
                .unwrap_or_else(|| panic!("forge default model for {role:?}"));
            assert!(!model.is_empty(), "{role:?} model name must be non-empty");
        }
    }
}
