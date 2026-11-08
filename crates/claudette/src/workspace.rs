//! The working directory the tools run in.

use std::path::PathBuf;

/// The directory that subprocess tools (`git_*`, `bash`, `run_tests`, ...)
/// run in and that relative paths resolve against: the process cwd, where
/// claudette was launched. Falls back to `.` when the cwd cannot be read
/// (for example, it was deleted from under the process).
#[must_use]
pub fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    #[test]
    fn cwd_is_the_process_cwd() {
        // Other tests move the process cwd while holding this lock.
        let _guard = crate::test_env_lock();
        assert_eq!(super::cwd(), std::env::current_dir().expect("cwd"));
    }
}
