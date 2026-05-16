//! Top-level entry points — single-shot and REPL.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{
    compact_session, estimate_session_tokens, CompactionConfig, ConversationRuntime,
    PermissionMode, PermissionPolicy, PermissionPromptDecision, PermissionPrompter,
    PermissionRequest, Session, TurnSummary,
};
use anyhow::{Context, Result};

use crate::api::{stdout_text_callback, telegram_text_callback, OllamaApiClient};
use crate::commands::{dispatch_slash_command, parse_slash_command, ReplState, SlashOutcome};
use crate::executor::SecretaryToolExecutor;
use crate::forge;
use crate::memory::try_load_memory;
use crate::model_config;
use crate::prompt::{
    forge_planner_system_prompt, forge_system_prompt, forge_verifier_system_prompt,
    secretary_system_prompt_with_memory,
};
use crate::theme;
use crate::tool_groups::{ToolGroup, ToolRegistry};

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
/// though the session itself was small. A real transcript on 2026-04-09
/// caught this — six consecutive turns each removing 5 messages.
///
/// The fix: bypass the runtime's trigger (set its threshold to
/// `u32::MAX` in [`build_runtime`]) and roll our own in
/// [`maybe_compact_session`], using `estimate_session_tokens(session)` —
/// a metric that's actually bounded by the current session size and
/// drops back below the threshold after a successful compact.
///
/// Default `1_000_000` makes auto-compact effectively a no-op for typical
/// local-brain setups (16K–128K context). The gate stays in place so a
/// pathologically long session still trips it, but day-to-day work won't
/// see compaction noise. Users on tight context windows who *want* the old
/// safety net can set `CLAUDETTE_COMPACT_THRESHOLD=12000` (or whatever
/// fraction of their `num_ctx` they prefer).
pub const DEFAULT_COMPACT_THRESHOLD: usize = 1_000_000;

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

/// Soft (early) compaction threshold. Returns `None` when unset — the
/// default — preserving the existing one-tier behaviour where the only
/// gate is `compact_threshold()` at 1M.
///
/// When the env var is set to a positive number AND the session has grown
/// above it but is still under the hard threshold, [`maybe_compact_session`]
/// runs a *soft* compact: same machinery as the hard path but preserves
/// 12 recent messages instead of 4, so summarisation kicks in earlier with
/// less context loss. Useful for long real-world sessions on 35B+ brains
/// where one transcript was paying ~573K input tokens per turn.
///
/// Tracks P3 in the 2026-05-04 optimization queue.
#[must_use]
pub fn soft_compact_threshold() -> Option<usize> {
    std::env::var("CLAUDETTE_SOFT_COMPACT_THRESHOLD")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
}

/// Recent-message preservation count for the hard (1M default) compaction
/// path. Aggressive — keeps just enough context for the model to continue
/// the immediate conversation.
const HARD_COMPACT_PRESERVE: usize = 4;

/// Recent-message preservation count for the soft (env-var-gated) path.
/// Three times the hard count: the user opted into early compaction, so
/// trade summary aggressiveness for continuity.
const SOFT_COMPACT_PRESERVE: usize = 12;

/// Which compaction tier fired in a given pass. Carried back out of
/// [`maybe_compact_session`] so the callsite's log message names the right
/// threshold and tier — without this, soft-tier compactions were reported
/// as "session was over <hard>-token threshold", which made test 8 of the
/// 2026-05-12 sprint impossible to interpret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompactionTier {
    /// Hit [`compact_threshold`] (the default 1M hard ceiling).
    Hard,
    /// Hit [`soft_compact_threshold`] but stayed below the hard ceiling.
    Soft,
}

impl CompactionTier {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Hard => "hard",
            Self::Soft => "soft",
        }
    }
}

/// What happened in one auto-compaction pass: how many messages were
/// summarised, which tier fired, and the threshold token-count that gated
/// it. Used by the REPL/single-shot log lines to surface tier-aware status
/// instead of always naming the hard threshold.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CompactionOutcome {
    pub removed: usize,
    pub tier: CompactionTier,
    pub threshold: usize,
}

/// Default REPL/TUI max iterations per turn — how many (model → tool → result)
/// cycles a single user prompt is allowed to drive before the runtime aborts
/// with "conversation loop exceeded the maximum number of iterations".
///
/// `40` is generous: it accommodates legitimate long tool chains (multi-step
/// research, build + test + grep + fix) while still capping pathological
/// spirals from small brains. Override via `CLAUDETTE_MAX_ITERATIONS`.
pub const DEFAULT_MAX_ITERATIONS: usize = 40;

/// Resolve the per-turn max-iteration ceiling. Honors
/// `CLAUDETTE_MAX_ITERATIONS`; falls back to [`DEFAULT_MAX_ITERATIONS`].
#[must_use]
pub fn max_iterations() -> usize {
    std::env::var("CLAUDETTE_MAX_ITERATIONS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_ITERATIONS)
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

/// Default cap on Coder→Verifier fix-loop rounds in v0c forge-mode.
/// Round 0 is the initial Coder pass; up to this many additional rounds
/// run if the Verifier rejects. Empirically two rounds is the sweet spot
/// — a local 8b coder model that didn't get it after two passes usually
/// won't on a third, and burning more rounds runs the user's context
/// budget into the ground.
const DEFAULT_MAX_FIX_ROUNDS: u32 = 2;

/// Hard upper bound on fix-loop rounds, even if `CLAUDETTE_MAX_FIX_ROUNDS`
/// is set higher. Past ~10 rounds the brain is reliably stuck in a local
/// minimum and the right move is to bail and let the user re-prompt.
const FIX_ROUNDS_HARD_CAP: u32 = 10;

/// Resolve the active fix-loop round cap. Honors `CLAUDETTE_MAX_FIX_ROUNDS`
/// (parsed as u32, clamped to `FIX_ROUNDS_HARD_CAP`) and falls back to
/// `DEFAULT_MAX_FIX_ROUNDS` on missing or unparseable input. Read on every
/// call — the forge loop fires a few times per mission so the cost is
/// negligible, and re-reading makes the knob hot-pluggable across sessions.
fn max_fix_rounds() -> u32 {
    match std::env::var("CLAUDETTE_MAX_FIX_ROUNDS")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    {
        Some(n) => n.min(FIX_ROUNDS_HARD_CAP),
        None => DEFAULT_MAX_FIX_ROUNDS,
    }
}

/// One Verifier judgement. `pass` is the authoritative gate (a Verifier
/// can score 8 and still mark fail if it spotted a security bug); `score`
/// is advisory and shown to the user but not compared against a threshold
/// in [`run_forge_mission`].
#[derive(Debug, Clone)]
pub(crate) struct VerifierResult {
    pub score: u8,
    pub pass: bool,
    pub feedback: String,
}

/// Run a forge-mode mission inside the active brownfield mission and
/// return the cumulative summary. Errors immediately if no mission is
/// active — forge-mode without a mission has no tree to edit and no PR
/// target.
///
/// **v0c pipeline (current):**
/// 1. **Planner** — tool-less brain turn that decomposes the request into
///    a 3-5 step numbered plan. Output is prepended to the Coder's input.
/// 2. **Coder** — forge runtime with files/search/git/advanced/github
///    pre-enabled and `should_submit=false`. Commits the change but does
///    NOT call `mission_submit` so the Verifier can review first.
/// 3. **Verifier** — tool-less brain turn that scores the `git diff HEAD`
///    against the original request. Returns `{score, pass, feedback}`.
///    On parse failure, treated as pass (advisory mode).
/// 4. **Fix-loop** — if Verifier `pass=false` and `round < max_fix_rounds()`,
///    re-runs Coder with the Verifier's feedback prepended. Default two
///    rounds; override with `CLAUDETTE_MAX_FIX_ROUNDS` (clamped to 10).
/// 5. **Submitter** — final Coder turn with `should_submit=true` that only
///    calls `mission_submit` (PR opens here, not earlier).
///
/// `opts.resume` and `opts.autosave` behave as in [`run_secretary`]: forge
/// turns are part of the same session log when `--resume` was passed; a
/// one-off forge invocation without `--resume` doesn't clobber the REPL
/// session.
pub fn run_forge_mission(user_input: &str, opts: SessionOptions) -> Result<TurnSummary> {
    // ── Auto-bootstrap ──────────────────────────────────────────────────
    // If no mission is active, try to bootstrap an ephemeral one rooted at
    // the cwd's git toplevel (under $HOME or CLAUDETTE_WORKSPACE). Lets
    // `claudette --forge "<prompt>"` Just Work inside the repo the user is
    // already cd'd into, without an explicit `/brownfield owner/repo`
    // clone first. The ephemeral mission is never persisted and is auto-
    // cleared on any error in this fn so a failed forge doesn't leak a
    // mission slot the user didn't ask for.
    let mission = match crate::missions::active_mission() {
        Some(m) => m,
        None => match crate::missions::try_bootstrap_local_mission() {
            Ok(m) => {
                eprintln!(
                    "{} {} {}",
                    theme::BOLT,
                    theme::accent("forge: ephemeral mission"),
                    theme::dim(&m.path.display().to_string())
                );
                crate::missions::set_active(m.clone())
                    .map_err(|e| anyhow::anyhow!("set_active for ephemeral mission: {e}"))?;
                m
            }
            Err(why) => {
                return Err(anyhow::anyhow!(
                    "forge-mode requires an active brownfield mission, and could not \
                     auto-bootstrap one from the working directory ({why}). Either \
                     `cd` into a git repo under $HOME / CLAUDETTE_WORKSPACE, or run \
                     `/brownfield <owner/repo>` first to clone a target tree."
                ));
            }
        },
    };

    // Guard for the ephemeral path: any early return from this point on
    // clears the mission slot if and only if WE installed it. User-
    // initiated missions (`/brownfield`, `mission_attach`) are left alone
    // so the user can retry / inspect after a forge failure. Disarmed at
    // the end of the happy path so a successful run also leaves the slot
    // intact (lets subsequent `/forge` invocations in the same REPL keep
    // the same mission without re-bootstrapping).
    let mut cleanup = EphemeralMissionGuard::new(mission.ephemeral);

    // Snapshot HEAD before any forge phase runs so the Verifier can diff
    // against it after the Coder commits. Without this, `git diff HEAD`
    // inside the Verifier loop returns empty (HEAD already points at the
    // Coder's new commit) and the Verifier sees nothing to grade.
    let base_sha = capture_base_sha(&mission.path);

    let session = if opts.resume {
        try_load_session()?.ok_or_else(|| {
            anyhow::anyhow!("no saved session at {}", default_session_path().display())
        })?
    } else {
        Session::default()
    };

    let mut prompter = CliPrompter;
    let mut prompter_opt: Option<&mut dyn PermissionPrompter> = Some(&mut prompter);

    // ── Phase 1: Planner ────────────────────────────────────────────
    eprintln!("{} {}", theme::BOLT, theme::accent("forge: planner"));
    let plan = run_planner(session.clone(), &mission, user_input, &mut prompter_opt)
        .unwrap_or_else(|e| {
            eprintln!(
                "  {} {}",
                theme::dim("∘"),
                theme::dim(&format!("planner skipped: {e}"))
            );
            String::new()
        });
    if !plan.trim().is_empty() {
        eprintln!("{}", theme::dim(plan.trim()));
    }

    let augmented_input = if plan.trim().is_empty() {
        user_input.to_string()
    } else {
        format!("Plan:\n{}\n\nTask: {user_input}", plan.trim())
    };

    // ── Phase 2 + 3 + 4: Coder ↔ Verifier fix-loop ───────────────────
    let mut feedback: Option<String> = None;
    let mut round: u32 = 0;
    loop {
        eprintln!(
            "{} {} (round {})",
            theme::BOLT,
            theme::accent("forge: coder"),
            round
        );
        let coder_input = match &feedback {
            None => augmented_input.clone(),
            Some(f) => format!(
                "The Verifier rejected your previous attempt with this feedback:\n{f}\n\n\
                 Revise your work — add additional commits to the same branch as needed. \
                 Do NOT push or call mission_submit yet; the Verifier will review again.\n\n\
                 Original task: {user_input}"
            ),
        };
        let mut coder_runtime = build_forge_runtime(session.clone(), &mission, false);
        crate::tools::set_current_turn_paths(crate::tools::extract_user_prompt_paths(&coder_input));
        let _ = crate::brain_selector::run_turn_with_fallback(
            &mut coder_runtime,
            &coder_input,
            &mut prompter_opt,
        )
        .map_err(|e| anyhow::anyhow!("forge coder turn failed (round {round}): {e}"))?;

        // Verifier
        eprintln!("{} {}", theme::BOLT, theme::accent("forge: verifier"));
        let diff = capture_git_diff(&mission.path, base_sha.as_deref()).unwrap_or_default();
        let verifier = run_verifier(
            session.clone(),
            &mission,
            user_input,
            &diff,
            &mut prompter_opt,
        )
        .unwrap_or_else(|e| {
            eprintln!(
                "  {} {}",
                theme::dim("∘"),
                theme::dim(&format!("verifier skipped: {e}"))
            );
            VerifierResult {
                score: 10,
                pass: true,
                feedback: String::new(),
            }
        });
        let feedback_display: &str = if verifier.feedback.is_empty() {
            "(no feedback)"
        } else {
            verifier.feedback.as_str()
        };
        eprintln!(
            "  {} {}",
            theme::BOLT,
            theme::info(&format!(
                "score={} pass={} {feedback_display}",
                verifier.score, verifier.pass,
            ))
        );

        if verifier.pass {
            break;
        }
        if round >= max_fix_rounds() {
            eprintln!(
                "  {} {}",
                theme::dim("∘"),
                theme::dim(&format!(
                    "verifier still failing after {round} round(s); submitting anyway"
                ))
            );
            break;
        }
        feedback = Some(verifier.feedback);
        round += 1;
    }

    // ── Phase 5: Submitter ──────────────────────────────────────────
    eprintln!("{} {}", theme::BOLT, theme::accent("forge: submit"));
    let mut submit_runtime = build_forge_runtime(session, &mission, true);
    let submit_input =
        "All quality checks passed. Now call mission_submit with a short PR title that \
         summarises the change. Do nothing else.";
    crate::tools::set_current_turn_paths(crate::tools::extract_user_prompt_paths(submit_input));
    let submit_summary = crate::brain_selector::run_turn_with_fallback(
        &mut submit_runtime,
        submit_input,
        &mut prompter_opt,
    )
    .map_err(|e| anyhow::anyhow!("forge submitter turn failed: {e}"))?;

    if let Some(outcome) = maybe_compact_session(&mut submit_runtime, false) {
        eprintln!(
            "[auto-compacted {} older message(s) — {} tier @ {} tokens]",
            outcome.removed,
            outcome.tier.name(),
            outcome.threshold,
        );
    }
    if opts.autosave {
        save_session(submit_runtime.session())?;
    }

    // Report the Submitter's summary as the canonical one — it's the turn
    // that opened the PR. Earlier Coder/Verifier iterations are visible
    // from the user's terminal stream but don't roll into the returned
    // counter; the user sees per-phase progress as it happens.
    cleanup.disarm();
    Ok(submit_summary)
}

/// RAII guard: clears the active mission slot on Drop iff the mission we
/// installed was ephemeral AND `disarm()` was not called. Pairs with the
/// auto-bootstrap path in [`run_forge_mission`] so a mid-pipeline failure
/// can't leave a `/forge`-installed mission active in the REPL.
struct EphemeralMissionGuard {
    armed: bool,
}

impl EphemeralMissionGuard {
    fn new(ephemeral: bool) -> Self {
        Self { armed: ephemeral }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for EphemeralMissionGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = crate::missions::clear_active();
        }
    }
}

/// v0c: capture the diff the Coder produced this mission. When a `base`
/// SHA is provided (captured once at the start of [`run_forge_mission`]
/// before the Coder commits anything), runs `git diff <base>..HEAD` so the
/// Verifier sees the full Coder output even though the Coder has already
/// committed. Falls back to `git diff HEAD` (uncommitted working-tree
/// changes) when no base is available — e.g., fresh repo with no commits
/// yet, or `git rev-parse` failed at mission start. Returns `None` on any
/// `git` failure so a transient error can't deadlock the pipeline.
fn capture_git_diff(mission_path: &std::path::Path, base: Option<&str>) -> Option<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(mission_path);
    match base {
        Some(b) => cmd.args(["diff", &format!("{b}..HEAD")]),
        None => cmd.args(["diff", "HEAD"]),
    };
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Capture the mission's HEAD SHA at the moment forge begins, before the
/// Planner or Coder run. Used by [`capture_git_diff`] to produce a
/// `base..HEAD` diff that survives the Coder committing mid-pipeline
/// (otherwise `git diff HEAD` returns empty and the Verifier sees nothing
/// to grade). Returns `None` on fresh repos with no commits yet or any
/// `git` failure; callers fall back to the working-tree diff.
fn capture_base_sha(mission_path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(mission_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

/// v0c: run a tool-less Planner turn. The Planner sees the user's request,
/// emits a 3-5 step numbered plan, then exits. Output is the plan as plain
/// text; an empty/whitespace-only response is returned as `Ok("")`
/// (caller treats that as "no plan, skip prepending").
///
/// Uses the `Planner` role model from `~/.claudettes-forge/models.toml` if
/// configured; otherwise falls back to claudette's active brain.
fn run_planner(
    session: Session,
    mission: &crate::missions::Mission,
    user_input: &str,
    prompter: &mut Option<&mut dyn PermissionPrompter>,
) -> Result<String> {
    let mut runtime = build_forge_role_runtime(
        session,
        mission,
        forge::types::Role::Planner,
        forge_planner_system_prompt(&mission.path.to_string_lossy()),
        &[], // no tool groups
    );
    let summary = crate::brain_selector::run_turn_with_fallback(&mut runtime, user_input, prompter)
        .map_err(|e| anyhow::anyhow!("planner turn failed: {e}"))?;
    Ok(extract_assistant_text(&summary))
}

/// v0c: run a tool-less Verifier turn. The Verifier sees the original
/// request plus the captured `git diff` and returns a JSON object that's
/// parsed into [`VerifierResult`]. Unparseable responses fall through to a
/// permissive default (pass=true, score=10) so a poorly-behaved Verifier
/// model can't deadlock a working Coder.
fn run_verifier(
    session: Session,
    mission: &crate::missions::Mission,
    user_input: &str,
    diff: &str,
    prompter: &mut Option<&mut dyn PermissionPrompter>,
) -> Result<VerifierResult> {
    let mut runtime = build_forge_role_runtime(
        session,
        mission,
        forge::types::Role::Verifier,
        forge_verifier_system_prompt(&mission.path.to_string_lossy()),
        &[],
    );
    let payload = format!(
        "Original request: {user_input}\n\n--- git diff HEAD ---\n{diff}\n--- end diff ---"
    );
    let summary = crate::brain_selector::run_turn_with_fallback(&mut runtime, &payload, prompter)
        .map_err(|e| anyhow::anyhow!("verifier turn failed: {e}"))?;
    let text = extract_assistant_text(&summary);
    Ok(parse_verifier_response(&text))
}

/// Concatenate the assistant text blocks from a `TurnSummary`. Forge
/// Planner/Verifier turns produce a single assistant message with text
/// content; this helper centralises the unwrapping.
fn extract_assistant_text(summary: &TurnSummary) -> String {
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

/// Parse a Verifier JSON response. Resilient to (a) the model wrapping the
/// JSON in ```code fences, (b) trailing prose after the closing brace, and
/// (c) malformed JSON — in cases (b) and (c) we fall through to a
/// permissive pass=true default rather than blocking the pipeline.
fn parse_verifier_response(text: &str) -> VerifierResult {
    let default = VerifierResult {
        score: 10,
        pass: true,
        feedback: String::new(),
    };
    let trimmed = text.trim();
    // Strip ```json … ``` fences if present.
    let stripped = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map_or(trimmed, |s| s.trim_start())
        .strip_suffix("```")
        .map_or(trimmed, |s| s.trim_end());
    // Match the JSON object — find the first `{` and the last `}`.
    let Some(start) = stripped.find('{') else {
        return default;
    };
    let Some(end) = stripped.rfind('}') else {
        return default;
    };
    if end <= start {
        return default;
    }
    let json_slice = &stripped[start..=end];
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json_slice) else {
        return default;
    };
    let score = v
        .get("score")
        .and_then(serde_json::Value::as_u64)
        .map_or(10, |n| n.clamp(0, 10) as u8);
    let pass = v
        .get("pass")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let feedback = v
        .get("feedback")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    VerifierResult {
        score,
        pass,
        feedback,
    }
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
        theme::dim(&format!("session: {}", default_session_path().display()))
    );

    // Pre-flight the recall embedder so a missing embed model (the typical
    // LM Studio first-run state) surfaces a clean warn line here, not as
    // per-turn noise after the user starts asking questions. Honors
    // CLAUDETTE_RECALL_DISABLE — opting out skips the probe too.
    probe_recall_at_startup();

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

        match turn_result {
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
                        summary.iterations, summary.usage.input_tokens, summary.usage.output_tokens,
                    ))
                );

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
    // One shared ToolRegistry is the single source of truth for the
    // `tools` field on every request. The API client reads from it (via
    // ToolsProvider::Dynamic) and the executor mutates it when the model
    // calls `enable_tools`. Both halves hold a clone of the Arc so the
    // mutations are immediately visible on the next chat turn.
    //
    // No mode (REPL, single-shot, Telegram) pre-enables groups any more.
    // Pre-rewrite, Telegram auto-enabled five groups so the model could
    // call tools without the enable_tools → tool two-step. The cost
    // (~2,500 tokens of schema on every turn, ~15% of a 16K window) was
    // dominating one-word interactions like "hey". Now everything goes
    // through enable_tools; the brain pays one extra round-trip for the
    // first tool call in a session and saves ~2,300 tokens per turn.
    let reg = ToolRegistry::new();
    let registry = Arc::new(Mutex::new(reg));

    let mut api_client = OllamaApiClient::with_registry(brain.model.clone(), registry.clone())
        .with_context(brain.num_ctx)
        .with_max_predict(brain.num_predict);
    if streaming {
        let cb = if telegram {
            telegram_text_callback()
        } else {
            stdout_text_callback()
        };
        api_client = api_client.with_text_callback(cb);
    }
    // Clone the registry handle for the unknown-tool hinter before the
    // executor consumes it. The hinter maps a confabulated *group* name
    // (e.g. `facts`, `markets`) to that group's actual tools so the brain
    // gets a useful "did you mean?" list instead of an empty array.
    let hinter_registry = Arc::clone(&registry);
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
    // single-shot search/grep/git chains. The shared default (currently 40)
    // and the `CLAUDETTE_MAX_ITERATIONS` env-var knob live in `max_iterations`.
    .with_max_iterations(max_iterations())
    .with_auto_compaction_input_tokens_threshold(u32::MAX)
    .with_unknown_tool_hinter(move |name: &str| {
        ToolGroup::parse(name).map_or_else(Vec::new, |group| {
            // Poisoned-lock recovery: another thread held the lock and
            // panicked. Continue with the inner state — the hinter is a
            // best-effort suggestion, not a correctness path.
            let reg = match hinter_registry.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            reg.group_tool_names(group)
        })
    })
}

/// Forge-mode runtime: same plumbing as [`build_runtime_with_brain`] but with
/// a forge-specific system prompt and the tool groups the brain needs
/// pre-enabled (files, search, git, advanced, github) so it doesn't burn
/// turns on `enable_tools`.
///
/// The mission path is threaded into the system prompt so the model has
/// accurate cwd context; the `tools::active_cwd()` routing primitive ensures
/// tools land in the mission tree regardless.
fn build_forge_runtime(
    session: Session,
    mission: &crate::missions::Mission,
    should_submit: bool,
) -> ConversationRuntime<OllamaApiClient, SecretaryToolExecutor> {
    // v0b: persona overlay. Auto-load the bundled `codex7` coder persona for
    // forge mode. The persona's voice + backstory get woven into the system
    // prompt via `forge_system_prompt`. Lookup failures fall back to an
    // unpersonified prompt — persona overlay is best-effort, never required.
    let persona = forge_default_coder_persona();
    let memory = try_load_memory();
    let persona_overlay = persona
        .as_ref()
        .map(|p| (p.voice.as_str(), p.backstory.as_str()));

    let system = forge_system_prompt(
        &mission.path.to_string_lossy(),
        memory.as_deref(),
        persona_overlay,
        should_submit,
    );

    // Coder rounds get the full forge toolset. The Submitter phase (v0c)
    // calls back in with `should_submit=true` and uses the same toolset —
    // restricting it to just github tools is tempting but the brain may
    // still need to look at files (e.g. to compose a PR title from the diff).
    build_forge_role_runtime(
        session,
        mission,
        forge::types::Role::Coder,
        system,
        &[
            ToolGroup::Files,
            ToolGroup::Search,
            ToolGroup::Git,
            ToolGroup::Advanced,
            ToolGroup::Github,
        ],
    )
}

/// v0c: phase-aware forge runtime builder. Used by the Coder runtime (full
/// toolset, `Role::Coder` model from `models.toml`) and by the Planner /
/// Verifier turns (no tool groups, different role-routing). Centralises the
/// `OllamaApiClient` + `SecretaryToolExecutor` + permission policy + hinter
/// setup that every forge phase needs.
fn build_forge_role_runtime(
    session: Session,
    _mission: &crate::missions::Mission,
    role: forge::types::Role,
    system_prompt: Vec<String>,
    tool_groups: &[ToolGroup],
) -> ConversationRuntime<OllamaApiClient, SecretaryToolExecutor> {
    let mut brain = model_config::active().brain;

    // v0b/v0c: models.toml role-routing. If the user has the requested role
    // configured in `~/.claudettes-forge/models.toml` (or env-overridden),
    // use it for this phase. num_ctx/num_predict aren't in models.toml so
    // they carry over from claudette's config.
    if let Some(role_model) = forge_role_model(role) {
        brain.model = role_model;
    }

    let mut reg = ToolRegistry::new();
    for group in tool_groups {
        reg.enable(*group);
    }
    let registry = Arc::new(Mutex::new(reg));

    let api_client = OllamaApiClient::with_registry(brain.model.clone(), registry.clone())
        .with_context(brain.num_ctx)
        .with_max_predict(brain.num_predict)
        .with_text_callback(stdout_text_callback());

    let hinter_registry = Arc::clone(&registry);
    let executor = SecretaryToolExecutor::with_registry(registry);
    let policy = build_permission_policy();

    ConversationRuntime::new(session, api_client, executor, policy, system_prompt)
        .with_max_iterations(max_iterations())
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

/// v0b helper: resolve any forge role's model from `~/.claudettes-forge/
/// models.toml` (or env overrides). Returns `None` on any failure — the
/// caller falls back to claudette's active brain model. Best-effort; a
/// missing/malformed config never blocks forge mode from running.
///
/// v0b only consumed this for `Role::Coder`; v0c extends it to the Planner
/// and Verifier role-routed turns.
fn forge_role_model(role: forge::types::Role) -> Option<String> {
    forge::types::ModelMap::load()
        .ok()?
        .resolve(role)
        .map(|(_, name)| name.to_string())
}

/// v0b helper: load the bundled `codex7` Coder persona, parsed at runtime
/// from content baked in via `include_str!`. Returns `None` if the bundled
/// content fails to parse, which should only happen if the personas file is
/// edited into invalid TOML/markdown — caught by
/// `forge::personas::bundled_personas_all_parse`.
///
/// Bundled rather than disk-resolved because claudette is shipped as a
/// single binary (no `cargo install`-side `personas/` directory).
fn forge_default_coder_persona() -> Option<forge::personas::Persona> {
    const CODEX7: &str = include_str!("../personas/codex7.md");
    forge::personas::parse_persona_content(CODEX7, "bundled:codex7").ok()
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
        // load_workspace_rules: reads ~/.claudette/instructions.md on demand
        // (added in the 2026-05-04 token-trim work to lazy-load what used to
        // auto-attach to the system prompt). Read-only.
        .with_tool_requirement("load_workspace_rules", ReadOnly)
        .with_tool_requirement("glob_search", ReadOnly)
        .with_tool_requirement("grep_search", ReadOnly)
        .with_tool_requirement("git_status", ReadOnly)
        .with_tool_requirement("git_diff", ReadOnly)
        .with_tool_requirement("git_log", ReadOnly)
        .with_tool_requirement("git_branch", ReadOnly)
        // ── Workspace-write (auto-allowed) ──────────────────────────
        .with_tool_requirement("note_create", WorkspaceWrite)
        .with_tool_requirement("note_update", WorkspaceWrite)
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
        .with_tool_requirement("gh_list_repo_issues", ReadOnly)
        .with_tool_requirement("gh_pr_status", ReadOnly)
        .with_tool_requirement("gh_create_issue", WorkspaceWrite)
        .with_tool_requirement("gh_comment_issue", WorkspaceWrite)
        .with_tool_requirement("gh_fork", WorkspaceWrite)
        .with_tool_requirement("gh_create_pr", WorkspaceWrite)
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
        // ── Life Agent (v0.2.0): calendar group ──────────────────────
        // Reads: auto-allowed. Writes/RSVP: WorkspaceWrite. Delete is
        // irreversible from claudette's side, so DangerFullAccess.
        .with_tool_requirement("calendar_list_events", ReadOnly)
        .with_tool_requirement("calendar_create_event", WorkspaceWrite)
        .with_tool_requirement("calendar_update_event", WorkspaceWrite)
        .with_tool_requirement("calendar_respond_to_event", WorkspaceWrite)
        .with_tool_requirement("calendar_delete_event", DangerFullAccess)
        // ── Life Agent: gmail group (gmail.readonly OAuth scope) ─────
        .with_tool_requirement("gmail_list", ReadOnly)
        .with_tool_requirement("gmail_search", ReadOnly)
        .with_tool_requirement("gmail_read", ReadOnly)
        .with_tool_requirement("gmail_list_labels", ReadOnly)
        // ── Life Agent: schedule group ───────────────────────────────
        .with_tool_requirement("schedule_list", ReadOnly)
        .with_tool_requirement("schedule_once", WorkspaceWrite)
        .with_tool_requirement("schedule_recurring", WorkspaceWrite)
        .with_tool_requirement("schedule_cancel", WorkspaceWrite)
        // ── Recall (cross-session memory): pure search ───────────────
        .with_tool_requirement("recall", ReadOnly)
        // ── Dangerous (ALWAYS prompts for [y/N] confirmation) ────��──
        .with_tool_requirement("bash", DangerFullAccess)
        .with_tool_requirement("edit_file", DangerFullAccess)
        .with_tool_requirement("git_add", DangerFullAccess)
        .with_tool_requirement("git_commit", DangerFullAccess)
        .with_tool_requirement("git_push", DangerFullAccess)
        .with_tool_requirement("git_checkout", DangerFullAccess)
        // Brownfield: git_clone writes a fresh tree under the controlled
        // ~/.claudette/missions/ root. Auto-allowed (WorkspaceWrite).
        .with_tool_requirement("git_clone", WorkspaceWrite)
        // ── T2 brownfield: mission_* tools ──────────────────────────────
        // mission_status / mission_list / mission_attach only read state
        // (attach loads a marker + flips an in-memory slot; downstream
        // writes still go through their own gates). mission_exit mutates
        // session state with no FS writes. mission_start clones into
        // ~/.claudette/missions/ (WorkspaceWrite, matching git_clone).
        // mission_submit stages/commits/pushes/opens a PR — must be
        // DangerFullAccess to match its worst action (`git push -u`).
        .with_tool_requirement("mission_start", WorkspaceWrite)
        .with_tool_requirement("mission_status", ReadOnly)
        .with_tool_requirement("mission_list", ReadOnly)
        .with_tool_requirement("mission_attach", ReadOnly)
        .with_tool_requirement("mission_exit", WorkspaceWrite)
        .with_tool_requirement("mission_submit", DangerFullAccess)
}

// ────────────────────────────────────────────────────────────────────────────
// Cross-session recall hooks
// ────────────────────────────────────────────────────────────────────────────

/// Whether the post-turn recall indexing is disabled. Off-by-default
/// privacy/perf escape hatch: `CLAUDETTE_RECALL_DISABLE=1`. Anything else
/// (unset, "0", garbage) leaves indexing enabled.
pub(crate) fn recall_disabled() -> bool {
    matches!(
        std::env::var("CLAUDETTE_RECALL_DISABLE").as_deref(),
        Ok("1")
    )
}

/// Sticky session-scoped flag: once recall indexing fails (e.g. LM Studio
/// has no embed model loaded), every subsequent turn would re-fail with
/// identical noise. After the first failure we set this and silently skip
/// the indexing call until the process restarts. The user gets ONE warning
/// at the first failure with instructions for fixing it (load the model
/// or set `CLAUDETTE_RECALL_DISABLE=1`).
static RECALL_INDEX_BROKEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(crate) fn recall_index_allowed() -> bool {
    !recall_disabled() && !RECALL_INDEX_BROKEN.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn mark_recall_index_broken() {
    RECALL_INDEX_BROKEN.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Clear the sticky `RECALL_INDEX_BROKEN` flag and re-run the startup
/// embed probe. Exposed via the `/recall reprobe` slash command so the
/// user can recover from a mid-session embed failure (e.g. LM Studio
/// just loaded the embed model) without restarting the process. Returns
/// the probe's own `Result` so the slash handler can format a success/
/// failure message.
pub fn reprobe_recall() -> Result<(), String> {
    RECALL_INDEX_BROKEN.store(false, std::sync::atomic::Ordering::Relaxed);
    crate::recall::probe()
}

/// Pre-flight the recall embedder by running a tiny embed call at REPL/TUI
/// startup. On failure (e.g. LM Studio's "No models loaded" 400), set the
/// sticky-disable flag and print one clear warn line. Silent on success so
/// healthy startups stay quiet.
///
/// Called once per process. Honors `CLAUDETTE_RECALL_DISABLE=1` — if recall
/// is already opted out, the probe is a no-op (we don't want to wake the
/// store at all in privacy mode).
pub(crate) fn probe_recall_at_startup() {
    if recall_disabled() {
        return;
    }
    if let Err(e) = crate::recall::probe() {
        mark_recall_index_broken();
        eprintln!(
            "{} {}",
            theme::warn(theme::WARN_GLYPH),
            theme::warn(&format!(
                "recall: probe failed — {e}. Indexing disabled for this session \
                 (load an embed model and restart, or set CLAUDETTE_RECALL_DISABLE=1 to silence)."
            ))
        );
    }
}

/// Extract the (user, assistant) snippets for one turn — pure CPU,
/// returns owned strings so callers can enqueue them onto the async
/// indexer channel without holding a borrow on the runtime. Empty
/// snippets stay empty (the indexer thread skips them).
///
/// Why we pass `user_input` directly instead of walking back to find the
/// "latest user message": on retries, the runtime injects a synthetic
/// nudge user-message into the session (see [`run_turn_with_retry`]). The
/// raw `trimmed` REPL line is what the human actually typed, so we
/// index that and skip the synthetic.
fn extract_turn_snippets<C, T>(
    user_input: &str,
    runtime: &ConversationRuntime<C, T>,
) -> (String, String)
where
    C: crate::ApiClient,
    T: crate::ToolExecutor,
{
    use crate::ContentBlock;
    let user_text = user_input.trim().to_string();
    let mut asst_text = String::new();
    if let Some(msg) = runtime
        .session()
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, crate::MessageRole::Assistant))
    {
        for block in &msg.blocks {
            if let ContentBlock::Text { text: t } = block {
                if !asst_text.is_empty() {
                    asst_text.push('\n');
                }
                asst_text.push_str(t);
            }
        }
    }
    (user_text, asst_text)
}

/// One job for the background recall indexer.
struct IndexJob {
    role: crate::recall::Role,
    snippet: String,
}

/// Lazily-spawned mpsc channel for the recall indexer thread. The Sender
/// is cloned on every push; the Receiver is owned by the one worker
/// thread spawned on first use. Channel-close (last Sender dropped at
/// process exit) terminates the thread cleanly.
fn recall_index_sender() -> &'static std::sync::mpsc::Sender<IndexJob> {
    use std::sync::OnceLock;
    static SENDER: OnceLock<std::sync::mpsc::Sender<IndexJob>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<IndexJob>();
        std::thread::Builder::new()
            .name("recall-indexer".to_string())
            .spawn(move || {
                // Drain until the channel closes. Each failed embed call
                // sets the sticky-disable flag and logs once; subsequent
                // jobs that slip through (in flight before the flag flipped)
                // also fail-fast on the same flag check.
                while let Ok(job) = rx.recv() {
                    if !recall_index_allowed() {
                        continue;
                    }
                    if job.snippet.trim().is_empty() {
                        continue;
                    }
                    if let Err(e) = crate::recall::global_index(job.role, &job.snippet) {
                        mark_recall_index_broken();
                        eprintln!(
                            "{} {}",
                            theme::warn(theme::WARN_GLYPH),
                            theme::warn(&format!(
                                "recall: {e} — disabling recall indexing for this session \
                                 (run /recall reprobe to retry after loading the embed model)"
                            ))
                        );
                    }
                }
            })
            .expect("spawn recall-indexer thread");
        tx
    })
}

/// Enqueue this turn's (user, assistant) snippets for async indexing.
/// Cheap (one channel push per snippet) — the embed call itself happens
/// on the background thread spawned by [`recall_index_sender`]. This is
/// the foreground entry point the REPL/TUI/Telegram all hit after a
/// successful turn.
///
/// Pre-2026-05-15 the embed call ran synchronously here, blocking the
/// REPL ~100 ms typical and seconds on a cold embed model. Moving it
/// behind a channel restores per-turn latency to what the user sees on
/// the streamed brain text.
pub(crate) fn index_turn_for_recall<C, T>(user_input: &str, runtime: &ConversationRuntime<C, T>)
where
    C: crate::ApiClient,
    T: crate::ToolExecutor,
{
    let (user_text, asst_text) = extract_turn_snippets(user_input, runtime);
    let tx = recall_index_sender();
    if !user_text.is_empty() {
        let _ = tx.send(IndexJob {
            role: crate::recall::Role::User,
            snippet: user_text,
        });
    }
    if !asst_text.trim().is_empty() {
        let _ = tx.send(IndexJob {
            role: crate::recall::Role::Assistant,
            snippet: asst_text,
        });
    }
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
        let input_chars = request.input.chars().count();
        let _ = writeln!(
            err,
            "  {} {} wants to run ({} chars):",
            theme::warn(theme::WARN_GLYPH),
            theme::accent(&request.tool_name),
            input_chars
        );
        // Show the full command. The old code truncated at 200 chars, which
        // let an adversary-crafted payload hide past the preview edge while
        // bash ran the complete input. Split on newlines so multi-line
        // commands stay readable. `str::lines()` handles a trailing-newline-
        // less single-line case correctly — yields the one line.
        if request.input.is_empty() {
            let _ = writeln!(err, "    {}", theme::dim("(empty input)"));
        } else {
            for line in request.input.lines() {
                let _ = writeln!(err, "    {}", theme::dim(line));
            }
        }
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
/// Called from [`run_secretary_repl`] after every model turn. The metric
/// is `crate::estimate_session_tokens` (a char-count heuristic that
/// scales with the actual session size), not the cumulative input-token
/// counter that grows monotonically.
///
/// **Tiered behaviour (P3, 2026-05-04 queue):**
/// - At/above [`compact_threshold`] (1M default): hard compact, preserves
///   [`HARD_COMPACT_PRESERVE`] recent messages.
/// - At/above [`soft_compact_threshold`] but below the hard threshold:
///   soft compact, preserves [`SOFT_COMPACT_PRESERVE`] recent messages.
///   Only fires when the user opts in via `CLAUDETTE_SOFT_COMPACT_THRESHOLD`.
/// - Below both: no-op.
pub(crate) fn maybe_compact_session(
    runtime: &mut ConversationRuntime<OllamaApiClient, SecretaryToolExecutor>,
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

/// Decide what compaction (if any) the session needs. Returns
/// `(tier, preserve_recent_messages, threshold_that_fired)` or `None` when
/// neither threshold is crossed.
///
/// Pure function — separates the tiering policy from the runtime-mutating
/// half of `maybe_compact_session` so it can be unit-tested without
/// constructing a runtime.
#[must_use]
pub(crate) fn pick_compact_plan(
    estimated: usize,
    hard_threshold: usize,
    soft_threshold: Option<usize>,
) -> Option<(CompactionTier, usize, usize)> {
    if estimated >= hard_threshold {
        return Some((CompactionTier::Hard, HARD_COMPACT_PRESERVE, hard_threshold));
    }
    if let Some(soft) = soft_threshold {
        if estimated >= soft {
            return Some((CompactionTier::Soft, SOFT_COMPACT_PRESERVE, soft));
        }
    }
    None
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

    // ─── Tiered compaction policy (P3) ──────────────────────────────────────

    #[test]
    fn pick_compact_plan_returns_none_below_both_thresholds() {
        assert_eq!(pick_compact_plan(50_000, 1_000_000, Some(200_000)), None);
        assert_eq!(pick_compact_plan(50_000, 1_000_000, None), None);
    }

    #[test]
    fn pick_compact_plan_returns_soft_when_only_soft_crossed() {
        assert_eq!(
            pick_compact_plan(250_000, 1_000_000, Some(200_000)),
            Some((CompactionTier::Soft, SOFT_COMPACT_PRESERVE, 200_000))
        );
    }

    #[test]
    fn pick_compact_plan_returns_hard_when_hard_crossed() {
        assert_eq!(
            pick_compact_plan(1_500_000, 1_000_000, Some(200_000)),
            Some((CompactionTier::Hard, HARD_COMPACT_PRESERVE, 1_000_000))
        );
    }

    #[test]
    fn pick_compact_plan_prefers_hard_over_soft_when_both_crossed() {
        // At >= hard, the soft tier is skipped — we want maximally aggressive
        // summarisation when the session is genuinely huge.
        assert_eq!(
            pick_compact_plan(2_000_000, 1_000_000, Some(200_000)),
            Some((CompactionTier::Hard, HARD_COMPACT_PRESERVE, 1_000_000))
        );
    }

    #[test]
    fn pick_compact_plan_skips_soft_when_threshold_unset() {
        // No CLAUDETTE_SOFT_COMPACT_THRESHOLD set → only the hard threshold
        // gates compaction, preserving the historical one-tier behaviour.
        assert_eq!(pick_compact_plan(500_000, 1_000_000, None), None);
    }

    #[test]
    fn compaction_tier_names_are_lowercase_for_logs() {
        // The log message format expects bare lowercase names ("soft tier
        // @ 200000 tokens") — any change here is a user-visible log break,
        // so pin the strings.
        assert_eq!(CompactionTier::Soft.name(), "soft");
        assert_eq!(CompactionTier::Hard.name(), "hard");
    }

    #[test]
    fn soft_compact_threshold_returns_none_when_unset() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CLAUDETTE_SOFT_COMPACT_THRESHOLD").ok();
        std::env::remove_var("CLAUDETTE_SOFT_COMPACT_THRESHOLD");

        assert_eq!(soft_compact_threshold(), None);

        if let Some(v) = prev {
            std::env::set_var("CLAUDETTE_SOFT_COMPACT_THRESHOLD", v);
        }
    }

    #[test]
    fn soft_compact_threshold_returns_some_when_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CLAUDETTE_SOFT_COMPACT_THRESHOLD").ok();
        std::env::set_var("CLAUDETTE_SOFT_COMPACT_THRESHOLD", "200000");

        assert_eq!(soft_compact_threshold(), Some(200_000));

        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_SOFT_COMPACT_THRESHOLD", v),
            None => std::env::remove_var("CLAUDETTE_SOFT_COMPACT_THRESHOLD"),
        }
    }

    /// Helper: run `git` with args under `dir`, asserting success. Tests
    /// for `capture_git_diff` need to drive a real repo since we shell out.
    #[cfg(test)]
    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git should be on PATH for forge tests");
        assert!(status.success(), "git {args:?} failed in {dir:?}");
    }

    /// Repro of the 2026-05-16 false-negative: Coder commits its changes,
    /// `git diff HEAD` returns empty afterwards, Verifier sees no work.
    /// Fix: snapshot the base SHA before Coder runs, use `base..HEAD`.
    #[test]
    fn capture_git_diff_with_base_sees_committed_coder_work() {
        let dir = std::env::temp_dir().join(format!(
            "claudette-forge-diff-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();

        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("seed.txt"), "seed\n").unwrap();
        git(&dir, &["add", "seed.txt"]);
        git(&dir, &["commit", "-q", "-m", "seed"]);

        // Snapshot base BEFORE the simulated Coder commit.
        let base = capture_base_sha(&dir).expect("base SHA should be capturable");

        // Simulate the Coder phase: edit a file and commit.
        std::fs::write(dir.join("new.txt"), "coder output\n").unwrap();
        git(&dir, &["add", "new.txt"]);
        git(&dir, &["commit", "-q", "-m", "coder change"]);

        // OLD behavior (base=None): working-tree diff = empty after commit.
        let old_diff = capture_git_diff(&dir, None).expect("git diff HEAD should succeed");
        assert!(
            old_diff.trim().is_empty(),
            "without base SHA, post-commit diff should be empty (this is the bug we're fixing); got {old_diff:?}"
        );

        // NEW behavior: base..HEAD shows the Coder's commit.
        let new_diff = capture_git_diff(&dir, Some(&base)).expect("base..HEAD diff should succeed");
        assert!(
            new_diff.contains("coder output"),
            "base..HEAD diff should include the Coder's changes; got {new_diff:?}"
        );
        assert!(
            new_diff.contains("new.txt"),
            "base..HEAD diff should mention the new file; got {new_diff:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_base_sha_returns_none_on_fresh_repo() {
        // No commits yet → rev-parse HEAD fails → callers fall back to
        // working-tree diff. Verify we don't panic and don't return a sha.
        let dir = std::env::temp_dir().join(format!(
            "claudette-forge-emptyrepo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);

        assert!(capture_base_sha(&dir).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn soft_compact_threshold_treats_zero_as_unset() {
        // 0 is a magic "disabled" value — explicit opt-out via env without
        // having to unset.
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CLAUDETTE_SOFT_COMPACT_THRESHOLD").ok();
        std::env::set_var("CLAUDETTE_SOFT_COMPACT_THRESHOLD", "0");

        assert_eq!(soft_compact_threshold(), None);

        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_SOFT_COMPACT_THRESHOLD", v),
            None => std::env::remove_var("CLAUDETTE_SOFT_COMPACT_THRESHOLD"),
        }
    }

    /// Regression test: every tool name advertised in `secretary_tools_json`
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
        let full = crate::tools::secretary_tools_json();
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

    /// The bundled `codex7` persona is baked into the binary via
    /// `include_str!`. If the file is edited into invalid TOML or stripped of
    /// its frontmatter, `forge_default_coder_persona` returns `None` and
    /// forge-mode silently runs without a persona. Catch that at build time.
    #[test]
    fn forge_default_coder_persona_parses_bundled_codex7() {
        let p = forge_default_coder_persona().expect("bundled codex7 must parse");
        assert_eq!(p.name, "CodeX-7");
        assert_eq!(p.role, forge::types::Role::Coder);
        assert!(!p.voice.is_empty(), "codex7 should have a voice");
        assert!(!p.backstory.is_empty(), "codex7 should have backstory");
    }

    /// `forge_role_model` is best-effort: a missing `~/.claudettes-forge/
    /// models.toml` returns the built-in default (qwen3-coder:30b for Coder,
    /// qwen3.5:14b for Planner/Verifier), env overrides win when set. The
    /// smoke test asserts each forge role returns *some* non-empty string
    /// in a clean environment (defaults from
    /// `forge::models_toml::default_model_map`).
    #[test]
    fn forge_role_model_returns_a_default_for_each_role() {
        for role in [
            forge::types::Role::Coder,
            forge::types::Role::Planner,
            forge::types::Role::Verifier,
        ] {
            let model = forge_role_model(role)
                .unwrap_or_else(|| panic!("forge default model for {role:?}"));
            assert!(!model.is_empty(), "{role:?} model name must be non-empty");
        }
    }

    // ─── Forge v0c: Verifier JSON parsing ─────────────────────────────

    #[test]
    fn verifier_parses_clean_json() {
        let r = parse_verifier_response(r#"{"score": 9, "pass": true, "feedback": "looks good"}"#);
        assert_eq!(r.score, 9);
        assert!(r.pass);
        assert_eq!(r.feedback, "looks good");
    }

    #[test]
    fn verifier_parses_json_in_code_fence() {
        let r = parse_verifier_response(
            "```json\n{\"score\": 5, \"pass\": false, \"feedback\": \"missing tests\"}\n```",
        );
        assert_eq!(r.score, 5);
        assert!(!r.pass);
        assert_eq!(r.feedback, "missing tests");
    }

    #[test]
    fn verifier_parses_json_with_trailing_prose() {
        let r = parse_verifier_response(
            "Here is my evaluation:\n{\"score\": 7, \"pass\": true, \"feedback\": \"ok\"}\nDone.",
        );
        assert_eq!(r.score, 7);
        assert!(r.pass);
    }

    #[test]
    fn verifier_unparseable_falls_through_to_pass() {
        // Garbage in → permissive default. This prevents a flaky local model
        // from deadlocking the forge pipeline.
        let r = parse_verifier_response("I don't know how to format JSON");
        assert!(r.pass);
        assert_eq!(r.score, 10);
        assert!(r.feedback.is_empty());
    }

    #[test]
    fn verifier_clamps_out_of_range_scores() {
        // A model that returns score=42 (or any value > 10) gets clamped to
        // 10 rather than overflowing or rejecting the response.
        let r = parse_verifier_response(r#"{"score": 42, "pass": false, "feedback": "x"}"#);
        assert_eq!(r.score, 10);
        assert!(!r.pass);
    }

    #[test]
    fn verifier_missing_fields_use_permissive_defaults() {
        // Only `score` present → pass defaults to true, feedback empty.
        let r = parse_verifier_response(r#"{"score": 6}"#);
        assert_eq!(r.score, 6);
        assert!(r.pass);
        assert!(r.feedback.is_empty());
    }
}
