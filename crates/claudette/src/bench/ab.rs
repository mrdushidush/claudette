//! A/B harness — runs each template under two conditions and emits a
//! comparison.
//!
//! Three axes are supported (matches the round-3 sweep's methodology):
//!
//! - **qa** — `control` is the standard forge pipeline (Planner + Coder
//!   plus Verifier); `variant` skips the Verifier loop. Tests whether the
//!   Verifier earns its iteration cost.
//! - **url** — `control` is the standard pipeline; `variant` prepends a
//!   reference URL to the mission prompt as context. Tests whether
//!   external-reference conditioning improves outcomes.
//! - **determinism** — `control` and `variant` are identical runs of
//!   the same template + brain + temperature. Tests pipeline stability
//!   across identical inputs (BCF learning #29).
//!
//! The harness here is the **planning + summary layer**: it picks
//! templates, dispatches them through the forge runner, and joins the
//! results into [`AbComparison`] rows. The actual forge invocation
//! lives in [`crate::run::run_forge_mission`] and is parameterized so
//! the A/B knobs can route different branches.

use serde::{Deserialize, Serialize};

use super::{AbComparison, BenchResult};

/// Which A/B axis to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// Verifier on (control) vs Verifier off (variant).
    Qa,
    /// Plain mission (control) vs mission + reference URL (variant).
    Url,
    /// Two identical runs (control vs variant) — variance only.
    Determinism,
}

impl Axis {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Qa => "qa",
            Self::Url => "url",
            Self::Determinism => "determinism",
        }
    }

    /// Parse a CLI argument (`--axis qa`/`--axis url`/`--axis determinism`).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "qa" | "with-qa" | "without-qa" => Some(Self::Qa),
            "url" | "with-url" | "without-url" => Some(Self::Url),
            "determinism" | "round-1-vs-round-2" | "rerun" => Some(Self::Determinism),
            _ => None,
        }
    }
}

/// Full A/B sweep outcome — one comparison per template, plus an
/// aggregate score-delta + wall-clock-delta. Written to disk as
/// `bench/runs/<id>/ab-<axis>.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbReport {
    pub axis: Axis,
    pub comparisons: Vec<AbComparison>,
    /// Sum of `comparisons[i].score_delta` — positive ⇒ variant wins
    /// overall; negative ⇒ control wins.
    pub aggregate_score_delta: i32,
    /// Sum of wall-clock deltas, milliseconds.
    pub aggregate_wall_clock_delta_ms: i64,
}

impl AbReport {
    /// Roll up a vec of `(control, variant)` pairs into an [`AbReport`].
    /// The pairs must share `template` per element; [`AbComparison::new`]
    /// debug-asserts the invariant.
    #[must_use]
    pub fn new(axis: Axis, pairs: Vec<(BenchResult, BenchResult)>) -> Self {
        let comparisons: Vec<AbComparison> = pairs
            .into_iter()
            .map(|(c, v)| AbComparison::new(c, v))
            .collect();
        let aggregate_score_delta: i32 = comparisons.iter().map(|c| i32::from(c.score_delta)).sum();
        let aggregate_wall_clock_delta_ms: i64 =
            comparisons.iter().map(|c| c.wall_clock_delta_ms).sum();
        Self {
            axis,
            comparisons,
            aggregate_score_delta,
            aggregate_wall_clock_delta_ms,
        }
    }

    /// Decide which side "wins" the sweep. Returns `"variant"` when the
    /// aggregate score delta is positive, `"control"` when negative, and
    /// `"tie"` when zero. Useful for one-line summaries.
    #[must_use]
    pub fn winner(&self) -> &'static str {
        match self.aggregate_score_delta.cmp(&0) {
            std::cmp::Ordering::Greater => "variant",
            std::cmp::Ordering::Less => "control",
            std::cmp::Ordering::Equal => "tie",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn br(template: &str, condition: &str, score: u8, wall_ms: u64) -> BenchResult {
        BenchResult {
            run_id: format!("ab-test-{template}-{condition}"),
            template: template.to_string(),
            condition: condition.to_string(),
            brain_model: "qwen3.6".to_string(),
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
    fn axis_parse_accepts_canonical_names() {
        assert_eq!(Axis::parse("qa"), Some(Axis::Qa));
        assert_eq!(Axis::parse("url"), Some(Axis::Url));
        assert_eq!(Axis::parse("determinism"), Some(Axis::Determinism));
    }

    #[test]
    fn axis_parse_accepts_variant_spellings() {
        assert_eq!(Axis::parse("WITH-QA"), Some(Axis::Qa));
        assert_eq!(Axis::parse("without-url"), Some(Axis::Url));
        assert_eq!(Axis::parse("rerun"), Some(Axis::Determinism));
    }

    #[test]
    fn axis_parse_rejects_unknown() {
        assert!(Axis::parse("bogus").is_none());
        assert!(Axis::parse("").is_none());
    }

    #[test]
    fn report_aggregates_score_deltas() {
        let pairs = vec![
            (br("a", "control", 6, 1000), br("a", "variant", 9, 1500)),
            (br("b", "control", 8, 2000), br("b", "variant", 7, 1800)),
        ];
        let r = AbReport::new(Axis::Qa, pairs);
        assert_eq!(r.aggregate_score_delta, 3 + -1);
        assert_eq!(r.aggregate_wall_clock_delta_ms, 500 + -200);
        assert_eq!(r.winner(), "variant");
    }

    #[test]
    fn report_winner_handles_tie_and_loss() {
        // Tie: deltas sum to 0.
        let tie = AbReport::new(
            Axis::Url,
            vec![(br("a", "control", 6, 0), br("a", "variant", 6, 0))],
        );
        assert_eq!(tie.winner(), "tie");

        // Control wins.
        let loss = AbReport::new(
            Axis::Determinism,
            vec![(br("a", "control", 9, 0), br("a", "variant", 5, 0))],
        );
        assert_eq!(loss.winner(), "control");
    }

    #[test]
    fn report_round_trips_through_json() {
        let r = AbReport::new(
            Axis::Qa,
            vec![(
                br("storefront", "control", 7, 100),
                br("storefront", "variant", 8, 120),
            )],
        );
        let json = serde_json::to_string(&r).unwrap();
        let back: AbReport = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }
}
