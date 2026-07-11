//! Post-edit check — run a fast syntax/type check after a successful write-class tool call.
//!
//! After a write-class tool call succeeds, `run_post_edit_check` runs a fast
//! auto-detected syntax or type check against the changed file and returns
//! truncated failure output (`None` on success/skip). Opt-in via
//! `CLAUDETTE_POST_EDIT_CHECK`. No-op under offline mode because check
//! toolchains execute arbitrary project code (same rule as `run_tests`; roast
//! 2026-06-30 H1).

use std::path::{Path, PathBuf};

/// Environment variable that enables post-edit checks.
pub(crate) const CHECK_ENV: &str = "CLAUDETTE_POST_EDIT_CHECK";

/// Environment variable for a custom check command override.
pub(crate) const CMD_ENV: &str = "CLAUDETTE_CHECK_CMD";

/// Environment variable for the timeout in seconds (clamped to 1..=120).
pub(crate) const TIMEOUT_ENV: &str = "CLAUDETTE_CHECK_TIMEOUT_SECS";

/// Environment variable for the max fix rounds per file per turn (clamped to 1..=10).
pub(crate) const MAX_ROUNDS_ENV: &str = "CLAUDETTE_CHECK_MAX_ROUNDS";

/// Maximum number of output lines to retain.
pub(crate) const MAX_OUTPUT_LINES: usize = 30;

/// Maximum number of output characters to retain (after line cap).
pub(crate) const MAX_OUTPUT_CHARS: usize = 2000;

/// A check command with its program, arguments, and working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckCmd {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// Returns `true` when post-edit checks are enabled.
///
/// Truthy values (case-insensitive, trimmed): `"1"`, `"true"`, `"yes"`, `"on"`.
/// Unset or anything else → `false`. Default is OFF.
pub(crate) fn enabled() -> bool {
    std::env::var(CHECK_ENV)
        .ok()
        .is_some_and(|v| matches!(v.to_ascii_lowercase().trim(), "1" | "true" | "yes" | "on"))
}

/// Returns the configured timeout in seconds, clamped to `1..=120`.
///
/// Default is `10` when unset or unparseable.
pub(crate) fn timeout_secs() -> u64 {
    std::env::var(TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(10)
        .clamp(1, 120)
}

/// Per-file per-turn cap on appended check-failure output.
///
/// Parses `MAX_ROUNDS_ENV`, clamps to `1..=10`, defaults to `2` when unset or
/// unparseable. A stubborn error can't feed an edit↔check spiral beyond this
/// count (design 2026-07-11, W4).
pub(crate) fn max_rounds() -> u32 {
    std::env::var(MAX_ROUNDS_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(2)
        .clamp(1, 10)
}

/// Suppressed-notice body when the per-file round cap is exceeded.
///
/// Returned verbatim — no trailing newline (the caller appends its own).
pub(crate) fn suppressed_notice(path: &str) -> String {
    format!(
        "\n\n[post_edit_check] {path} still fails its check \
         (output suppressed after repeated rounds this turn — run run_tests or diagnostics for the full picture)"
    )
}

/// Parse a raw command string into a `CheckCmd`.
///
/// Split on whitespace. First token = program, rest = args. Every arg
/// containing the literal `{file}` gets it replaced by `file.display()`.
/// If NO token contained `{file}`, append `file.display()` as one extra final arg.
/// Returns `None` when `raw` is empty after trimming.
pub(crate) fn override_cmd(raw: &str, file: &Path, workspace: &Path) -> Option<CheckCmd> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    let program = tokens[0].to_string();
    let file_display = file.display().to_string();

    let mut args: Vec<String> = Vec::new();
    let mut had_placeholder = false;

    for token in &tokens[1..] {
        if token.contains("{file}") {
            let replaced = token.replace("{file}", &file_display);
            args.push(replaced);
            had_placeholder = true;
        } else {
            args.push(token.to_string());
        }
    }

    if !had_placeholder {
        args.push(file_display);
    }

    Some(CheckCmd {
        program,
        args,
        cwd: workspace.to_path_buf(),
    })
}

/// Auto-detect a check command based on the file's extension.
///
/// Matches (lowercased): `.rs` → cargo check; `.py` → ruff or py_compile;
/// `.go` → go vet; `.js`/`.mjs`/`.cjs` → node --check. Everything else
/// (including `.ts`, `.tsx`) returns `None`.
pub(crate) fn builtin_cmd(file: &Path, workspace: &Path, ruff_available: bool) -> Option<CheckCmd> {
    let ext = file.extension()?.to_ascii_lowercase();

    match ext.to_str()? {
        "rs" => Some(CheckCmd {
            program: "cargo".to_string(),
            args: vec!["check".to_string(), "--message-format=short".to_string()],
            cwd: workspace.to_path_buf(),
        }),

        "py" => {
            if ruff_available {
                Some(CheckCmd {
                    program: "ruff".to_string(),
                    args: vec!["check".to_string(), file.display().to_string()],
                    cwd: workspace.to_path_buf(),
                })
            } else {
                Some(CheckCmd {
                    program: "python".to_string(),
                    args: vec![
                        "-m".to_string(),
                        "py_compile".to_string(),
                        file.display().to_string(),
                    ],
                    cwd: workspace.to_path_buf(),
                })
            }
        }

        "go" => {
            let cwd = file.parent().map_or(workspace.to_path_buf(), |p| {
                if p.as_os_str().is_empty() {
                    workspace.to_path_buf()
                } else {
                    p.to_path_buf()
                }
            });
            Some(CheckCmd {
                program: "go".to_string(),
                args: vec!["vet".to_string(), ".".to_string()],
                cwd,
            })
        }

        "js" | "mjs" | "cjs" => Some(CheckCmd {
            program: "node".to_string(),
            args: vec!["--check".to_string(), file.display().to_string()],
            cwd: workspace.to_path_buf(),
        }),

        _ => None,
    }
}

/// Returns `true` if the `ruff` binary is on PATH and exits successfully.
pub(crate) fn ruff_on_path() -> bool {
    std::process::Command::new("ruff")
        .arg("--version")
        .output()
        .ok()
        .is_some_and(|out| out.status.success())
}

/// Truncate raw output to the first `MAX_OUTPUT_LINES` lines, then cut at
/// `MAX_OUTPUT_CHARS`. Appends a marker if anything was dropped.
pub(crate) fn truncate_output(raw: &str) -> String {
    let result = raw.lines().take(MAX_OUTPUT_LINES).collect::<Vec<_>>();
    let any_lines_dropped = raw.lines().count() > MAX_OUTPUT_LINES;

    let joined = result.join("\n");

    if !any_lines_dropped && joined.len() <= MAX_OUTPUT_CHARS {
        return joined;
    }

    // Cut at char boundary.
    let truncated: String = joined.chars().take(MAX_OUTPUT_CHARS).collect();
    let any_chars_dropped = truncated.len() < joined.len();

    if !any_lines_dropped && !any_chars_dropped {
        return truncated;
    }

    let mut out = truncated;
    use std::fmt::Write as _;
    let _ = write!(out, "\n… (check output truncated)");

    out
}

/// Determine the check command to run for a file.
///
/// If `CMD_ENV` is set and non-empty after trimming → `override_cmd`.
/// Otherwise → `builtin_cmd(file, workspace, ruff_on_path())`.
pub(crate) fn command_for(file: &Path, workspace: &Path) -> Option<CheckCmd> {
    let raw = std::env::var(CMD_ENV).ok().unwrap_or_default();
    if !raw.trim().is_empty() {
        return override_cmd(&raw, file, workspace);
    }
    builtin_cmd(file, workspace, ruff_on_path())
}

/// Run a post-edit check on `file` in `workspace`.
///
/// Returns `None` unless enabled; also returns `None` under offline mode or
/// when no command can be determined. On success (exit 0) or timeout → `None`.
/// Otherwise returns truncated failure output.
pub(crate) fn run_post_edit_check(file: &Path, workspace: &Path) -> Option<String> {
    if !enabled() {
        return None;
    }

    if crate::egress::is_offline() {
        return None;
    }

    let cmd = command_for(file, workspace)?;

    let args: Vec<&str> = cmd.args.iter().map(String::as_str).collect();
    let result = crate::test_runner::run_command_with_timeout(
        &cmd.program,
        &args,
        timeout_secs(),
        Some(&cmd.cwd),
    );

    if result.timed_out {
        return None;
    }

    if result.success {
        return None;
    }

    let output = format!("{}\n{}", result.stdout, result.stderr);
    Some(truncate_output(&output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Every test that touches env vars must hold this lock and restore/remove
    /// the vars before dropping the guard.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn set_env(key: &str, val: &str) {
        std::env::set_var(key, val);
    }

    fn unset_env(key: &str) {
        std::env::remove_var(key);
    }

    #[test]
    fn enabled_defaults_off_and_parses_truthy() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Unset → false.
        unset_env(CHECK_ENV);
        assert!(!enabled());

        // Truthy values.
        set_env(CHECK_ENV, "1");
        assert!(enabled());
        set_env(CHECK_ENV, "true");
        assert!(enabled());
        set_env(CHECK_ENV, "YES");
        assert!(enabled());
        set_env(CHECK_ENV, "on");
        assert!(enabled());

        // Falsy values.
        set_env(CHECK_ENV, "0");
        assert!(!enabled());
        set_env(CHECK_ENV, "off");
        assert!(!enabled());
        set_env(CHECK_ENV, "");
        assert!(!enabled());

        unset_env(CHECK_ENV);
    }

    #[test]
    fn timeout_defaults_and_clamps() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Unset → 10.
        unset_env(TIMEOUT_ENV);
        assert_eq!(timeout_secs(), 10);

        set_env(TIMEOUT_ENV, "5");
        assert_eq!(timeout_secs(), 5);

        set_env(TIMEOUT_ENV, "0");
        assert_eq!(timeout_secs(), 1); // clamped to min.

        set_env(TIMEOUT_ENV, "999");
        assert_eq!(timeout_secs(), 120); // clamped to max.

        set_env(TIMEOUT_ENV, "garbage");
        assert_eq!(timeout_secs(), 10); // fallback default.

        unset_env(TIMEOUT_ENV);
    }

    #[test]
    fn builtin_cmd_rust_maps_to_cargo_check() {
        let ws = PathBuf::from("/workspace");
        let cmd = builtin_cmd(Path::new("src/main.rs"), &ws, false).unwrap();
        assert_eq!(cmd.program, "cargo");
        assert_eq!(cmd.args, vec!["check", "--message-format=short"]);
        assert_eq!(cmd.cwd, ws);
    }

    #[test]
    fn builtin_cmd_python_prefers_ruff_falls_back_py_compile() {
        let ws = PathBuf::from("/workspace");
        let file = Path::new("app.py");

        // ruff available → uses ruff.
        let cmd = builtin_cmd(file, &ws, true).unwrap();
        assert_eq!(cmd.program, "ruff");
        assert_eq!(cmd.args, vec!["check", "app.py"]);

        // ruff unavailable → falls back to py_compile.
        let cmd = builtin_cmd(file, &ws, false).unwrap();
        assert_eq!(cmd.program, "python");
        assert_eq!(cmd.args, vec!["-m", "py_compile", "app.py"]);
    }

    #[test]
    fn builtin_cmd_go_targets_package_dir() {
        let ws = PathBuf::from("/workspace");
        // File in a subdirectory → cwd is the file's parent.
        let cmd = builtin_cmd(Path::new("cmd/server/main.go"), &ws, false).unwrap();
        assert_eq!(cmd.program, "go");
        assert_eq!(cmd.args, vec!["vet", "."]);
        assert_eq!(cmd.cwd, PathBuf::from("cmd/server"));

        // File at workspace root → falls back to workspace.
        let cmd = builtin_cmd(Path::new("main.go"), &ws, false).unwrap();
        assert_eq!(cmd.cwd, ws);
    }

    #[test]
    fn builtin_cmd_js_node_check() {
        let ws = PathBuf::from("/workspace");
        for ext in ["js", "mjs", "cjs"] {
            let file = PathBuf::from(format!("index.{ext}"));
            let cmd = builtin_cmd(&file, &ws, false).unwrap();
            assert_eq!(cmd.program, "node");
            assert_eq!(
                cmd.args,
                vec!["--check".to_string(), file.display().to_string()]
            );
        }
    }

    #[test]
    fn builtin_cmd_unknown_ext_is_none() {
        let ws = PathBuf::from("/workspace");
        for ext in ["ts", "tsx", "md", "toml"] {
            let file = PathBuf::from(format!("file.{ext}"));
            assert!(builtin_cmd(&file, &ws, false).is_none());
        }
        // Extensionless path → None.
        assert!(builtin_cmd(Path::new("Makefile"), &ws, false).is_none());
    }

    #[test]
    fn override_cmd_substitutes_file_placeholder() {
        let ws = PathBuf::from("/workspace");
        let file = PathBuf::from("/project/src/main.rs");
        let cmd = override_cmd("ruff check {file}", &file, &ws).unwrap();
        assert_eq!(cmd.program, "ruff");
        assert_eq!(cmd.args, vec!["check", "/project/src/main.rs"]);
    }

    #[test]
    fn override_cmd_appends_file_when_no_placeholder() {
        let ws = PathBuf::from("/workspace");
        let file = PathBuf::from("app.py");
        let cmd = override_cmd("ruff check", &file, &ws).unwrap();
        assert_eq!(cmd.program, "ruff");
        assert_eq!(cmd.args, vec!["check", "app.py"]);
    }

    #[test]
    fn override_cmd_empty_is_none() {
        let ws = PathBuf::from("/workspace");
        assert!(override_cmd("", Path::new("x.rs"), &ws).is_none());
        assert!(override_cmd("  ", Path::new("x.rs"), &ws).is_none());
    }

    #[test]
    fn truncate_output_passthrough_when_small() {
        let input = "line1\nline2\nline3";
        assert_eq!(truncate_output(input), "line1\nline2\nline3");
    }

    #[test]
    fn truncate_output_caps_lines_then_chars() {
        // 40 one-char lines → should be capped at 30 lines + marker.
        let input: String = (0..40)
            .map(|i| format!("x{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = truncate_output(&input);
        assert!(result.lines().count() <= MAX_OUTPUT_LINES + 1);
        assert!(result.contains("check output truncated"));

        // A single 5000-char line → ≤ MAX_OUTPUT_CHARS + marker.
        let long_line: String = "a".repeat(5000);
        let result = truncate_output(&long_line);
        assert!(result.len() <= MAX_OUTPUT_CHARS + "\n… (check output truncated)".len());
    }

    #[test]
    fn run_post_edit_check_disabled_returns_none() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Ensure CHECK_ENV is unset so enabled() → false.
        unset_env(CHECK_ENV);
        // Point CMD_ENV at a nonexistent program — if the function were to
        // actually try running it, we'd get an error (not None).
        set_env(CMD_ENV, "/nonexistent_program_xyz");

        let result = run_post_edit_check(Path::new("test.rs"), Path::new("/ws"));
        assert!(
            result.is_none(),
            "should return None when disabled; got {:?}",
            result
        );

        unset_env(CMD_ENV);
    }

    #[test]
    fn run_post_edit_check_skips_under_offline() {
        let _lock = ENV_LOCK.lock().unwrap();
        set_env(CHECK_ENV, "1");
        // Simulate offline mode.
        std::env::set_var(crate::egress::OFFLINE_ENV, "1");

        let result = run_post_edit_check(Path::new("test.rs"), Path::new("/ws"));
        assert!(
            result.is_none(),
            "should return None under offline; got {:?}",
            result
        );

        unset_env(CHECK_ENV);
        std::env::remove_var(crate::egress::OFFLINE_ENV);
    }

    #[test]
    fn max_rounds_defaults_and_clamps() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Unset → 2.
        unset_env(MAX_ROUNDS_ENV);
        assert_eq!(max_rounds(), 2);

        set_env(MAX_ROUNDS_ENV, "1");
        assert_eq!(max_rounds(), 1);

        set_env(MAX_ROUNDS_ENV, "0");
        assert_eq!(max_rounds(), 1); // clamped to min.

        set_env(MAX_ROUNDS_ENV, "99");
        assert_eq!(max_rounds(), 10); // clamped to max.

        set_env(MAX_ROUNDS_ENV, "garbage");
        assert_eq!(max_rounds(), 2); // fallback default.

        unset_env(MAX_ROUNDS_ENV);
    }

    #[test]
    fn suppressed_notice_names_the_file() {
        let path = "src/main.rs";
        let notice = suppressed_notice(path);
        assert!(notice.contains("[post_edit_check]"));
        assert!(notice.contains("still fails its check"));
        assert!(notice.contains("output suppressed after repeated rounds this turn — run run_tests or diagnostics for the full picture"));
        assert!(notice.contains("src/main.rs"));
    }
}
