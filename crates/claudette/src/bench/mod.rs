//! Bench harness — SWE-bench runner + multi-template A/B methodology.
//!
//! Phase 6 of `docs/sprint_import_2026_05_19.md`. Formalizes the round-3
//! e2e methodology ([[project-e2e-sweep-2026-05-16-round3]]) as a
//! reproducible `claudette bench …` surface.
//!
//! ## Subcommands
//!
//! - `claudette bench templates --list` — print the bundled mission
//!   templates that ship with the bench harness.
//! - `claudette bench run --template <name>` — run a single template
//!   through the forge pipeline and write `results.json`.
//! - `claudette bench ab --axis qa|url|determinism --templates N` —
//!   the A/B harness. Runs each template under both conditions of the
//!   chosen axis and emits a comparison.
//! - `claudette bench swe --fixture <path>` — SWE-bench ReAct runner
//!   adapted from `D:\dev\clawForge\crates\forge\src\swebench.rs`.
//!
//! ## Output format
//!
//! Every run emits `bench/runs/<run-id>/results.json` with a stable
//! schema documented in [`BenchResult`]. Designed to be diffable across
//! runs — model name + temperature + seed all captured so the JSON is a
//! reproducibility receipt as much as a results dump.
//!
//! ## What this module does NOT do (yet)
//!
//! - **SWE-bench runner** (`swebench`) is a stub. The full ReAct loop +
//!   7-tool dispatch lifts from clawForge but the corpus loader is
//!   large enough to warrant its own sub-sprint. See `swebench.rs`.
//! - **Antipattern integration** (Phase 7) reads from `bench/runs/`
//!   failures; the schema below preserves the shape that consumer needs.

pub mod ab;
pub mod swebench;
pub mod templates;

use serde::{Deserialize, Serialize};

/// One bench run's full record. Written as `bench/runs/<id>/results.json`
/// at the end of every `bench run` / `bench ab` invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchResult {
    /// `bench-<unix-ts>-<short-uuid>`-style identifier.
    pub run_id: String,
    /// Template name that produced this result (e.g. `csv-analytics`).
    pub template: String,
    /// Which A/B condition this is, if any. `"control"` for the default
    /// run; `"variant"` for the contrasting one.
    pub condition: String,
    /// Model that ran the brain role for this mission.
    pub brain_model: String,
    /// Forge Verifier's final score (`0..=10`).
    pub verifier_score: u8,
    /// Whether the Verifier set `pass=true`.
    pub verifier_pass: bool,
    /// Wall-clock time the full pipeline took, milliseconds.
    pub wall_clock_ms: u64,
    /// Number of fix-loop rounds the forge ran.
    pub rounds: u32,
    /// `true` when the best-round restore (Phase 3) fired during this run.
    pub best_round_restore_fired: bool,
    /// Free-form per-template failure category, captured for the
    /// antipattern detector (Phase 7).
    pub failure_category: Option<String>,
    /// Verifier feedback verbatim — input to similarity matching.
    pub verifier_feedback: String,
}

impl BenchResult {
    /// Empty / failed-to-run sentinel — surfaced when the harness aborted
    /// before producing a real verdict (e.g. mission_start failed).
    #[must_use]
    pub fn errored(template: &str, condition: &str, reason: &str) -> Self {
        Self {
            run_id: format!("bench-error-{}", chrono::Utc::now().timestamp()),
            template: template.to_string(),
            condition: condition.to_string(),
            brain_model: String::new(),
            verifier_score: 0,
            verifier_pass: false,
            wall_clock_ms: 0,
            rounds: 0,
            best_round_restore_fired: false,
            failure_category: Some("harness_error".to_string()),
            verifier_feedback: reason.to_string(),
        }
    }
}

/// Comparison output of an A/B run — two `BenchResult` rows joined by
/// template name with a delta computed from the verifier scores.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbComparison {
    pub template: String,
    pub control: BenchResult,
    pub variant: BenchResult,
    /// `variant.verifier_score - control.verifier_score`. Positive means
    /// the variant did better; negative means the control did.
    pub score_delta: i16,
    /// `variant.wall_clock_ms - control.wall_clock_ms`. Positive means
    /// the variant was slower.
    pub wall_clock_delta_ms: i64,
}

impl AbComparison {
    /// Build a comparison from a `(control, variant)` pair. Both must
    /// share the same `template` field; debug-asserts otherwise.
    #[must_use]
    pub fn new(control: BenchResult, variant: BenchResult) -> Self {
        debug_assert_eq!(
            control.template, variant.template,
            "A/B comparison requires matching templates"
        );
        let score_delta = i16::from(variant.verifier_score) - i16::from(control.verifier_score);
        let wall_clock_delta_ms = i64::try_from(variant.wall_clock_ms)
            .unwrap_or(i64::MAX)
            .saturating_sub(i64::try_from(control.wall_clock_ms).unwrap_or(i64::MAX));
        let template = control.template.clone();
        Self {
            template,
            control,
            variant,
            score_delta,
            wall_clock_delta_ms,
        }
    }

    /// Render a one-line summary for the terminal — `template  Δscore  Δms`.
    #[must_use]
    pub fn summary_line(&self) -> String {
        format!(
            "{:<20}  Δscore={:+}  Δms={:+}",
            self.template, self.score_delta, self.wall_clock_delta_ms
        )
    }
}

/// Resolve the bench-output root. Honors `CLAUDETTE_BENCH_DIR` and
/// falls back to `$HOME/.claudette/bench/runs/`.
#[must_use]
pub fn bench_runs_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CLAUDETTE_BENCH_DIR") {
        if !p.is_empty() {
            return std::path::PathBuf::from(p).join("runs");
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".claudette")
        .join("bench")
        .join("runs")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(template: &str, score: u8, wall_ms: u64) -> BenchResult {
        BenchResult {
            run_id: format!("test-{template}-{score}"),
            template: template.to_string(),
            condition: "control".to_string(),
            brain_model: "qwen3.6-35b-a3b".to_string(),
            verifier_score: score,
            verifier_pass: score >= 8,
            wall_clock_ms: wall_ms,
            rounds: 1,
            best_round_restore_fired: false,
            failure_category: None,
            verifier_feedback: String::new(),
        }
    }

    #[test]
    fn ab_comparison_computes_positive_score_delta() {
        let control = fixture("csv-analytics", 6, 30_000);
        let variant = fixture("csv-analytics", 9, 45_000);
        let cmp = AbComparison::new(control, variant);
        assert_eq!(cmp.score_delta, 3);
        assert_eq!(cmp.wall_clock_delta_ms, 15_000);
    }

    #[test]
    fn ab_comparison_computes_negative_deltas() {
        let control = fixture("dns-parser", 9, 40_000);
        let variant = fixture("dns-parser", 5, 28_000);
        let cmp = AbComparison::new(control, variant);
        assert_eq!(cmp.score_delta, -4);
        assert_eq!(cmp.wall_clock_delta_ms, -12_000);
    }

    #[test]
    fn ab_comparison_summary_includes_template_and_deltas() {
        let cmp = AbComparison::new(
            fixture("storefront", 7, 10_000),
            fixture("storefront", 8, 12_500),
        );
        let line = cmp.summary_line();
        assert!(line.contains("storefront"));
        assert!(line.contains("+1"));
        assert!(line.contains("+2500"));
    }

    #[test]
    fn bench_runs_dir_honors_env_var() {
        let prev = std::env::var("CLAUDETTE_BENCH_DIR").ok();
        std::env::set_var("CLAUDETTE_BENCH_DIR", "/tmp/test-bench");
        let dir = bench_runs_dir();
        assert!(dir.to_string_lossy().contains("test-bench"));
        assert!(dir.ends_with("runs"));
        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_BENCH_DIR", v),
            None => std::env::remove_var("CLAUDETTE_BENCH_DIR"),
        }
    }

    #[test]
    fn bench_result_errored_sentinel_has_failure_category() {
        let r = BenchResult::errored("portfolio", "control", "mission_start failed");
        assert_eq!(r.template, "portfolio");
        assert_eq!(r.condition, "control");
        assert_eq!(r.failure_category.as_deref(), Some("harness_error"));
        assert_eq!(r.verifier_feedback, "mission_start failed");
        assert!(!r.verifier_pass);
    }

    #[test]
    fn bench_result_round_trips_through_json() {
        let r = fixture("rms-scheduler", 8, 22_500);
        let json = serde_json::to_string(&r).unwrap();
        let back: BenchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }
}
