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
}
