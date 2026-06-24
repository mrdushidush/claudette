//! Top-level entry points — single-shot and REPL.

use std::io::{self, Write};
use std::path::PathBuf;

use crate::{
    compact_session, estimate_session_tokens, CompactionConfig, ConversationRuntime,
    PermissionPrompter, Session, TurnSummary,
};
use anyhow::{Context, Result};

use crate::api::OllamaApiClient;
use crate::executor::AgentToolExecutor;
use crate::forge;
use crate::model_config;
use crate::prompt::{forge_planner_system_prompt, forge_verifier_system_prompt};
use crate::theme;
use crate::tool_groups::ToolGroup;

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

/// Default cap on TOTAL Coder→Verifier fix-loop passes in v0c forge-mode
/// (round 0 = initial pass; the loop runs at most this many passes total).
/// Empirically three passes is the sweet spot — a local 8b coder model that
/// didn't get it after three passes usually won't, and burning more rounds
/// runs the user's context budget into the ground. (Roast RC-H F2: the knob
/// is now total passes, not "additional rounds", so the count matches the
/// documented number instead of running one extra.)
const DEFAULT_MAX_FIX_ROUNDS: u32 = 3;

/// Hard upper bound on fix-loop passes, even if `CLAUDETTE_MAX_FIX_ROUNDS`
/// is set higher. Past ~10 passes the brain is reliably stuck in a local
/// minimum and the right move is to bail and let the user re-prompt.
const FIX_ROUNDS_HARD_CAP: u32 = 10;

/// Resolve the active fix-loop pass cap (total Coder passes). Honors
/// `CLAUDETTE_MAX_FIX_ROUNDS`, clamped to `[1, FIX_ROUNDS_HARD_CAP]`, and
/// falls back to `DEFAULT_MAX_FIX_ROUNDS` on missing input. An unparseable
/// value warns and falls back (roast RC-H F4: a typo'd knob was previously
/// indistinguishable from unset). Read on every call — the forge loop fires
/// a few times per mission so the cost is negligible.
fn max_fix_rounds() -> u32 {
    match std::env::var("CLAUDETTE_MAX_FIX_ROUNDS") {
        Ok(raw) => match raw.trim().parse::<u32>() {
            // Floor of 1: there is always at least one Coder pass; "0" never
            // meant anything coherent.
            Ok(n) => n.clamp(1, FIX_ROUNDS_HARD_CAP),
            Err(_) => {
                eprintln!(
                    "  {} {}",
                    theme::dim("∘"),
                    theme::warn(&format!(
                        "CLAUDETTE_MAX_FIX_ROUNDS={raw:?} is not a number — using default {DEFAULT_MAX_FIX_ROUNDS}"
                    ))
                );
                DEFAULT_MAX_FIX_ROUNDS
            }
        },
        Err(_) => DEFAULT_MAX_FIX_ROUNDS,
    }
}

/// Opt-in: when set, forge phases auto-approve every tool call (the runtime
/// uses `PermissionMode::Allow`, so the [y/N] prompter is never consulted).
/// For UNATTENDED / scripted forge runs only — DangerFullAccess tools (bash,
/// git, apply_diff) then run without confirmation, so only enable it for
/// throwaway repos. Off by default; affects forge phases only (secretary/TUI
/// keep the normal WorkspaceWrite+prompt policy).
fn forge_auto_approve_enabled() -> bool {
    matches!(
        std::env::var("CLAUDETTE_FORGE_AUTO_APPROVE").as_deref(),
        Ok("1" | "true" | "yes" | "on")
    )
}

/// True for the canonical truthy env values. Shared by the forge gate knobs.
fn env_flag_enabled(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1" | "true" | "yes" | "on")
    )
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

/// Opt-in (roast RC-C): proceed with the Submitter even when a HIGH-severity
/// security finding survived the fix-loop. Off by default — a surviving HIGH
/// hard-blocks PR creation. Only set this once you've reviewed the finding.
fn security_override_enabled() -> bool {
    env_flag_enabled("CLAUDETTE_FORGE_SECURITY_OVERRIDE")
}

/// Opt-in (roast RC-A MED-7): open a PR even when the Verifier never passed
/// within the round limit. Off by default — forge declines to submit work the
/// gate rejected and leaves the commits on the mission branch for inspection.
fn submit_on_fail_enabled() -> bool {
    env_flag_enabled("CLAUDETTE_FORGE_SUBMIT_ON_FAIL")
}

/// Opt-in (roast RC-D): allow forge to operate on a dirty / mid-merge /
/// detached working tree. Off by default — Phase 0 refuses rather than risk
/// `git reset --hard` clobbering the user's uncommitted work or committing
/// onto an in-progress branch.
fn allow_dirty_tree_enabled() -> bool {
    env_flag_enabled("CLAUDETTE_FORGE_ALLOW_DIRTY")
}

/// Seconds the ephemeral-mission auto-bootstrap waits before proceeding so
/// the user can Ctrl+C if cwd wasn't what they intended. Default 3; set
/// `CLAUDETTE_FORGE_ABORT_WINDOW_SECS=0` to disable (e.g. CI / scripted
/// runs), or to a larger value for cautious workflows. Clamped to [0, 30]
/// — a 30-second wait is the longest that's still a safety pause rather
/// than just an annoyance.
fn ephemeral_abort_window_secs() -> u64 {
    std::env::var("CLAUDETTE_FORGE_ABORT_WINDOW_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(3)
        .min(30)
}

/// Whether the Verifier runs the project build + test suite each round (the
/// deterministic correctness gate). ON by default — a diff that doesn't compile
/// or breaks tests is exactly what an LLM-reading-the-diff misses. Opt out with
/// `CLAUDETTE_FORGE_NO_BUILD_CHECK=1` for repos where the suite is slow, needs
/// network, or requires an install step forge can't perform.
fn forge_build_check_enabled() -> bool {
    !env_flag_enabled("CLAUDETTE_FORGE_NO_BUILD_CHECK")
}

/// Per-step timeout (seconds) for the Verifier's build + test commands. Default
/// 180; override with `CLAUDETTE_FORGE_TEST_TIMEOUT_SECS`. Clamped to
/// `[10, 1800]` — below 10s nothing meaningful compiles; above 30 minutes a
/// hung suite would stall the whole pipeline.
fn forge_test_timeout_secs() -> u64 {
    std::env::var("CLAUDETTE_FORGE_TEST_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(180)
        .clamp(10, 1800)
}

/// Whether the human-review gate fires before the Submitter opens a PR. ON by
/// default for attended runs — the user reviews the plan + full diff and
/// approves before anything is pushed (this is the QA step). Skipped when:
///   • auto-approve is on (`CLAUDETTE_FORGE_AUTO_APPROVE`) — an explicitly
///     unattended run has nobody to answer, or
///   • the user opts out with `CLAUDETTE_FORGE_NO_REVIEW=1` (back to the old
///     hands-off submit).
fn forge_human_review_enabled() -> bool {
    if forge_auto_approve_enabled() {
        return false;
    }
    !env_flag_enabled("CLAUDETTE_FORGE_NO_REVIEW")
}

/// Max diff lines shown inline at the review gate before truncating. Generous
/// enough to eyeball a normal change; the full diff is always recoverable from
/// the mission tree with `git diff`.
const REVIEW_DIFF_MAX_LINES: usize = 600;

/// Split `diff` into `(shown, omitted_line_count)` at `max` lines so a huge
/// diff doesn't scroll the approval prompt off-screen.
fn truncate_diff_for_review(diff: &str, max: usize) -> (String, usize) {
    let total = diff.lines().count();
    if total <= max {
        return (diff.to_string(), 0);
    }
    let shown = diff.lines().take(max).collect::<Vec<_>>().join("\n");
    (shown, total - max)
}

/// Interactive human-review gate. Prints the plan + the full final diff to
/// stderr, then reads `y/N` from stdin. Returns `true` ONLY on an explicit
/// "y"/"yes". Any other answer — including EOF / a non-interactive stdin —
/// returns `false` (fail-closed: never open a PR nobody approved). This is the
/// user's QA step before [`run_forge_mission`]'s Submitter phase opens the PR.
fn forge_confirm_submit(plan: &str, diff: &str, passed: bool) -> bool {
    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(err);
    let _ = writeln!(
        err,
        "{} {}",
        theme::BOLT,
        theme::accent("forge: review — approve before opening the PR")
    );

    let plan_t = plan.trim();
    if !plan_t.is_empty() {
        let _ = writeln!(err, "{}", theme::dim("── plan ──────────────────────────"));
        for line in plan_t.lines() {
            let _ = writeln!(err, "  {}", theme::dim(line));
        }
    }

    let _ = writeln!(err, "{}", theme::dim("── diff ──────────────────────────"));
    let (shown, omitted) = truncate_diff_for_review(diff, REVIEW_DIFF_MAX_LINES);
    if shown.trim().is_empty() {
        let _ = writeln!(err, "  {}", theme::dim("(empty diff)"));
    } else {
        for line in shown.lines() {
            let _ = writeln!(err, "  {line}");
        }
    }
    if omitted > 0 {
        let _ = writeln!(
            err,
            "  {}",
            theme::warn(&format!(
                "… {omitted} more diff line(s) not shown — inspect the full diff with `git diff` \
                 in the mission tree"
            ))
        );
    }

    let verdict = if passed {
        theme::ok("automated checks passed").to_string()
    } else {
        theme::warn("automated checks did NOT fully pass").to_string()
    };
    let _ = writeln!(err, "  {} {verdict}", theme::dim("verdict:"));
    let _ = write!(
        err,
        "  {} Open the PR with these changes? [y/N] ",
        theme::warn(theme::WARN_GLYPH)
    );
    let _ = err.flush();

    let mut buf = String::new();
    match io::stdin().read_line(&mut buf) {
        // EOF (Ok(0)) means non-interactive stdin — treat as "not approved".
        Ok(0) => false,
        Ok(_) => {
            let answer = buf.trim().to_lowercase();
            answer == "y" || answer == "yes"
        }
        Err(_) => false,
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

/// One forge fix-loop round: the Verifier's score + the Coder's resulting
/// HEAD SHA. Captured so [`run_forge_mission`] can: (1) smart-stop when the
/// score is regressing two consecutive rounds, and (2) restore to the best-
/// scoring round's commit before the Submitter phase if the final round
/// scored lower than an earlier one ([[project-import-sprint-2026-05-19]]
/// Phase 3 — BCF learning #12 "full regen always degrades score").
#[derive(Debug, Clone)]
pub(crate) struct RoundReport {
    pub round: u32,
    /// HEAD after the Coder committed. `None` when git rev-parse failed —
    /// the round is still tracked for scoring but can't participate in
    /// best-round restore.
    pub head_sha: Option<String>,
    pub score: u8,
    pub pass: bool,
    /// True when the security review found a HIGH-severity issue in this
    /// round's diff. Tracked separately from `score` (which the security
    /// stage never mutates) so [`best_round`] can refuse to restore a
    /// vulnerable round over a clean one (roast RC-C).
    pub security_high: bool,
}

/// Pick the best round to restore from `history`. Ordering (roast RC-C), best
/// first: (1) a passing round beats a failing one, (2) a security-clean round
/// beats one with a HIGH finding, (3) then highest score, (4) then lowest
/// round index (earlier-is-better, so we don't `git reset` for nothing).
///
/// This prevents restoring a high-*scoring* round that the security stage
/// condemned over a clean lower-scoring one — the score alone is not the
/// authoritative key, because the security review never lowers the score.
/// Returns `None` when `history` is empty or no entry has a recoverable
/// `head_sha`.
pub(crate) fn best_round(history: &[RoundReport]) -> Option<&RoundReport> {
    history
        .iter()
        .filter(|r| r.head_sha.is_some())
        .min_by(|a, b| {
            // `min_by` keeps the *smallest*, so map "better" to "smaller":
            // pass first (false sorts after true via !pass), then clean
            // (security_high=false first), then higher score, then earlier.
            (!a.pass)
                .cmp(&!b.pass)
                .then_with(|| a.security_high.cmp(&b.security_high))
                .then_with(|| b.score.cmp(&a.score))
                .then_with(|| a.round.cmp(&b.round))
        })
}

/// True when `history`'s last three entries are strictly monotonically
/// declining in score. Triggers the smart-stop break in
/// [`run_forge_mission`]. Returns `false` for fewer than 3 entries — we
/// need a baseline plus two declines (so the name's "two consecutive" refers
/// to two *drops* across three data points).
///
/// NOTE (roast RC-H F3): this needs ≥3 history entries, so at the default
/// `DEFAULT_MAX_FIX_ROUNDS` it can only fire on the same final pass the round
/// cap would break on anyway — it changes the exit *message*, not the pass
/// count. It only saves passes when `CLAUDETTE_MAX_FIX_ROUNDS` is raised to
/// ≥4. This is intentional: triggering on a single drop (2 entries) stops too
/// eagerly on a normal one-round dip.
pub(crate) fn score_declining_two_consecutive(history: &[RoundReport]) -> bool {
    if history.len() < 3 {
        return false;
    }
    let n = history.len();
    let a = history[n - 3].score;
    let b = history[n - 2].score;
    let c = history[n - 1].score;
    b < a && c < b
}

/// Truncate `sha` to its first 7 hex chars for log output. Short-circuits
/// on already-short inputs so empty / malformed SHAs don't panic.
fn short_sha(sha: &str) -> &str {
    let end = sha.len().min(7);
    &sha[..end]
}

/// `git reset --hard <sha>` inside `mission_path`. Returns the command's
/// stderr on failure so the caller can surface the reason. Used by
/// [`run_forge_mission`]'s best-round restore path.
fn git_reset_hard(mission_path: &std::path::Path, sha: &str) -> Result<(), String> {
    let output = std::process::Command::new("git")
        .args(["reset", "--hard", sha])
        .current_dir(mission_path)
        .output()
        .map_err(|e| format!("git reset --hard {sha}: spawn failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git reset --hard {sha}: {stderr}"));
    }
    Ok(())
}

/// Phase-0 safety pre-flight for an ephemeral (cwd-rooted) mission on the
/// user's *live* repo (roast RC-D). Brownfield missions (cloned into
/// `~/.claudette/missions/`) are skipped — their tree is a fresh clone, so
/// none of these hazards apply and the existing flow is left untouched.
///
/// For an ephemeral mission this:
/// 1. refuses a dirty working tree (uncommitted/untracked changes) so a later
///    `git reset --hard` can't silently destroy the user's in-progress work,
/// 2. refuses a mid-merge / mid-rebase / detached-HEAD / state we can't safely
///    branch from and restore,
/// 3. creates and checks out a dedicated `claudette-mission/<slug>-<ts>` branch
///    so AI commits never land on the user's current branch.
///
/// Returns `Ok(Some((repo, original_branch)))` when a branch was created (the
/// caller arms the guard to restore it), `Ok(None)` when there's nothing to do
/// (non-ephemeral), or `Err` when forge should refuse to proceed. The dirty /
/// non-clean refusals are overridable with `CLAUDETTE_FORGE_ALLOW_DIRTY=1`.
fn forge_phase0_preflight(mission: &crate::missions::Mission) -> Result<Option<(PathBuf, String)>> {
    if !mission.ephemeral {
        return Ok(None);
    }
    let path = &mission.path;
    let git = |args: &[&str]| -> Result<std::process::Output> {
        std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .map_err(|e| anyhow::anyhow!("git {}: {e}", args.join(" ")))
    };

    // Detached HEAD / unknown branch — `--abbrev-ref HEAD` yields "HEAD" when
    // detached. We need a real branch to return to.
    let head_out = git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    let original_branch = String::from_utf8_lossy(&head_out.stdout).trim().to_string();
    if !head_out.status.success() || original_branch.is_empty() || original_branch == "HEAD" {
        return Err(anyhow::anyhow!(
            "forge: the working tree at {} is in a detached-HEAD state (no current branch). \
             Check out a branch first so forge can isolate its commits and restore your branch \
             afterwards.",
            path.display()
        ));
    }

    // Mid-merge / mid-rebase — committing here would finalize a half-resolved
    // operation and `git add` would stage conflict markers.
    let git_dir = {
        let out = git(&["rev-parse", "--git-dir"])?;
        let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let p = std::path::PathBuf::from(&raw);
        if p.is_absolute() {
            p
        } else {
            path.join(p)
        }
    };
    if git_dir.join("MERGE_HEAD").exists()
        || git_dir.join("rebase-merge").exists()
        || git_dir.join("rebase-apply").exists()
    {
        return Err(anyhow::anyhow!(
            "forge: the working tree at {} is in the middle of a merge or rebase. Finish or abort \
             it before running forge.",
            path.display()
        ));
    }

    // Dirty tree — uncommitted/untracked changes are at the mercy of the
    // best-round restore's `git reset --hard` and the submit `git add`.
    let status_out = git(&["status", "--porcelain"])?;
    let dirty = !String::from_utf8_lossy(&status_out.stdout)
        .trim()
        .is_empty();
    if dirty && !allow_dirty_tree_enabled() {
        return Err(anyhow::anyhow!(
            "forge: the working tree at {} has uncommitted or untracked changes. forge commits and \
             may `git reset --hard` on this tree, which would destroy that work. Commit or stash \
             it first, or set CLAUDETTE_FORGE_ALLOW_DIRTY=1 to proceed anyway (your changes will \
             be carried onto the mission branch).",
            path.display()
        ));
    }

    // Create + check out a dedicated mission branch so AI commits are isolated.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let branch = format!("claudette-mission/{}-{ts}", mission.slug);
    let co = git(&["checkout", "-b", &branch])?;
    if !co.status.success() {
        return Err(anyhow::anyhow!(
            "forge: failed to create mission branch {branch}: {}",
            String::from_utf8_lossy(&co.stderr).trim()
        ));
    }
    eprintln!(
        "  {} {}",
        theme::dim("∘"),
        theme::accent(&format!(
            "forge: isolated commits on branch {branch} (will restore {original_branch} on exit)"
        )),
    );
    Ok(Some((path.clone(), original_branch)))
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
/// `opts.resume` and `opts.autosave` behave as in [`run_agent`]: forge
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
        None => {
            // F8b safety gate: if the user invoked /brownfield earlier in
            // this process and it failed, refuse to silently fall back to
            // a cwd-rooted ephemeral mission. They were explicit about
            // wanting to target a different repo; running forge against
            // the dev tree instead is a footgun.
            if crate::missions::brownfield_failed_this_session() {
                return Err(anyhow::anyhow!(
                    "forge: refusing to auto-bootstrap from cwd because a \
                     /brownfield invocation failed earlier in this session. \
                     Fix the underlying error and retry /brownfield, or run \
                     /mission_exit to clear the failure flag and operate on \
                     the current directory."
                ));
            }
            match crate::missions::try_bootstrap_local_mission() {
                Ok(m) => {
                    // F8 safety: surface this loud and clear. forge will
                    // commit AI-generated changes into this tree, and the
                    // pre-fix dim line was easy to miss in a busy terminal.
                    eprintln!();
                    eprintln!(
                        "{} {}",
                        theme::warn(theme::WARN_GLYPH),
                        theme::warn(
                            "forge: NO active brownfield mission — \
                                     auto-bootstrapping an ephemeral mission \
                                     rooted at the current directory."
                        )
                    );
                    eprintln!(
                        "  {} {} {}",
                        theme::dim("∘"),
                        theme::dim("target tree:"),
                        theme::accent(&m.path.display().to_string()),
                    );
                    let abort_secs = ephemeral_abort_window_secs();
                    if abort_secs > 0 {
                        eprintln!(
                            "  {} {}",
                            theme::dim("∘"),
                            theme::dim(&format!(
                                "commits will land here. Press Ctrl+C in the next {abort_secs} \
                                 seconds to abort if this isn't what you want."
                            )),
                        );
                        std::thread::sleep(std::time::Duration::from_secs(abort_secs));
                    } else {
                        eprintln!(
                            "  {} {}",
                            theme::dim("∘"),
                            theme::dim("commits will land here."),
                        );
                    }
                    eprintln!();
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
            }
        }
    };

    // Guard for the ephemeral path: any early return from this point on
    // clears the mission slot if and only if WE installed it. User-
    // initiated missions (`/brownfield`, `mission_attach`) are left alone
    // so the user can retry / inspect after a forge failure. Disarmed at
    // the end of the happy path so a successful run also leaves the slot
    // intact (lets subsequent `/forge` invocations in the same REPL keep
    // the same mission without re-bootstrapping).
    let mut cleanup = EphemeralMissionGuard::new(mission.ephemeral);

    // Loud one-time banner when running unattended (roast RC-B F5): under
    // auto-approve every tool call — including `bash`, which is unsandboxed —
    // runs with no confirmation against the target tree.
    if forge_auto_approve_enabled() {
        eprintln!(
            "  {} {}",
            theme::warn(theme::WARN_GLYPH),
            theme::warn(&format!(
                "AUTO-APPROVE ON — all tool calls (incl. unsandboxed `bash`) run WITHOUT \
                 confirmation against {}",
                mission.path.display()
            )),
        );
    }

    // Phase-0 safety pre-flight (roast RC-D): on an ephemeral cwd-rooted
    // mission, refuse a dirty/merging/detached tree and isolate AI commits on
    // a dedicated branch. Runs before any phase so a refusal costs nothing.
    match forge_phase0_preflight(&mission) {
        Ok(Some((repo, original_branch))) => cleanup.set_restore_branch(repo, original_branch),
        Ok(None) => {}
        Err(e) => return Err(e),
    }

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

    // Light localization sanity check (roast RC-F F3): the brief is trusted
    // blindly downstream and never re-planned, so if it names files that
    // don't exist under the mission tree, surface a warning — a confidently
    // wrong/hallucinated localization is the silent failure mode.
    if !plan.trim().is_empty() {
        warn_if_brief_paths_missing(&plan, &mission.path);
    }

    let augmented_input = if plan.trim().is_empty() {
        user_input.to_string()
    } else {
        format!("Plan:\n{}\n\nTask: {user_input}", plan.trim())
    };

    // ── Phase 2 + 3 + 4: Coder ↔ Verifier fix-loop ───────────────────
    //
    // Each round's HEAD SHA + Verifier score lands in `history` so the
    // post-loop best-round restore can `git reset --hard` to an earlier,
    // higher-scoring commit when the fix-pass regresses (BCF learning #12:
    // "full regen always degrades score"; smart-stop catches the chain of
    // two consecutive declines).
    let mut history: Vec<RoundReport> = Vec::new();
    let mut feedback: Option<String> = None;
    let mut round: u32 = 0;
    loop {
        eprintln!(
            "{} {} (round {})",
            theme::BOLT,
            theme::accent("forge: coder"),
            round
        );
        // Retry rounds keep the full Planner brief (roast RC-F F1): the brief
        // (relevant files + plan) is folded into `augmented_input` and was
        // previously dropped on every revision, so the Coder lost its
        // localization exactly when it needed to re-edit. Now the brief
        // persists across all rounds; only the feedback preamble is added.
        let coder_input = match &feedback {
            None => augmented_input.clone(),
            Some(f) => format!(
                "The Verifier rejected your previous attempt with this feedback:\n{f}\n\n\
                 Revise your work — add additional commits to the same branch as needed. \
                 Do NOT push or call mission_submit yet; the Verifier will review again.\n\n\
                 {augmented_input}"
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

        // Snapshot HEAD now — the Coder turn's commit (if any) lands the
        // round's diff on top of the mission branch.
        let head_after = capture_base_sha(&mission.path);

        // Verifier
        eprintln!("{} {}", theme::BOLT, theme::accent("forge: verifier"));
        let diff = capture_git_diff(&mission.path, base_sha.as_deref()).unwrap_or_default();
        let mut verifier = run_verifier(
            session.clone(),
            &mission,
            user_input,
            &plan,
            &diff,
            &mut prompter_opt,
        )
        .unwrap_or_else(|e| {
            eprintln!(
                "  {} {}",
                theme::dim("∘"),
                theme::dim(&format!("verifier errored: {e}"))
            );
            // FAIL-CLOSED (roast RC-A HIGH-4): a verifier turn error (timeout,
            // OOM, provider 5xx) is an abstention, not an endorsement. Was
            // pass=true/score=10, which shipped unverified diffs on infra
            // failure and let an errored round win best-round restore.
            VerifierResult {
                score: 0,
                pass: false,
                feedback: format!("verifier turn failed ({e}) — treated as fail"),
            }
        });

        // Empty / no-commit diff guard (roast RC-A HIGH / RC-H F1): if the
        // Coder committed nothing, `diff` is empty and the Verifier would be
        // grading a blank diff. Force a fail so the known no-commit failure
        // mode can't route to a default-pass and submit a zero-line PR.
        if diff.trim().is_empty() {
            verifier.pass = false;
            verifier.score = 0;
            if verifier.feedback.trim().is_empty() {
                verifier.feedback =
                    "no committed changes were produced — commit your edits to the mission \
                     branch (use apply_diff/edit_file then git_add + git_commit) before the \
                     Verifier can review."
                        .to_string();
            }
        }

        // ── Build + test gate (on by default) ──────────────────────────
        // The Verifier above only *reads* the diff; it can't see a type error
        // or a test the change regressed. Run the project's real build + test
        // suite in the mission tree and turn the result into a deterministic
        // gate: a build break or a failing test forces pass=false and feeds the
        // failures back to the Coder for the next round. Infra problems (no
        // framework, tool missing, timeout) stay advisory so a docs PR isn't
        // blocked by a flaky/uninstalled suite. Opt out with
        // CLAUDETTE_FORGE_NO_BUILD_CHECK=1. (Skipped on an empty diff — nothing
        // changed to verify.)
        if forge_build_check_enabled() && !diff.trim().is_empty() {
            eprintln!("{} {}", theme::BOLT, theme::accent("forge: build + test"));
            let outcome = crate::tools::quality::run_build_and_tests(
                &mission.path,
                forge_test_timeout_secs(),
            );
            for line in outcome.summary.lines() {
                eprintln!("  {} {}", theme::dim("∘"), theme::dim(line));
            }
            // `ran=false` (no framework detected) leaves build_ok/tests_ok None,
            // so is_hard_fail() is already false — the LLM Verifier verdict
            // stands. The summary above explains the skip.
            if outcome.ran && outcome.is_hard_fail() {
                verifier.pass = false;
                // Score the round down so best-round restore never picks a
                // round whose build/tests are broken over a clean one.
                verifier.score = 0;
                let gate = format!(
                    "Automated build/test gate FAILED (framework: {}). Fix these before the \
                     change can pass:\n{}",
                    outcome.framework, outcome.summary
                );
                verifier.feedback = if verifier.feedback.trim().is_empty() {
                    gate
                } else {
                    format!("{gate}\n\n{}", verifier.feedback)
                };
            }
        }

        // ── Security review stage (opt-in) ─────────────────────────────
        // Scan the round's diff for unsafe constructs. HIGH findings flip
        // the round to "not passing" and prepend remediation feedback so
        // the Coder fixes them within the fix-loop (bounded by
        // max_fix_rounds); MEDIUM/LOW are advisory. Enable with
        // CLAUDETTE_FORGE_SECURITY_REVIEW=1.
        let mut security_high = false;
        if crate::security_review::enabled() {
            let findings = crate::security_review::scan_diff(&diff);
            if !findings.is_empty() {
                eprintln!(
                    "{} {}",
                    theme::BOLT,
                    theme::accent("forge: security review")
                );
                for f in &findings {
                    eprintln!("  {} {}", theme::dim("∘"), theme::dim(&f.to_string()));
                }
                security_high = findings
                    .iter()
                    .any(|f| f.severity == crate::security_review::Severity::High);
                // A HIGH finding is a hard fail, INDEPENDENT of the Verifier's
                // verdict (roast RC-C C1). Previously this only fired when the
                // Verifier had *already* passed (`has_high && verifier.pass`),
                // so a HIGH finding in a Verifier-rejected round dropped its
                // remediation feedback entirely and rode along on a later
                // "passing" round.
                if security_high {
                    let sec = crate::security_review::findings_feedback(&findings);
                    verifier.pass = false;
                    verifier.feedback = if verifier.feedback.trim().is_empty() {
                        sec
                    } else {
                        format!("{sec}\n\n{}", verifier.feedback)
                    };
                }
            }
        }

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

        history.push(RoundReport {
            round,
            head_sha: head_after.clone(),
            score: verifier.score,
            pass: verifier.pass,
            security_high,
        });

        if verifier.pass {
            break;
        }
        if score_declining_two_consecutive(&history) {
            let n = history.len();
            eprintln!(
                "  {} {}",
                theme::dim("∘"),
                theme::warn(&format!(
                    "smart-stop: score declined two consecutive rounds ({} → {} → {}); \
                     breaking out of fix-loop",
                    history[n - 3].score,
                    history[n - 2].score,
                    history[n - 1].score,
                ))
            );
            break;
        }
        // Round-cap break. `round` is 0-indexed and incremented at the end of
        // the loop body, so the loop runs `max_fix_rounds()` Coder passes
        // total: round 0 (initial) plus up to `max_fix_rounds()-1` revisions.
        // (Roast RC-H F2: the old `round >= max` post-increment guard ran
        // max+1 passes — "2 rounds" did 3.)
        if round + 1 >= max_fix_rounds() {
            eprintln!(
                "  {} {}",
                theme::dim("∘"),
                theme::warn(&format!(
                    "verifier still failing after {} round(s); stopping fix-loop",
                    round + 1
                ))
            );
            break;
        }
        // Accumulate a bounded feedback ledger so the Coder doesn't regress on
        // an issue flagged two rounds ago while fixing the latest one (roast
        // RC-H F5 / RC-F). Keep the most recent two rounds of feedback.
        feedback = Some(match feedback.take() {
            Some(prev) => {
                let prev_tail = prev.lines().rev().take(40).collect::<Vec<_>>();
                let prev_tail = prev_tail.into_iter().rev().collect::<Vec<_>>().join("\n");
                format!(
                    "{}\n\n--- earlier feedback (still applies) ---\n{prev_tail}",
                    verifier.feedback
                )
            }
            None => verifier.feedback.clone(),
        });
        round += 1;
    }

    // ── Best-round restore ─────────────────────────────────────────
    // If the final round didn't pass, `git reset --hard` to the BEST round's
    // HEAD before the Submitter phase so the PR ships the strongest revision
    // the fix-loop produced rather than the latest one. "Best" now prefers a
    // passing, security-clean round over a higher-*scoring* but vulnerable one
    // (roast RC-C — see `best_round`). Best-effort: a missing SHA or git
    // failure logs + continues. `submitted` tracks the round whose tree we
    // actually end up submitting so the outcome reporting is honest.
    let final_report = history.last().cloned();
    let mut submitted = final_report.clone();
    if let Some(ref final_r) = final_report {
        if !final_r.pass {
            if let Some(best) = best_round(&history) {
                if best.round != final_r.round {
                    if let Some(sha) = best.head_sha.as_deref() {
                        eprintln!(
                            "  {} {}",
                            theme::BOLT,
                            theme::info(&format!(
                                "best-round restore: round {} (score {}, pass {}, sec_high {}) \
                                 beats final round {} (score {}); resetting to {}",
                                best.round,
                                best.score,
                                best.pass,
                                best.security_high,
                                final_r.round,
                                final_r.score,
                                short_sha(sha),
                            ))
                        );
                        match git_reset_hard(&mission.path, sha) {
                            Ok(()) => submitted = Some(best.clone()),
                            Err(e) => eprintln!(
                                "  {} {}",
                                theme::dim("∘"),
                                theme::dim(&format!("restore failed: {e} — continuing"))
                            ),
                        }
                    }
                }
            }
        }
    }
    let submitted_pass = submitted.as_ref().is_some_and(|r| r.pass);

    // ── Final security gate (roast RC-C) ────────────────────────────
    // If the review is on and HIGH findings survived into the tree we're
    // about to submit, BLOCK the PR. This is a real gate now, not an
    // advisory log line: an unattended (auto-approve) run must not push a
    // confirmed XSS/eval/shell finding with nobody in the loop. Override with
    // CLAUDETTE_FORGE_SECURITY_OVERRIDE=1 when you've reviewed and accept it.
    let mut security_block = false;
    if crate::security_review::enabled() {
        let final_diff = capture_git_diff(&mission.path, base_sha.as_deref()).unwrap_or_default();
        let remaining: Vec<_> = crate::security_review::scan_diff(&final_diff)
            .into_iter()
            .filter(|f| f.severity == crate::security_review::Severity::High)
            .collect();
        if !remaining.is_empty() {
            eprintln!(
                "  {} {}",
                theme::BOLT,
                theme::warn(&format!(
                    "SECURITY: {} HIGH-severity finding(s) remain in the diff after the fix-loop:",
                    remaining.len()
                ))
            );
            for f in &remaining {
                eprintln!("    {} {}", theme::dim("∘"), theme::warn(&f.to_string()));
            }
            if security_override_enabled() {
                eprintln!(
                    "  {} {}",
                    theme::dim("∘"),
                    theme::warn("CLAUDETTE_FORGE_SECURITY_OVERRIDE=1 set — submitting anyway"),
                );
            } else {
                security_block = true;
            }
        }
    }

    // ── Phase 5: Submitter ──────────────────────────────────────────
    // Three guards stand before the PR (roast RC-C / RC-G / RC-A):
    //   1. repo.is_none() — an ephemeral/local mission has no GitHub target;
    //      mission_submit would hard-error. Report the local result honestly
    //      instead of running a turn that silently fails while we claim success.
    //   2. security_block — a surviving HIGH finding (handled above).
    //   3. !submitted_pass — the fix-loop never passed; don't open a PR for
    //      work the gate rejected unless CLAUDETTE_FORGE_SUBMIT_ON_FAIL=1.
    if mission.repo.is_none() {
        eprintln!(
            "  {} {}",
            theme::BOLT,
            theme::info(&format!(
                "forge: changes committed locally at {} (ephemeral/local mission — no GitHub PR \
                 target). Review with `git log`/`git diff`, then push + open a PR manually if you \
                 want one.",
                mission.path.display()
            )),
        );
        if opts.autosave {
            save_session(&session)?;
        }
        cleanup.disarm();
        return Ok(empty_turn_summary());
    }
    if security_block {
        cleanup.disarm();
        return Err(anyhow::anyhow!(
            "forge: refusing to open a PR — HIGH-severity security finding(s) remain in the diff \
             after the fix-loop. Fix them and re-run, or set CLAUDETTE_FORGE_SECURITY_OVERRIDE=1 \
             to submit anyway."
        ));
    }
    if !submitted_pass && !submit_on_fail_enabled() {
        eprintln!(
            "  {} {}",
            theme::BOLT,
            theme::warn(
                "forge: NOT opening a PR — the Verifier never passed within the round limit. \
                 Commits remain on the mission branch for inspection. Re-run to continue, or set \
                 CLAUDETTE_FORGE_SUBMIT_ON_FAIL=1 to open a PR for the best revision anyway."
            ),
        );
        if opts.autosave {
            save_session(&session)?;
        }
        cleanup.disarm();
        return Ok(empty_turn_summary());
    }

    // ── Human-review gate (on by default) ───────────────────────────
    // The user's QA step: by here we KNOW a PR is about to open (brownfield
    // mission, not security-blocked, loop passed or submit-on-fail). Show the
    // plan + the full final diff and require an explicit "y" before the
    // Submitter pushes + opens the PR. Skipped under auto-approve (unattended)
    // or CLAUDETTE_FORGE_NO_REVIEW=1. Fail-closed — a declined or
    // non-interactive answer leaves the commits on the mission branch and opens
    // no PR. Runs after best-round restore so the diff shown is exactly the tree
    // that would ship.
    if forge_human_review_enabled() {
        let review_diff = capture_git_diff(&mission.path, base_sha.as_deref()).unwrap_or_default();
        if !forge_confirm_submit(&plan, &review_diff, submitted_pass) {
            eprintln!(
                "  {} {}",
                theme::BOLT,
                theme::warn(&format!(
                    "forge: PR not opened — change declined at review. Commits remain on the \
                     mission branch in {} for inspection (`git -C {} log` / `git -C {} diff`). \
                     Re-run /forge to continue, or set CLAUDETTE_FORGE_NO_REVIEW=1 to skip the \
                     review gate.",
                    mission.path.display(),
                    mission.path.display(),
                    mission.path.display(),
                )),
            );
            if opts.autosave {
                save_session(&session)?;
            }
            cleanup.disarm();
            return Ok(empty_turn_summary());
        }
    }

    eprintln!("{} {}", theme::BOLT, theme::accent("forge: submit"));
    let mut submit_runtime = build_forge_runtime(session, &mission, true);
    // Tell the Submitter the truth about the loop outcome (roast RC-H F7: the
    // old prompt hard-coded "All quality checks passed" even when they hadn't).
    let submit_input = if submitted_pass {
        "All quality checks passed. Now call mission_submit with a short PR title that \
         summarises the change. Do nothing else."
    } else {
        "The round limit was reached without a full pass, but submission was explicitly \
         requested. Call mission_submit with a short PR title summarising the change, and note \
         in the body that automated review found unresolved issues. Do nothing else."
    };
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

/// Best-effort check that the Planner's brief points at files that actually
/// exist under `mission_path` (roast RC-F F3). The brief is free text, so this
/// is heuristic: it pulls out tokens that look like file paths (a `/` or `\`
/// separator, or a dotted extension) and, if the brief names path-like tokens
/// but *none* of them resolve under the tree, warns that the localization may
/// be wrong. Advisory only — never blocks; false negatives (odd path styles)
/// just mean no warning.
fn warn_if_brief_paths_missing(plan: &str, mission_path: &std::path::Path) {
    let mut candidates: Vec<&str> = Vec::new();
    for raw in plan.split(|c: char| {
        c.is_whitespace() || matches!(c, ',' | ';' | '`' | '"' | '\'' | '(' | ')' | '[' | ']')
    }) {
        let tok = raw.trim_matches(|c: char| matches!(c, ':' | '.' | '-' | '*' | '#'));
        if tok.len() < 3 || tok.len() > 200 {
            continue;
        }
        let looks_path = tok.contains('/')
            || tok.contains('\\')
            || std::path::Path::new(tok)
                .extension()
                .is_some_and(|e| (1..=5).contains(&e.len()));
        if looks_path {
            candidates.push(tok);
        }
    }
    if candidates.is_empty() {
        return;
    }
    let any_exist = candidates.iter().any(|c| {
        let p = std::path::Path::new(c);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            mission_path.join(p)
        };
        abs.exists()
    });
    if !any_exist {
        eprintln!(
            "  {} {}",
            theme::dim("∘"),
            theme::warn(&format!(
                "planner localization check: none of the {} path(s) named in the brief exist \
                 under {} — the localization may be wrong; the Coder has Search tools to \
                 re-localize.",
                candidates.len(),
                mission_path.display()
            )),
        );
    }
}

/// An empty `TurnSummary` for forge exit paths that don't run a final model
/// turn (local/ephemeral mission with no PR target, a blocked submit, or a
/// failed loop we decline to submit). Lets `run_forge_mission` return `Ok`
/// without fabricating a Submitter turn that never happened.
fn empty_turn_summary() -> TurnSummary {
    TurnSummary {
        assistant_messages: Vec::new(),
        tool_results: Vec::new(),
        iterations: 0,
        usage: crate::TokenUsage::default(),
        auto_compaction: None,
        hit_iteration_cap: false,
    }
}

/// RAII guard for the auto-bootstrap path in [`run_forge_mission`]:
/// - clears the active mission slot on Drop iff the mission we installed was
///   ephemeral AND `disarm()` was not called (a mid-pipeline failure can't
///   leave a `/forge`-installed mission active in the REPL);
/// - restores the user's original git branch on Drop, ALWAYS (independent of
///   `disarm`), so a forge run that checked out a dedicated mission branch
///   leaves the user back where they started with the AI commits isolated on
///   the mission branch (roast RC-D MED-2 — "ephemeral" now means cleaned up).
struct EphemeralMissionGuard {
    armed: bool,
    /// `(repo_path, original_branch)` to `git checkout` on Drop. Set by Phase 0
    /// when it creates a dedicated mission branch on the user's live tree.
    restore_branch: Option<(PathBuf, String)>,
}

impl EphemeralMissionGuard {
    fn new(ephemeral: bool) -> Self {
        Self {
            armed: ephemeral,
            restore_branch: None,
        }
    }
    fn set_restore_branch(&mut self, repo: PathBuf, branch: String) {
        self.restore_branch = Some((repo, branch));
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for EphemeralMissionGuard {
    fn drop(&mut self) {
        if let Some((repo, branch)) = self.restore_branch.take() {
            let out = std::process::Command::new("git")
                .args(["checkout", &branch])
                .current_dir(&repo)
                .output();
            match out {
                Ok(o) if o.status.success() => {}
                Ok(o) => eprintln!(
                    "  {} {}",
                    theme::dim("∘"),
                    theme::dim(&format!(
                        "forge: could not restore branch {branch}: {}",
                        String::from_utf8_lossy(&o.stderr).trim()
                    ))
                ),
                Err(e) => eprintln!(
                    "  {} {}",
                    theme::dim("∘"),
                    theme::dim(&format!("forge: could not restore branch {branch}: {e}"))
                ),
            }
        }
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
    // The Planner gets READ-ONLY tools so it can investigate + localize the
    // code to change once for the whole pipeline. Files (read_file, list_dir)
    // and Search (glob_search, grep_search) only — no Git/Advanced/write
    // access, so it cannot edit the tree before the plan exists.
    let mut runtime = build_forge_role_runtime(
        session,
        mission,
        forge::types::Role::Planner,
        forge_planner_system_prompt(&mission.path.to_string_lossy()),
        &[ToolGroup::Files, ToolGroup::Search],
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
    plan: &str,
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
    // Show the Verifier the Planner's grounded brief (relevant files + plan)
    // when one exists, so its grading is informed by the intended localization.
    let brief = plan.trim();
    let brief_block = if brief.is_empty() {
        String::new()
    } else {
        format!("--- Planner brief (relevant files + plan) ---\n{brief}\n--- end brief ---\n\n")
    };
    let payload = format!(
        "Original request: {user_input}\n\n{brief_block}--- git diff HEAD ---\n{diff}\n--- end diff ---"
    );
    let summary = crate::brain_selector::run_turn_with_fallback(&mut runtime, &payload, prompter)
        .map_err(|e| anyhow::anyhow!("verifier turn failed: {e}"))?;
    let text = extract_assistant_text(&summary);
    Ok(parse_verifier_response(&text))
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

/// Minimum Verifier score that can count as a pass. The Verifier prompt
/// states "pass requires score >= 8 AND no bug"; [`parse_verifier_response`]
/// enforces the numeric half in code so a model can't ship a self-declared
/// low-score diff by flipping `pass` to true (roast RC-A HIGH-1).
const VERIFIER_PASS_SCORE: u8 = 8;

/// Parse a Verifier JSON response. Resilient to (a) the model wrapping the
/// JSON in ```code fences, (b) trailing prose after the closing brace, and
/// (c) malformed JSON.
///
/// FAIL-CLOSED (roast RC-A): the Verifier is the only correctness gate before
/// a PR, never runs the code, and is the easiest thing in the pipeline to
/// confuse. Every degenerate path therefore ABSTAINS as a *fail*, not a pass:
/// unparseable / fenced-only / missing-field output → `pass=false, score=0`.
/// A genuinely stuck Verifier then exhausts the bounded fix-loop and exits via
/// the cap rather than green-lighting unverified code. `pass` is additionally
/// reconciled against [`VERIFIER_PASS_SCORE`] so the model can't pass a diff it
/// scored below threshold, and float scores are rounded instead of silently
/// becoming the max.
fn parse_verifier_response(text: &str) -> VerifierResult {
    // Abstention default — fail, with a score of 0 so it can never win
    // best-round restore by masquerading as a clean 10.
    let abstain = VerifierResult {
        score: 0,
        pass: false,
        feedback: "verifier produced no parseable verdict — treated as fail".to_string(),
    };
    let trimmed = text.trim();
    // Match the JSON object — find the first `{` and the last `}`. This also
    // tolerates ```json fences and trailing prose, so no separate fence strip
    // is needed (the brace scan re-locates the object regardless).
    let Some(start) = trimmed.find('{') else {
        return abstain;
    };
    let Some(end) = trimmed.rfind('}') else {
        return abstain;
    };
    if end <= start {
        return abstain;
    }
    let json_slice = &trimmed[start..=end];
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json_slice) else {
        return abstain;
    };
    // Score: accept ints and floats (models love decimals); a missing or
    // non-numeric score is treated as 0, not the max.
    let score = v
        .get("score")
        .and_then(|s| {
            s.as_u64()
                .or_else(|| s.as_f64().map(|f| f.round().max(0.0) as u64))
        })
        .map_or(0, |n| n.min(10) as u8);
    // A missing `pass` field is an abstention → fail (was `unwrap_or(true)`).
    let model_pass = v
        .get("pass")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let feedback = v
        .get("feedback")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    VerifierResult {
        score,
        // Reconcile: a pass requires BOTH the model's verdict AND the score
        // threshold. The score gate is no longer prompt-only theater.
        pass: model_pass && score >= VERIFIER_PASS_SCORE,
        feedback,
    }
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
            "Here is my evaluation:\n{\"score\": 8, \"pass\": true, \"feedback\": \"ok\"}\nDone.",
        );
        assert_eq!(r.score, 8);
        assert!(r.pass);
    }

    #[test]
    fn verifier_pass_requires_score_threshold() {
        // roast RC-A HIGH-1: the model can't ship a self-declared low-score
        // diff by flipping `pass` — a pass requires score >= VERIFIER_PASS_SCORE.
        let r = parse_verifier_response(r#"{"score": 3, "pass": true, "feedback": "fine"}"#);
        assert_eq!(r.score, 3);
        assert!(
            !r.pass,
            "score below threshold must not pass even if model says pass"
        );
    }

    #[test]
    fn verifier_unparseable_fails_closed() {
        // roast RC-A HIGH-2: garbage in → ABSTAIN as a fail (was pass=true/10).
        // A flaky model can't rubber-stamp a broken diff; it exhausts the
        // bounded fix-loop instead.
        let r = parse_verifier_response("I don't know how to format JSON");
        assert!(!r.pass);
        assert_eq!(r.score, 0);
        assert!(!r.feedback.is_empty(), "should explain the abstention");
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
    fn verifier_rounds_float_scores() {
        // roast RC-A MED-5: a float score must round, not silently become 0
        // (or, in the old code, the max). 8.5 → 9, with pass honored.
        let r = parse_verifier_response(r#"{"score": 8.5, "pass": true, "feedback": ""}"#);
        assert_eq!(r.score, 9);
        assert!(r.pass);
    }

    #[test]
    fn verifier_missing_fields_fail_closed() {
        // roast RC-A HIGH-3: only `score` present → missing `pass` is an
        // abstention (fail), not a permissive true.
        let r = parse_verifier_response(r#"{"score": 9}"#);
        assert_eq!(r.score, 9);
        assert!(!r.pass, "missing pass field must not default to pass");
    }

    // ─── Forge best-round restore + smart stopping (Phase 3 of
    //     import_2026_05_19) ──────────────────────────────────────────────

    fn round(n: u32, score: u8, pass: bool, sha: Option<&str>) -> RoundReport {
        RoundReport {
            round: n,
            head_sha: sha.map(str::to_string),
            score,
            pass,
            security_high: false,
        }
    }

    fn round_sec(
        n: u32,
        score: u8,
        pass: bool,
        security_high: bool,
        sha: Option<&str>,
    ) -> RoundReport {
        RoundReport {
            round: n,
            head_sha: sha.map(str::to_string),
            score,
            pass,
            security_high,
        }
    }

    #[test]
    fn best_round_prefers_passing_over_higher_scoring_fail() {
        // roast RC-C: a passing round beats a higher-*scoring* failing one.
        let history = vec![
            round(0, 9, false, Some("aaaa")),
            round(1, 8, true, Some("bbbb")),
        ];
        let best = best_round(&history).unwrap();
        assert_eq!(best.round, 1, "the passing round must win");
    }

    #[test]
    fn best_round_prefers_security_clean_over_higher_scoring_vulnerable() {
        // roast RC-C C1: a high-scoring round with a HIGH finding must NOT be
        // restored over a clean lower-scoring one.
        let history = vec![
            round_sec(0, 9, false, true, Some("aaaa")), // score 9 but HIGH XSS
            round_sec(1, 7, false, false, Some("bbbb")), // clean
        ];
        let best = best_round(&history).unwrap();
        assert_eq!(
            best.round, 1,
            "the security-clean round must win over the vulnerable one"
        );
    }

    #[test]
    fn best_round_picks_highest_score_with_recoverable_sha() {
        let history = vec![
            round(0, 6, false, Some("aaaaaaaaaaaa")),
            round(1, 9, false, Some("bbbbbbbbbbbb")),
            round(2, 7, false, Some("cccccccccccc")),
        ];
        let best = best_round(&history).expect("non-empty history");
        assert_eq!(best.round, 1);
        assert_eq!(best.score, 9);
    }

    #[test]
    fn best_round_breaks_tie_by_earlier_round() {
        // Two rounds at score 8 — keep the earlier one (no churn).
        let history = vec![
            round(0, 8, false, Some("aaaa")),
            round(1, 6, false, Some("bbbb")),
            round(2, 8, false, Some("cccc")),
        ];
        let best = best_round(&history).unwrap();
        assert_eq!(best.round, 0);
    }

    #[test]
    fn best_round_skips_entries_with_no_sha() {
        // The round with the highest score (round 1, score 9) is unrestorable
        // because it has no SHA. Fall back to the next-best with a SHA.
        let history = vec![
            round(0, 7, false, Some("aaaa")),
            round(1, 9, false, None),
            round(2, 6, false, Some("cccc")),
        ];
        let best = best_round(&history).unwrap();
        assert_eq!(best.round, 0);
        assert_eq!(best.score, 7);
    }

    #[test]
    fn best_round_none_on_empty() {
        let history: Vec<RoundReport> = Vec::new();
        assert!(best_round(&history).is_none());
    }

    #[test]
    fn best_round_none_when_all_entries_lack_sha() {
        let history = vec![round(0, 9, false, None), round(1, 8, false, None)];
        assert!(best_round(&history).is_none());
    }

    #[test]
    fn score_declining_returns_false_for_short_history() {
        assert!(!score_declining_two_consecutive(&[]));
        assert!(!score_declining_two_consecutive(&[round(
            0, 9, false, None
        )]));
        assert!(!score_declining_two_consecutive(&[
            round(0, 9, false, None),
            round(1, 5, false, None),
        ]));
    }

    #[test]
    fn score_declining_detects_two_consecutive_drops() {
        let history = vec![
            round(0, 9, false, None),
            round(1, 7, false, None),
            round(2, 5, false, None),
        ];
        assert!(score_declining_two_consecutive(&history));
    }

    #[test]
    fn score_declining_does_not_fire_on_recovery() {
        // drop then recover — not a chain.
        let history = vec![
            round(0, 9, false, None),
            round(1, 6, false, None),
            round(2, 8, false, None),
        ];
        assert!(!score_declining_two_consecutive(&history));
    }

    #[test]
    fn score_declining_requires_strict_inequality() {
        // Flat scores → no decline.
        let history = vec![
            round(0, 7, false, None),
            round(1, 7, false, None),
            round(2, 7, false, None),
        ];
        assert!(!score_declining_two_consecutive(&history));
    }

    #[test]
    fn short_sha_truncates_long_inputs() {
        assert_eq!(short_sha("abcdef0123456789"), "abcdef0");
    }

    #[test]
    fn short_sha_returns_short_inputs_intact() {
        assert_eq!(short_sha("abc"), "abc");
        assert_eq!(short_sha(""), "");
    }

    // ─── Human-review gate (Forge trust) ──────────────────────────────

    #[test]
    fn truncate_diff_for_review_keeps_short_diffs_whole() {
        let d = "line a\nline b\nline c";
        let (shown, omitted) = truncate_diff_for_review(d, REVIEW_DIFF_MAX_LINES);
        assert_eq!(shown, d);
        assert_eq!(omitted, 0);
    }

    #[test]
    fn truncate_diff_for_review_caps_long_diffs() {
        let d = (0..1000)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let (shown, omitted) = truncate_diff_for_review(&d, 600);
        assert_eq!(shown.lines().count(), 600);
        assert_eq!(omitted, 400);
        // The omitted tail must be exactly what wasn't shown.
        assert_eq!(shown.lines().count() + omitted, 1000);
    }

    #[test]
    fn human_review_disabled_under_auto_approve() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev_aa = std::env::var("CLAUDETTE_FORGE_AUTO_APPROVE").ok();
        let prev_nr = std::env::var("CLAUDETTE_FORGE_NO_REVIEW").ok();
        std::env::set_var("CLAUDETTE_FORGE_AUTO_APPROVE", "1");
        std::env::remove_var("CLAUDETTE_FORGE_NO_REVIEW");
        // Auto-approve (unattended) bypasses the human-review gate.
        assert!(!forge_human_review_enabled());
        restore_env("CLAUDETTE_FORGE_AUTO_APPROVE", prev_aa);
        restore_env("CLAUDETTE_FORGE_NO_REVIEW", prev_nr);
    }

    #[test]
    fn human_review_on_by_default_and_opt_out_works() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev_aa = std::env::var("CLAUDETTE_FORGE_AUTO_APPROVE").ok();
        let prev_nr = std::env::var("CLAUDETTE_FORGE_NO_REVIEW").ok();
        std::env::remove_var("CLAUDETTE_FORGE_AUTO_APPROVE");
        std::env::remove_var("CLAUDETTE_FORGE_NO_REVIEW");
        // Attended, no opt-out → gate is ON.
        assert!(forge_human_review_enabled());
        std::env::set_var("CLAUDETTE_FORGE_NO_REVIEW", "1");
        assert!(!forge_human_review_enabled());
        restore_env("CLAUDETTE_FORGE_AUTO_APPROVE", prev_aa);
        restore_env("CLAUDETTE_FORGE_NO_REVIEW", prev_nr);
    }

    // ─── Build + test gate (Forge trust) ──────────────────────────────

    #[test]
    fn build_check_on_by_default_and_opt_out_works() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CLAUDETTE_FORGE_NO_BUILD_CHECK").ok();
        std::env::remove_var("CLAUDETTE_FORGE_NO_BUILD_CHECK");
        assert!(forge_build_check_enabled());
        std::env::set_var("CLAUDETTE_FORGE_NO_BUILD_CHECK", "1");
        assert!(!forge_build_check_enabled());
        restore_env("CLAUDETTE_FORGE_NO_BUILD_CHECK", prev);
    }

    #[test]
    fn test_timeout_defaults_and_clamps() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CLAUDETTE_FORGE_TEST_TIMEOUT_SECS").ok();
        std::env::remove_var("CLAUDETTE_FORGE_TEST_TIMEOUT_SECS");
        assert_eq!(forge_test_timeout_secs(), 180);
        std::env::set_var("CLAUDETTE_FORGE_TEST_TIMEOUT_SECS", "1"); // below floor
        assert_eq!(forge_test_timeout_secs(), 10);
        std::env::set_var("CLAUDETTE_FORGE_TEST_TIMEOUT_SECS", "99999"); // above ceiling
        assert_eq!(forge_test_timeout_secs(), 1800);
        std::env::set_var("CLAUDETTE_FORGE_TEST_TIMEOUT_SECS", "300");
        assert_eq!(forge_test_timeout_secs(), 300);
        std::env::set_var("CLAUDETTE_FORGE_TEST_TIMEOUT_SECS", "garbage"); // unparseable → default
        assert_eq!(forge_test_timeout_secs(), 180);
        restore_env("CLAUDETTE_FORGE_TEST_TIMEOUT_SECS", prev);
    }

    /// Restore (or clear) an env var to its captured prior state.
    fn restore_env(key: &str, prev: Option<String>) {
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
