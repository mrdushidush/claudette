//! Antipattern auto-detection — closes the godfather "self-evolving
//! few-shots" aspirational loop ([[project-import-sweep-2026-05-19]] §2.6).
//!
//! Phase 7 of `docs/sprint_import_2026_05_19.md`. The closed loop is:
//!
//! 1. **Capture**: when a forge mission fails (Verifier `pass=false` at
//!    the final round), the failure feedback is written as one record
//!    into `~/.claudette/failures/<mission-id>.json`.
//! 2. **Cluster**: on each new failure, compare its feedback text
//!    against the recent corpus. When three or more failures cluster
//!    above a similarity threshold (default 0.55 token-overlap, tuneable
//!    via `CLAUDETTE_ANTIPATTERN_SIM`), the cluster is a candidate.
//! 3. **Graduate**: clusters that haven't yet produced a rule emit one
//!    hard-coded "don't repeat: <pattern>" line into
//!    `~/.claudette/antipatterns/active.toml`. The forge prompt
//!    assembler reads that file and appends the active rules to the
//!    Coder system prompt.
//! 4. **Demote**: rules can be removed manually (`claudette
//!    antipattern demote <id>`) or auto-pruned after they correlate
//!    with N consecutive passes — left as Phase 7b.
//!
//! Similarity uses Jaccard token overlap on lowercased word tokens.
//! Cheap, dependency-free, and good enough for the "did we say this
//! same thing three times?" question. Embedding-grade similarity is
//! Phase 8 territory.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Default Jaccard threshold above which two feedback strings are
/// considered "the same antipattern." Tuneable via the
/// `CLAUDETTE_ANTIPATTERN_SIM` env var — a string like `"0.7"`.
pub const DEFAULT_SIMILARITY_THRESHOLD: f64 = 0.55;

/// Minimum cluster size before a candidate graduates. The godfather
/// brief specified "3 failures @ 70%"; this constant mirrors the count.
pub const GRADUATION_MIN_COUNT: usize = 3;

/// One captured forge failure. Written to disk per
/// `~/.claudette/failures/<mission-id>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureRecord {
    pub mission_id: String,
    /// The Verifier's `feedback` field — the human-readable reason for
    /// the fail.
    pub feedback: String,
    /// Unix timestamp at write time.
    pub recorded_at: i64,
    /// True once this record has contributed to a graduated rule.
    /// Re-graduation is suppressed for already-counted records so a
    /// long history doesn't keep re-firing on the same pattern.
    pub graduated: bool,
}

/// A graduated antipattern rule — written to
/// `~/.claudette/antipatterns/active.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntipatternRule {
    pub id: String,
    /// Short human label drawn from the cluster's shared tokens.
    pub label: String,
    /// Full rule body — surfaced verbatim in the forge Coder system
    /// prompt.
    pub rule: String,
    /// How many failures contributed to this rule's graduation.
    pub seed_count: usize,
    /// Unix timestamp at graduation.
    pub graduated_at: i64,
}

/// Resolve `~/.claudette/failures/`. Honors `CLAUDETTE_FAILURES_DIR` for
/// testing.
#[must_use]
pub fn failures_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CLAUDETTE_FAILURES_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claudette").join("failures")
}

/// Resolve `~/.claudette/antipatterns/active.toml`. Honors
/// `CLAUDETTE_ANTIPATTERNS_FILE` for testing.
#[must_use]
pub fn active_rules_path() -> PathBuf {
    if let Ok(p) = std::env::var("CLAUDETTE_ANTIPATTERNS_FILE") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".claudette")
        .join("antipatterns")
        .join("active.toml")
}

/// Read the `CLAUDETTE_ANTIPATTERN_SIM` env var, falling back to
/// [`DEFAULT_SIMILARITY_THRESHOLD`]. Clamped to `[0.0, 1.0]`.
#[must_use]
pub fn configured_similarity_threshold() -> f64 {
    let raw = std::env::var("CLAUDETTE_ANTIPATTERN_SIM").ok();
    let parsed = raw.and_then(|s| s.trim().parse::<f64>().ok());
    parsed
        .unwrap_or(DEFAULT_SIMILARITY_THRESHOLD)
        .clamp(0.0, 1.0)
}

/// Compute the Jaccard token overlap between two strings, lowercase-
/// folded, split on whitespace + punctuation. Returns 0.0 when either
/// input is token-empty (avoids NaN from `0/0` and treats vacuous
/// inputs as non-matching).
#[must_use]
pub fn jaccard_similarity(a: &str, b: &str) -> f64 {
    fn tokens(s: &str) -> std::collections::HashSet<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty() && t.len() >= 3) // drop stopwords-by-length
            .map(str::to_ascii_lowercase)
            .collect()
    }
    let a_tokens = tokens(a);
    let b_tokens = tokens(b);
    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }
    let intersection = a_tokens.intersection(&b_tokens).count();
    let union = a_tokens.union(&b_tokens).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

/// Cluster `failures` into groups of similar feedback strings using the
/// supplied threshold. Greedy single-linkage clustering: O(n²) but
/// works fine for ≤ a few hundred recent failures. Returns vec-of-vec
/// where each inner vec is one cluster (indices into the input slice).
///
/// Order-stable: failures earlier in the input that anchor a cluster
/// get cluster index 0, and so on. Useful for deterministic graduation
/// choices.
#[must_use]
pub fn cluster_failures(failures: &[FailureRecord], threshold: f64) -> Vec<Vec<usize>> {
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    'outer: for (i, fail) in failures.iter().enumerate() {
        for cluster in &mut clusters {
            let representative = &failures[cluster[0]];
            if jaccard_similarity(&fail.feedback, &representative.feedback) >= threshold {
                cluster.push(i);
                continue 'outer;
            }
        }
        clusters.push(vec![i]);
    }
    clusters
}

/// Decide which clusters are graduation candidates given a minimum
/// cluster size. Filters out clusters where every member already has
/// `graduated=true` (re-graduation suppression).
#[must_use]
pub fn graduation_candidates<'a>(
    failures: &'a [FailureRecord],
    clusters: &'a [Vec<usize>],
    min_count: usize,
) -> Vec<&'a Vec<usize>> {
    clusters
        .iter()
        .filter(|c| c.len() >= min_count && c.iter().any(|&i| !failures[i].graduated))
        .collect()
}

/// Build a label for a cluster — pick the 2-3 most common >=4-char
/// tokens across the cluster's feedback strings. Stable on ties via
/// alphabetic sort so repeated runs produce identical labels.
#[must_use]
pub fn cluster_label(failures: &[FailureRecord], cluster: &[usize]) -> String {
    use std::collections::HashMap;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for &i in cluster {
        for token in failures[i]
            .feedback
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() >= 4)
            .map(str::to_ascii_lowercase)
        {
            *counts.entry(token).or_default() += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(3)
        .map(|(tok, _)| tok)
        .collect::<Vec<_>>()
        .join("-")
}

/// Build the system-prompt overlay block for the currently-active rules.
/// Returns an empty string when no rules are loaded — caller can append
/// unconditionally without an `if !text.is_empty()` guard.
#[must_use]
pub fn rules_prompt_overlay(rules: &[AntipatternRule]) -> String {
    if rules.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\nLearned rules (do not repeat past failures):\n");
    for rule in rules {
        s.push_str("- ");
        s.push_str(&rule.rule);
        s.push('\n');
    }
    s
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn rec(id: &str, feedback: &str, graduated: bool) -> FailureRecord {
        FailureRecord {
            mission_id: id.to_string(),
            feedback: feedback.to_string(),
            recorded_at: 0,
            graduated,
        }
    }

    fn rule(id: &str, label: &str, body: &str) -> AntipatternRule {
        AntipatternRule {
            id: id.to_string(),
            label: label.to_string(),
            rule: body.to_string(),
            seed_count: 3,
            graduated_at: 0,
        }
    }

    // ─── Jaccard ───────────────────────────────────────────────────────

    #[test]
    fn jaccard_identical_strings_match() {
        assert_eq!(jaccard_similarity("hello world", "hello world"), 1.0);
    }

    #[test]
    fn jaccard_disjoint_strings_zero() {
        let s = jaccard_similarity("apple banana cherry", "xylophone yodel zebra");
        assert!(s < 0.05, "got {s}");
    }

    #[test]
    fn jaccard_partial_overlap_between_zero_and_one() {
        // "the parser failed to handle null" vs "the parser crashed on null"
        // share {parser, null}; union has 5+ tokens after stopword filter.
        let s = jaccard_similarity(
            "the parser failed to handle null",
            "the parser crashed on null",
        );
        assert!(s > 0.1 && s < 0.9, "expected partial overlap, got {s}");
    }

    #[test]
    fn jaccard_empty_inputs_return_zero() {
        assert_eq!(jaccard_similarity("", "non-empty"), 0.0);
        assert_eq!(jaccard_similarity("non-empty", ""), 0.0);
        assert_eq!(jaccard_similarity("", ""), 0.0);
    }

    #[test]
    fn jaccard_ignores_short_tokens() {
        // "I am a" tokens after filter (>=3 chars) = {} → zero.
        assert_eq!(jaccard_similarity("I am a", "you too"), 0.0);
    }

    // ─── Threshold env var ─────────────────────────────────────────────

    #[test]
    fn configured_threshold_defaults_when_unset() {
        let _lock = crate::test_env_lock();
        std::env::remove_var("CLAUDETTE_ANTIPATTERN_SIM");
        assert!((configured_similarity_threshold() - DEFAULT_SIMILARITY_THRESHOLD).abs() < 1e-9);
    }

    #[test]
    fn configured_threshold_honors_env() {
        let _lock = crate::test_env_lock();
        std::env::set_var("CLAUDETTE_ANTIPATTERN_SIM", "0.42");
        let t = configured_similarity_threshold();
        std::env::remove_var("CLAUDETTE_ANTIPATTERN_SIM");
        assert!((t - 0.42).abs() < 1e-9, "got {t}");
    }

    #[test]
    fn configured_threshold_clamps_to_unit_interval() {
        let _lock = crate::test_env_lock();
        std::env::set_var("CLAUDETTE_ANTIPATTERN_SIM", "5.0");
        let t = configured_similarity_threshold();
        std::env::remove_var("CLAUDETTE_ANTIPATTERN_SIM");
        assert_eq!(t, 1.0);
    }

    #[test]
    fn configured_threshold_clamps_negative() {
        let _lock = crate::test_env_lock();
        std::env::set_var("CLAUDETTE_ANTIPATTERN_SIM", "-0.5");
        let t = configured_similarity_threshold();
        std::env::remove_var("CLAUDETTE_ANTIPATTERN_SIM");
        assert_eq!(t, 0.0);
    }

    // ─── Clustering ────────────────────────────────────────────────────

    #[test]
    fn clustering_groups_similar_feedback() {
        let failures = vec![
            rec("a", "off-by-one in range; range(n) excludes n", false),
            rec("b", "off-by-one boundary; range(n) excludes n upper", false),
            rec(
                "c",
                "unrelated security finding about token handling",
                false,
            ),
            rec("d", "off-by-one in range boundary excludes n upper", false),
        ];
        let clusters = cluster_failures(&failures, 0.2);
        // Expect at least one cluster containing a, b, d (the range
        // failures) and a separate cluster for c.
        let big = clusters
            .iter()
            .find(|c| c.len() >= 3)
            .expect("should have a cluster of size >= 3");
        assert!(big.contains(&0));
        assert!(big.contains(&1));
        assert!(big.contains(&3));
    }

    #[test]
    fn clustering_is_order_stable_anchor_at_first_occurrence() {
        let failures = vec![
            rec("a", "off-by-one range", false),
            rec("b", "off-by-one range", false),
        ];
        let clusters = cluster_failures(&failures, 0.5);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0], vec![0, 1]);
    }

    #[test]
    fn graduation_candidates_filter_by_min_count() {
        let failures = vec![rec("a", "single failure", false)];
        let clusters = cluster_failures(&failures, 0.5);
        let candidates = graduation_candidates(&failures, &clusters, GRADUATION_MIN_COUNT);
        assert!(candidates.is_empty(), "cluster of 1 should not graduate");
    }

    #[test]
    fn graduation_candidates_skip_already_graduated_clusters() {
        // All three failures already flagged as graduated. The cluster
        // qualifies by size but every member is graduated, so it's
        // suppressed.
        let failures = vec![
            rec("a", "off by one error in range", true),
            rec("b", "off by one error in range", true),
            rec("c", "off by one error in range", true),
        ];
        let clusters = cluster_failures(&failures, 0.5);
        let candidates = graduation_candidates(&failures, &clusters, GRADUATION_MIN_COUNT);
        assert!(candidates.is_empty());
    }

    #[test]
    fn graduation_candidates_kept_when_any_member_ungraduated() {
        let failures = vec![
            rec("a", "off by one error in range", true),
            rec("b", "off by one error in range", true),
            rec("c", "off by one error in range", false),
        ];
        let clusters = cluster_failures(&failures, 0.5);
        let candidates = graduation_candidates(&failures, &clusters, GRADUATION_MIN_COUNT);
        assert_eq!(candidates.len(), 1);
    }

    // ─── Labeling ──────────────────────────────────────────────────────

    #[test]
    fn cluster_label_uses_most_common_tokens() {
        let failures = vec![
            rec("a", "parser crashed on null input handling", false),
            rec("b", "parser crashed when null was passed", false),
            rec("c", "parser handling of null input crashed", false),
        ];
        let label = cluster_label(&failures, &[0, 1, 2]);
        // Should mention "parser" + at least one of "null" / "crashed".
        assert!(label.contains("parser"));
        assert!(label.contains("null") || label.contains("crashed"));
    }

    #[test]
    fn cluster_label_is_deterministic_on_ties() {
        let failures = vec![rec("a", "alpha beta", false), rec("b", "alpha beta", false)];
        let l1 = cluster_label(&failures, &[0, 1]);
        let l2 = cluster_label(&failures, &[0, 1]);
        assert_eq!(l1, l2);
    }

    // ─── Prompt overlay ────────────────────────────────────────────────

    #[test]
    fn rules_overlay_empty_when_no_rules() {
        let overlay = rules_prompt_overlay(&[]);
        assert!(overlay.is_empty());
    }

    #[test]
    fn rules_overlay_lists_each_rule_as_bullet() {
        let rules = vec![
            rule(
                "r1",
                "off-by-one",
                "Always use `range(1, n+1)` for inclusive sums.",
            ),
            rule(
                "r2",
                "sql-injection",
                "Never concatenate user input into SQL.",
            ),
        ];
        let overlay = rules_prompt_overlay(&rules);
        assert!(overlay.contains("Learned rules"));
        assert!(overlay.contains("- Always use `range(1, n+1)`"));
        assert!(overlay.contains("- Never concatenate user input"));
    }

    // ─── Paths ─────────────────────────────────────────────────────────

    #[test]
    fn failures_dir_honors_env() {
        let _lock = crate::test_env_lock();
        let prev = std::env::var("CLAUDETTE_FAILURES_DIR").ok();
        std::env::set_var("CLAUDETTE_FAILURES_DIR", "/tmp/test-failures");
        let d = failures_dir();
        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_FAILURES_DIR", v),
            None => std::env::remove_var("CLAUDETTE_FAILURES_DIR"),
        }
        assert!(d.to_string_lossy().contains("test-failures"));
    }

    #[test]
    fn active_rules_path_honors_env() {
        let _lock = crate::test_env_lock();
        let prev = std::env::var("CLAUDETTE_ANTIPATTERNS_FILE").ok();
        std::env::set_var("CLAUDETTE_ANTIPATTERNS_FILE", "/tmp/test-antipatterns.toml");
        let p = active_rules_path();
        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_ANTIPATTERNS_FILE", v),
            None => std::env::remove_var("CLAUDETTE_ANTIPATTERNS_FILE"),
        }
        assert!(p.to_string_lossy().contains("test-antipatterns"));
    }

    // ─── Serde ─────────────────────────────────────────────────────────

    #[test]
    fn failure_record_round_trips_through_json() {
        let r = rec("m-7", "off-by-one in range", false);
        let json = serde_json::to_string(&r).unwrap();
        let back: FailureRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn rule_round_trips_through_toml() {
        let r = rule("r1", "off-by-one", "use range(1, n+1)");
        let s = toml::to_string(&r).unwrap();
        let back: AntipatternRule = toml::from_str(&s).unwrap();
        assert_eq!(r, back);
    }
}
