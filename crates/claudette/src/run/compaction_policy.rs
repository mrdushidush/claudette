//! Compaction & per-turn iteration policy (Wave C1 — split out of run.rs).
//!
//! The estimated-tokens compaction thresholds, the tiered `pick_compact_plan`
//! decision, and the per-turn `max_iterations` ceiling. The runtime-mutating
//! half (`maybe_compact_session`) stays in `run.rs`; this is the pure policy it
//! consults. Accessors read the environment on each call so a `/preset` or env
//! change takes effect mid-process.

use crate::model_config;

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
/// This is no longer the everyday trigger: [`compact_threshold`] now derives
/// the default from the model's `num_ctx` (half the window) so a real local
/// window compacts before it overflows. `1_000_000` survives as the UPPER
/// CAP on that adaptive value (and the fallback when `num_ctx` is unknown) —
/// a pathologically long session on a huge window still trips it. Users can
/// still pin an exact trigger with `CLAUDETTE_COMPACT_THRESHOLD=12000`.
pub const DEFAULT_COMPACT_THRESHOLD: usize = 1_000_000;

/// Resolve the compaction threshold the REPL is currently using.
///
/// An explicit `CLAUDETTE_COMPACT_THRESHOLD` always wins. Otherwise the
/// threshold is derived from the active brain's context window so a local
/// brain on a real (16K–128K) window actually compacts BEFORE it overflows.
/// (The old fixed 1M default never fired below a megatoken session, so a
/// 32K-window brain hit the context wall and paid a full prompt re-prefill
/// on every subsequent turn.)
///
/// Public so the `get_capabilities` tool and the `/status` slash command can
/// report the same value the REPL actually checks against.
#[must_use]
pub fn compact_threshold() -> usize {
    let env_override = std::env::var("CLAUDETTE_COMPACT_THRESHOLD")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    let num_ctx = model_config::active().brain.num_ctx as usize;
    resolve_compact_threshold(env_override, num_ctx)
}

/// Pure core of [`compact_threshold`]: given an explicit override (if the env
/// var is set and parses) and the active brain's `num_ctx`, resolve the
/// session-token count at which the REPL auto-compacts.
///
/// Half the window — this leaves headroom for the system prompt + tool
/// schemas + the model's reply, none of which `estimate_session_tokens`
/// counts. Floored at `4_000` so a tiny/misconfigured `num_ctx` can't drive
/// the threshold absurdly low, and capped at [`DEFAULT_COMPACT_THRESHOLD`] so
/// an enormous window still bounds total session growth.
///
/// Spiral-safe: the gate metric is `estimate_session_tokens` (which DROPS
/// after a compact, unlike the cumulative-input counter behind the 2026-04-09
/// spiral), and `maybe_compact_session` runs at most once per turn — after a
/// hard compact (4 preserved messages) the session falls well below
/// `num_ctx / 2`, so it does not immediately re-fire.
#[must_use]
fn resolve_compact_threshold(env_override: Option<usize>, num_ctx: usize) -> usize {
    if let Some(v) = env_override {
        return v;
    }
    (num_ctx / 2).clamp(4_000, DEFAULT_COMPACT_THRESHOLD)
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

/// Recent-message preservation count for the hard compaction path. Since the
/// threshold became adaptive (`num_ctx / 2`, PR #92) this is the EVERYDAY
/// trigger, not a 1M last resort — so keep enough recent turns that a small
/// brain doesn't lose track of an in-progress action ("have I pushed / opened
/// the PR yet?") and confabulate completion from the lossy summary.
const HARD_COMPACT_PRESERVE: usize = 12;

/// Recent-message preservation count for the soft (env-var-gated) path. Same
/// count as the hard path now — the soft tier's job is to compact EARLIER (a
/// lower threshold the user opts into), not to preserve more.
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
/// cycles a single user prompt is allowed to drive. On the interactive
/// surfaces (REPL/TUI, which enable `with_graceful_iteration_cap`) hitting
/// the cap ends the turn with a budget-warned, text-only state-of-work
/// summary; sub-agents and forge roles abort with "conversation loop
/// exceeded the maximum number of iterations".
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
    use std::sync::Mutex;

    /// `std::env::set_var` is process-global and races between parallel tests;
    /// the env-var-touching tests below take this lock. (Own copy — the parent
    /// run.rs test module keeps its own.)
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn compact_threshold_default_when_env_var_unset() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CLAUDETTE_COMPACT_THRESHOLD").ok();
        std::env::remove_var("CLAUDETTE_COMPACT_THRESHOLD");

        // No env var → derives from num_ctx (half the window), capped at 1M.
        let result = compact_threshold();
        assert!(result <= DEFAULT_COMPACT_THRESHOLD);
        assert!(result >= 4_000); // floor

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

        // Garbage env → falls back to adaptive (half num_ctx), not 1M.
        let result = compact_threshold();
        assert!(result <= DEFAULT_COMPACT_THRESHOLD);
        assert!(result >= 4_000); // floor

        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_COMPACT_THRESHOLD", v),
            None => std::env::remove_var("CLAUDETTE_COMPACT_THRESHOLD"),
        }
    }

    #[test]
    fn resolve_compact_threshold_honors_explicit_override() {
        // An explicit env override wins regardless of the window size...
        assert_eq!(resolve_compact_threshold(Some(12_345), 32_768), 12_345);
        // ...even a value below the adaptive floor.
        assert_eq!(resolve_compact_threshold(Some(50), 8_192), 50);
    }

    #[test]
    fn resolve_compact_threshold_derives_half_the_window() {
        // No override → half the context window, leaving headroom for the
        // system prompt + tools + reply that estimate_session_tokens omits.
        assert_eq!(resolve_compact_threshold(None, 32_768), 16_384);
        assert_eq!(resolve_compact_threshold(None, 65_536), 32_768);
    }

    #[test]
    fn resolve_compact_threshold_clamps_to_floor_and_ceiling() {
        // A tiny window can't drive the threshold below the 4_000 floor...
        assert_eq!(resolve_compact_threshold(None, 1_000), 4_000);
        // ...and an enormous one can't exceed the 1M legacy ceiling.
        assert_eq!(
            resolve_compact_threshold(None, 8_000_000),
            DEFAULT_COMPACT_THRESHOLD
        );
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
}
