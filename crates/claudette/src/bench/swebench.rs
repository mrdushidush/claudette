//! SWE-bench runner — scaffold for the academic-grade benchmark.
//!
//! The original implementation lives in
//! `D:\dev\clawForge\crates\forge\src\swebench.rs` (725 LOC). Lifting
//! that wholesale needs:
//!
//! 1. A fixture-corpus loader that pulls SWE-bench Lite (~300 issues)
//!    on first run. The corpus is ~80 MB compressed; downloading lazily
//!    keeps the binary small.
//! 2. A ReAct agent loop with the bundled 7-tool kit. Most of these
//!    overlap with claudette's existing forge tool set; the inventory
//!    is in clawForge's `swebench_tools.rs`.
//! 3. An evaluation harness that runs the patched repo's tests in a
//!    sandboxed venv and reports pass/fail per issue.
//!
//! This module ships the **type surface** (`Fixture`, `Issue`,
//! `Outcome`) + a `parse_fixture` JSON loader so callers can wire the
//! corpus path. The actual ReAct loop is the largest single deferred
//! piece — it's purely a code lift from clawForge but the ergonomics
//! (resumability, eval-report writeback) warrant a dedicated sprint
//! rather than a rushed inline reimplementation.

use serde::{Deserialize, Serialize};

/// A single SWE-bench issue, as captured in the upstream JSONL fixture
/// (`princeton-nlp/SWE-bench_Lite`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    /// Stable upstream ID, e.g. `astropy__astropy-7166`.
    pub instance_id: String,
    /// Target repository name (without the `https://github.com/` prefix).
    pub repo: String,
    /// Base commit SHA the patch is built against.
    pub base_commit: String,
    /// The natural-language problem statement (the GitHub issue body).
    pub problem_statement: String,
    /// Test patch that, applied to `base_commit`, exposes the failing
    /// behaviour. The agent's job is to produce a patch that makes
    /// these tests pass.
    pub test_patch: String,
    /// Reference patch (the eventual human fix). Used as the gold
    /// answer for scoring; not shown to the agent.
    pub gold_patch: String,
}

/// One end-of-run outcome for a single issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub instance_id: String,
    pub passed: bool,
    /// Wall-clock the ReAct loop took, milliseconds.
    pub wall_clock_ms: u64,
    /// Number of tool calls the agent made before reporting done.
    pub tool_calls: u32,
    /// Free-form error description when `passed=false` and the failure
    /// wasn't simply "tests still failing" (e.g. patch malformed,
    /// sandbox timeout).
    pub error: Option<String>,
}

/// A SWE-bench fixture — a JSONL file with one [`Issue`] per line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixture {
    pub issues: Vec<Issue>,
}

impl Fixture {
    /// Parse a JSONL string into a fixture. Each non-empty line must be
    /// a serializable [`Issue`]; blank lines are skipped. Used by the
    /// `bench swe` subcommand once the corpus is on disk.
    ///
    /// # Errors
    /// Returns `Err` describing the first malformed line — strict
    /// failure makes it obvious when a download truncated.
    pub fn parse_jsonl(raw: &str) -> Result<Self, String> {
        let mut issues = Vec::new();
        for (i, line) in raw.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let issue: Issue =
                serde_json::from_str(trimmed).map_err(|e| format!("line {}: {e}", i + 1))?;
            issues.push(issue);
        }
        Ok(Self { issues })
    }

    /// Returns the first `n` issues. Useful for `--limit` sampling
    /// during dev — running all 300 SWE-bench Lite issues takes hours.
    #[must_use]
    pub fn take(&self, n: usize) -> Self {
        Self {
            issues: self.issues.iter().take(n).cloned().collect(),
        }
    }

    /// Summarize a vec of outcomes into a one-line stat block.
    /// Format: `passed=NN/TT (XX%) median_ms=MM`.
    #[must_use]
    pub fn summarize(outcomes: &[Outcome]) -> String {
        let total = outcomes.len();
        if total == 0 {
            return "no outcomes".to_string();
        }
        let passed = outcomes.iter().filter(|o| o.passed).count();
        let pct = (passed * 100) / total;
        let mut wall_clock_ms: Vec<u64> = outcomes.iter().map(|o| o.wall_clock_ms).collect();
        wall_clock_ms.sort_unstable();
        let median_ms = wall_clock_ms[wall_clock_ms.len() / 2];
        format!("passed={passed}/{total} ({pct}%) median_ms={median_ms}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_issue(id: &str) -> Issue {
        Issue {
            instance_id: id.to_string(),
            repo: "example/repo".to_string(),
            base_commit: "deadbeef".to_string(),
            problem_statement: "fix the thing".to_string(),
            test_patch: "diff --git a/x b/x\n".to_string(),
            gold_patch: "diff --git a/x b/x\n".to_string(),
        }
    }

    fn outcome(id: &str, passed: bool, ms: u64) -> Outcome {
        Outcome {
            instance_id: id.to_string(),
            passed,
            wall_clock_ms: ms,
            tool_calls: 5,
            error: None,
        }
    }

    #[test]
    fn parse_jsonl_reads_multiple_issues() {
        let issue1 = serde_json::to_string(&sample_issue("a-1")).unwrap();
        let issue2 = serde_json::to_string(&sample_issue("b-2")).unwrap();
        let raw = format!("{issue1}\n{issue2}\n");
        let fx = Fixture::parse_jsonl(&raw).unwrap();
        assert_eq!(fx.issues.len(), 2);
        assert_eq!(fx.issues[0].instance_id, "a-1");
        assert_eq!(fx.issues[1].instance_id, "b-2");
    }

    #[test]
    fn parse_jsonl_skips_blank_lines() {
        let issue1 = serde_json::to_string(&sample_issue("x")).unwrap();
        let raw = format!("\n\n{issue1}\n\n");
        let fx = Fixture::parse_jsonl(&raw).unwrap();
        assert_eq!(fx.issues.len(), 1);
    }

    #[test]
    fn parse_jsonl_errors_on_bad_line_with_index() {
        let issue1 = serde_json::to_string(&sample_issue("x")).unwrap();
        let raw = format!("{issue1}\nnot json\n");
        let err = Fixture::parse_jsonl(&raw).unwrap_err();
        assert!(err.contains("line 2"), "got: {err}");
    }

    #[test]
    fn take_returns_first_n_issues() {
        let mut fx = Fixture { issues: Vec::new() };
        for i in 0..10 {
            fx.issues.push(sample_issue(&format!("id-{i}")));
        }
        let s = fx.take(3);
        assert_eq!(s.issues.len(), 3);
        assert_eq!(s.issues[0].instance_id, "id-0");
        assert_eq!(s.issues[2].instance_id, "id-2");
    }

    #[test]
    fn summarize_reports_pass_rate_and_median() {
        let outcomes = vec![
            outcome("a", true, 1000),
            outcome("b", true, 2000),
            outcome("c", false, 3000),
            outcome("d", true, 4000),
        ];
        let s = Fixture::summarize(&outcomes);
        assert!(s.contains("3/4"));
        assert!(s.contains("75%"));
        assert!(s.contains("3000")); // median of [1000,2000,3000,4000] = 3000
    }

    #[test]
    fn summarize_handles_empty_vec() {
        assert_eq!(Fixture::summarize(&[]), "no outcomes");
    }

    #[test]
    fn outcome_round_trips_through_json() {
        let o = outcome("astropy-7166", true, 12_500);
        let json = serde_json::to_string(&o).unwrap();
        let back: Outcome = serde_json::from_str(&json).unwrap();
        assert_eq!(o, back);
    }
}
