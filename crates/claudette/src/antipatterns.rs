//! Antipattern prompt overlay — injects "don't repeat past failures" rules
//! into the forge Coder system prompt.
//!
//! Rules live in `~/.claudette/antipatterns/active.toml` (honors
//! `CLAUDETTE_ANTIPATTERNS_FILE` for testing) as a `[[rules]]` array. On
//! each forge run [`load_active_rules`] reads the file — best-effort: a
//! missing or malformed file yields no rules and never blocks the
//! pipeline — and [`rules_prompt_overlay`] renders the active rules as a
//! bullet list appended to the Coder prompt.
//!
//! The original import-sweep design (Phase 7 of
//! `docs/sprint_import_2026_05_19.md`) also sketched an automatic
//! capture → cluster → graduate loop that would mine failed missions into
//! new rules. That write half was never wired into the runtime and was
//! removed in the 2026-06 dead-code cleanup; this file is the read side —
//! author rules by hand or via external tooling and they take effect on
//! the next forge run.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// An antipattern rule — read from `~/.claudette/antipatterns/active.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntipatternRule {
    pub id: String,
    /// Short human label for the rule.
    pub label: String,
    /// Full rule body — surfaced verbatim in the forge Coder system prompt.
    pub rule: String,
    /// How many failures contributed to this rule (provenance metadata).
    pub seed_count: usize,
    /// Unix timestamp at authoring.
    pub graduated_at: i64,
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

/// On-disk shape of `~/.claudette/antipatterns/active.toml`. The wrapper
/// gives us a `[[rules]]` array of tables, which is the natural TOML
/// representation for "a list of antipattern rules."
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActiveRulesFile {
    #[serde(default)]
    pub rules: Vec<AntipatternRule>,
}

/// Read the active rules file from disk. Returns an empty vector when the
/// file is missing, unreadable, or malformed — antipatterns are a
/// best-effort prompt overlay and must never block the forge pipeline.
#[must_use]
pub fn load_active_rules() -> Vec<AntipatternRule> {
    let path = active_rules_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    match toml::from_str::<ActiveRulesFile>(&text) {
        Ok(file) => file.rules,
        Err(_) => Vec::new(),
    }
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
mod tests {
    use super::*;

    fn rule(id: &str, label: &str, body: &str) -> AntipatternRule {
        AntipatternRule {
            id: id.to_string(),
            label: label.to_string(),
            rule: body.to_string(),
            seed_count: 3,
            graduated_at: 0,
        }
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
    fn rule_round_trips_through_toml() {
        let r = rule("r1", "off-by-one", "use range(1, n+1)");
        let s = toml::to_string(&r).unwrap();
        let back: AntipatternRule = toml::from_str(&s).unwrap();
        assert_eq!(r, back);
    }
}
