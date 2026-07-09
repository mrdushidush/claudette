//! Centralized environment / configuration accessors for the claudette layer
//! (Wave D).
//!
//! Goal: one source of truth for the env vars read across the crate, so call
//! sites can't disagree on a default or a fallback chain. This starts with the
//! home-directory resolver (previously duplicated verbatim in `doctor.rs` and
//! `transcript.rs`); later D PRs fold in the `CLAUDETTE_WORKSPACE` resolver and
//! the multi-site numeric/bool knobs.
//!
//! Named `env_config` (not `config`) because `config` is already the runtime
//! settings-schema module (`runtime/config.rs`); these are unrelated concerns.
//!
//! Accessors read the environment on each call (rather than caching a struct at
//! startup) on purpose: several knobs — notably `CLAUDETTE_WORKSPACE` — are
//! mutated by tests at runtime and re-read mid-process.

use std::path::PathBuf;

/// Canonical home-directory resolver: `USERPROFILE` (Windows) then `HOME`
/// (Unix), falling back to the current directory. Single source of truth so
/// call sites can't disagree on the default.
pub(crate) fn home_dir() -> PathBuf {
    let raw = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(raw)
}

/// Canonical truthy test for a boolean-knob *value*: `1` / `true` / `yes` /
/// `on`, matched **case-insensitively** (`TRUE`, `On`, `YES` all count). Any
/// other value — including `0`, `false`, `""`, or garbage — is false.
///
/// This is the single source of truth for how every `CLAUDETTE_*` boolean knob
/// interprets its value, so call sites can't disagree (some used a bare
/// `== "1"`, some `var_os().is_some()` which was *fail-open*, some the truthy
/// set). Case-insensitivity makes it a strict superset of the strictest prior
/// boolean parsers, so folding a knob in never *narrows* what it accepted.
pub(crate) fn is_truthy(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Whether the named env var is set to a canonical truthy value (see
/// [`is_truthy`]). Fail-SAFE: an unset var, or a var set to any non-truthy
/// value, is false — so a knob that guards a dangerous capability stays
/// guarded unless the user explicitly opts in.
pub(crate) fn is_enabled(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| is_truthy(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_dir_prefers_userprofile_then_home() {
        let _guard = crate::test_env_lock();
        let prev_up = std::env::var("USERPROFILE").ok();
        let prev_home = std::env::var("HOME").ok();

        std::env::set_var("USERPROFILE", "/from/userprofile");
        std::env::set_var("HOME", "/from/home");
        assert_eq!(home_dir(), PathBuf::from("/from/userprofile"));

        std::env::remove_var("USERPROFILE");
        assert_eq!(home_dir(), PathBuf::from("/from/home"));

        std::env::remove_var("HOME");
        assert_eq!(home_dir(), PathBuf::from("."));

        match prev_up {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn is_truthy_accepts_canonical_set_case_insensitively() {
        // No env mutation — pure value test, so no lock / flakiness.
        for v in ["1", "true", "yes", "on", "TRUE", "Yes", "On", "tRuE"] {
            assert!(is_truthy(v), "'{v}' should be truthy");
        }
    }

    #[test]
    fn is_truthy_rejects_falsey_and_garbage() {
        for v in [
            "0", "false", "no", "off", "", " ", "2", "enable", "1 ", "yeah", "FALSE",
        ] {
            assert!(!is_truthy(v), "'{v}' should not be truthy");
        }
    }

    #[test]
    fn is_enabled_reads_env_fail_safe() {
        let _guard = crate::test_env_lock();
        // Deliberately NOT a `CLAUDETTE_*` name: the `every_env_var_is_documented`
        // guard scans src/ for that prefix, and this synthetic probe isn't a
        // real, user-facing knob.
        let key = "ENVCFG_IS_ENABLED_PROBE_XYZZY";
        let prev = std::env::var(key).ok();

        std::env::remove_var(key);
        assert!(!is_enabled(key), "unset var must be fail-safe (false)");

        std::env::set_var(key, "TRUE");
        assert!(is_enabled(key), "case-insensitive truthy must enable");

        std::env::set_var(key, "0");
        assert!(!is_enabled(key), "'0' must not enable");

        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
