//! Subprocess wrappers for running code validation: syntax checks and unit
//! tests. Sync `try_wait` poll loop for timeouts plus pytest output parsing.
//!
//! All functions are blocking and fire real subprocesses — they're intended to
//! run from within a synchronous tool-dispatch context (inside `run_write_file`
//! or the `/validate` slash command), NOT from async code. The 30-second default
//! timeout prevents runaway test processes from stalling the REPL.
//!
//! Only Python is supported for the Sprint 3 MVP. Adding Rust (`cargo check` /
//! `cargo test --no-run`) or JavaScript (`node --check`) is a future extension
//! point — each gets its own `run_<lang>_*` pair + output parser.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Default timeout for validation subprocesses. Long enough for a realistic
/// test suite on a ~200-line file; short enough not to stall the REPL if a
/// test has an infinite loop.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Raw result from a subprocess invocation.
/// with the addition of `exit_code` for finer diagnostics.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
}

/// Parsed result from running `python -m unittest`.
#[derive(Debug, Clone)]
pub struct TestResult {
    /// True if the process exited 0 and no FAIL/ERROR lines were found.
    pub all_passed: bool,
    pub passed: u32,
    pub failed: u32,
    pub errors: u32,
    /// Combined stdout+stderr when something failed — handed to the coder
    /// model as context for the fix prompt. Empty on success.
    pub error_output: String,
}

// ────────────────────────────────────────────────────────────────────────────
// Subprocess execution
// ────────────────────────────────────────────────────────────────────────────

/// Spawn `program` with `args`, capturing stdout/stderr. Polls
/// `child.try_wait()` every 100 ms and kills the process if the timeout
/// is exceeded. Returns a `CommandResult` in all cases — never panics.
///
/// `cwd` overrides the subprocess working directory — needed by
/// `run_python_unittest` which must `cd` into the file's parent so
/// Python can import it by module name.
pub fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
    cwd: Option<&Path>,
) -> CommandResult {
    let mut cmd = Command::new(program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: format!("failed to spawn `{program}`: {e}"),
                timed_out: false,
                exit_code: None,
            };
        }
    };

    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = read_pipe(child.stdout.take());
                let stderr = read_pipe(child.stderr.take());
                return CommandResult {
                    success: status.success(),
                    stdout,
                    stderr,
                    timed_out: false,
                    exit_code: status.code(),
                };
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return CommandResult {
                        success: false,
                        stdout: String::new(),
                        stderr: format!("timed out after {timeout_secs}s"),
                        timed_out: true,
                        exit_code: None,
                    };
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: format!("try_wait error: {e}"),
                    timed_out: false,
                    exit_code: None,
                };
            }
        }
    }
}

/// Helper: drain a piped stdout/stderr handle into a String. Returns empty
/// string on None (pipe not captured) or read error.
fn read_pipe(pipe: Option<impl Read>) -> String {
    let Some(mut r) = pipe else {
        return String::new();
    };
    let mut buf = String::new();
    let _ = r.read_to_string(&mut buf);
    buf
}

// ────────────────────────────────────────────────────────────────────────────
// Python-specific checks
// ────────────────────────────────────────────────────────────────────────────

/// Run `python -m py_compile <path>`. Returns success if the file is valid
/// Python syntax, failure with the compiler's error message otherwise.
pub fn run_python_syntax_check(path: &Path) -> CommandResult {
    let path_str = path.to_string_lossy();
    run_command_with_timeout(
        "python",
        &["-m", "py_compile", &path_str],
        DEFAULT_TIMEOUT_SECS,
        None,
    )
}

/// Run `python -m unittest <module> -v` with the working directory set to
/// the file's parent directory. Python's unittest needs to IMPORT the file
/// as a module, so we pass the stem name (e.g. `userClass`, not the full
/// path or `userClass.py`) and `cd` into the parent so the import
/// succeeds.
///
/// **Why not `python <path>`?** That works only if the file has a
/// `if __name__ == "__main__": unittest.main()` guard at the bottom. The
/// module-import approach finds all `TestCase` subclasses regardless.
pub fn run_python_unittest(path: &Path) -> TestResult {
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let result = run_command_with_timeout(
        "python",
        &["-m", "unittest", stem, "-v"],
        DEFAULT_TIMEOUT_SECS,
        Some(parent),
    );
    parse_unittest_output(&result)
}

/// Parse `python -m unittest -v` output into a `TestResult`. Counts per-test
/// status lines (`... ok`, `... FAIL`, `... ERROR`) and checks the overall
/// exit code.
///
/// Example verbose output:
/// ```text
/// test_greet (__main__.TestUser.test_greet) ... ok
/// test_get_age (__main__.TestUser.test_get_age) ... FAIL
///
/// ======================================================================
/// FAIL: test_get_age (__main__.TestUser.test_get_age)
/// ...
/// Ran 11 tests in 0.001s
///
/// FAILED (failures=1)
/// ```
pub fn parse_unittest_output(result: &CommandResult) -> TestResult {
    let combined = format!("{}\n{}", result.stdout, result.stderr);
    let mut passed: u32 = 0;
    let mut failed: u32 = 0;
    let mut errors: u32 = 0;

    for line in combined.lines() {
        let trimmed = line.trim_end();
        if trimmed.ends_with("... ok") {
            passed += 1;
        } else if trimmed.ends_with("... FAIL") {
            failed += 1;
        } else if trimmed.ends_with("... ERROR") {
            errors += 1;
        }
    }

    // Fallback: if we didn't see any per-test lines (maybe -v wasn't
    // honoured or the output format differs), use the exit code + summary
    // line. `FAILED (failures=N)` or `FAILED (errors=N)` patterns.
    if passed == 0 && failed == 0 && errors == 0 && !result.success {
        // Parse "FAILED (failures=1, errors=2)" style
        for line in combined.lines() {
            if line.starts_with("FAILED") {
                for segment in line.split(|c: char| !c.is_ascii_digit()) {
                    if let Ok(n) = segment.parse::<u32>() {
                        if n > 0 && failed == 0 {
                            failed = n;
                        }
                    }
                }
                break;
            }
        }
        if failed == 0 {
            failed = 1; // we know SOMETHING failed — exit code was non-zero
        }
    }

    let all_passed = result.success && failed == 0 && errors == 0;
    TestResult {
        all_passed,
        passed,
        failed,
        errors,
        error_output: if all_passed { String::new() } else { combined },
    }
}

/// Check whether a file's content looks like it contains Python unit tests.
/// This is a heuristic, not a parser — good enough for the "should we run
/// `python -m unittest`?" decision. False-positives are fine (we just run
/// unittest and it finds 0 tests). False-negatives are rare because every
/// real test file uses at least one of these patterns.
#[must_use]
pub fn has_python_tests(content: &str) -> bool {
    content.contains("def test_")
        || content.contains("class Test")
        || content.contains("import unittest")
        || content.contains("from unittest")
}

/// Outcome of a Python import pre-flight check. See [`check_python_imports`].
#[derive(Debug, Clone)]
pub struct ImportCheckResult {
    /// Top-level module names referenced by the file that aren't importable.
    /// Empty means either everything is importable or the check itself
    /// couldn't run (see `check_error`).
    pub missing: Vec<String>,
    /// Non-empty when the check itself failed (no `python` on PATH, AST
    /// parse error, etc.). Callers should treat this as "don't know —
    /// fall through to the existing unittest path."
    pub check_error: String,
}

/// Walk the AST of a Python file and try `__import__` on every top-level
/// module it names. Returns modules that raise `ImportError`.
///
/// Used as a pre-flight before `run_python_unittest` so Codet can surface
/// a clean "missing package X" message instead of burning a regen loop on
/// a `unittest.loader._FailedTest` — which looks like a test failure but
/// is actually an environment issue the coder cannot repair.
///
/// Skips relative imports (`from . import x`) and treats non-ImportError
/// exceptions during `__import__` as "proceed anyway" — the goal is to
/// catch the obvious "pip install missing" case, not every possible
/// import-time problem.
pub fn check_python_imports(path: &Path) -> ImportCheckResult {
    const SCRIPT: &str = "\
import sys, ast\n\
if len(sys.argv) < 2:\n\
    sys.exit(0)\n\
try:\n\
    with open(sys.argv[1], 'r', encoding='utf-8') as f:\n\
        tree = ast.parse(f.read())\n\
except Exception:\n\
    sys.exit(0)\n\
missing = []\n\
seen = set()\n\
def try_mod(mod):\n\
    if mod in seen:\n\
        return\n\
    seen.add(mod)\n\
    try:\n\
        __import__(mod)\n\
    except ImportError:\n\
        missing.append(mod)\n\
    except Exception:\n\
        pass\n\
for node in ast.walk(tree):\n\
    if isinstance(node, ast.Import):\n\
        for alias in node.names:\n\
            try_mod(alias.name.split('.')[0])\n\
    elif isinstance(node, ast.ImportFrom):\n\
        if node.level == 0 and node.module:\n\
            try_mod(node.module.split('.')[0])\n\
if missing:\n\
    print(','.join(missing))\n\
    sys.exit(1)\n\
sys.exit(0)\n";

    let path_str = path.to_string_lossy();
    let result = run_command_with_timeout(
        "python",
        &["-c", SCRIPT, &path_str],
        DEFAULT_TIMEOUT_SECS,
        None,
    );

    // Exit 0 — all imports resolve. Exit 1 with stdout — list of misses.
    // Anything else (None exit code from spawn failure, non-zero without
    // stdout, timeout) is "check failed, don't block unittest."
    if result.success {
        return ImportCheckResult {
            missing: Vec::new(),
            check_error: String::new(),
        };
    }
    if result.exit_code == Some(1) && !result.stdout.trim().is_empty() {
        let missing = result
            .stdout
            .trim()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        return ImportCheckResult {
            missing,
            check_error: String::new(),
        };
    }
    ImportCheckResult {
        missing: Vec::new(),
        check_error: if result.timed_out {
            "import check timed out".to_string()
        } else {
            format!("import check failed: {}", result.stderr.trim())
        },
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Rust validation (Sprint 10)
// ────────────────────────────────────────────────────────────────────────────

/// Run `rustc --edition 2021 --crate-type lib <path>` to syntax-check a Rust
/// file without producing a binary. Uses a temp output dir so we don't litter
/// the workspace with `.rlib` files.
pub fn run_rust_syntax_check(path: &Path) -> CommandResult {
    let path_str = path.to_string_lossy();
    let tmp = std::env::temp_dir().join("claudette-rustc");
    let _ = std::fs::create_dir_all(&tmp);
    let out_dir = tmp.to_string_lossy();
    run_command_with_timeout(
        "rustc",
        &[
            "--edition",
            "2021",
            "--crate-type",
            "lib",
            "--out-dir",
            &out_dir,
            &path_str,
        ],
        DEFAULT_TIMEOUT_SECS,
        None,
    )
}

/// Check whether Rust source contains test functions.
#[must_use]
pub fn has_rust_tests(content: &str) -> bool {
    content.contains("#[test]") || content.contains("#[cfg(test)]")
}

// ────────────────────────────────────────────────────────────────────────────
// JavaScript / TypeScript validation (Sprint 10)
// ────────────────────────────────────────────────────────────────────────────

/// Run `node --check <path>` to syntax-check a JavaScript file.
pub fn run_js_syntax_check(path: &Path) -> CommandResult {
    let path_str = path.to_string_lossy();
    run_command_with_timeout("node", &["--check", &path_str], DEFAULT_TIMEOUT_SECS, None)
}

/// Run `npx tsc --noEmit --allowJs --strict <path>` to type-check a TypeScript
/// file. Falls back gracefully if `npx` is not available.
pub fn run_ts_syntax_check(path: &Path) -> CommandResult {
    let path_str = path.to_string_lossy();
    run_command_with_timeout(
        "npx",
        &["tsc", "--noEmit", "--strict", &path_str],
        DEFAULT_TIMEOUT_SECS,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd_result(success: bool, stdout: &str, stderr: &str) -> CommandResult {
        CommandResult {
            success,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            timed_out: false,
            exit_code: if success { Some(0) } else { Some(1) },
        }
    }

    #[test]
    fn parse_unittest_all_pass() {
        let result = cmd_result(
            true,
            "",
            "test_greet (__main__.TestUser.test_greet) ... ok\n\
             test_get_name (__main__.TestUser.test_get_name) ... ok\n\
             test_is_adult (__main__.TestUser.test_is_adult) ... ok\n\
             \n----------------------------------------------------------------------\n\
             Ran 3 tests in 0.001s\n\n\
             OK\n",
        );
        let tr = parse_unittest_output(&result);
        assert!(tr.all_passed);
        assert_eq!(tr.passed, 3);
        assert_eq!(tr.failed, 0);
        assert_eq!(tr.errors, 0);
        assert!(tr.error_output.is_empty());
    }

    #[test]
    fn parse_unittest_one_failure() {
        let result = cmd_result(
            false,
            "",
            "test_greet (__main__.TestUser.test_greet) ... ok\n\
             test_get_age (__main__.TestUser.test_get_age) ... FAIL\n\
             test_is_adult (__main__.TestUser.test_is_adult) ... ok\n\
             \n======================================================================\n\
             FAIL: test_get_age (__main__.TestUser.test_get_age)\n\
             AssertionError: 40 != 4\n\
             \n----------------------------------------------------------------------\n\
             Ran 3 tests in 0.001s\n\n\
             FAILED (failures=1)\n",
        );
        let tr = parse_unittest_output(&result);
        assert!(!tr.all_passed);
        assert_eq!(tr.passed, 2);
        assert_eq!(tr.failed, 1);
        assert_eq!(tr.errors, 0);
        assert!(tr.error_output.contains("FAIL"));
        assert!(tr.error_output.contains("40 != 4"));
    }

    #[test]
    fn parse_unittest_mixed_fail_and_error() {
        let result = cmd_result(
            false,
            "",
            "test_a ... ok\n\
             test_b ... FAIL\n\
             test_c ... ERROR\n\
             test_d ... ok\n\
             \nRan 4 tests in 0.002s\n\n\
             FAILED (failures=1, errors=1)\n",
        );
        let tr = parse_unittest_output(&result);
        assert!(!tr.all_passed);
        assert_eq!(tr.passed, 2);
        assert_eq!(tr.failed, 1);
        assert_eq!(tr.errors, 1);
    }

    #[test]
    fn parse_unittest_empty_output_nonzero_exit() {
        let result = cmd_result(false, "", "");
        let tr = parse_unittest_output(&result);
        assert!(!tr.all_passed);
        assert_eq!(
            tr.failed, 1,
            "should infer at least 1 failure from exit code"
        );
    }

    #[test]
    fn parse_unittest_success_no_verbose_lines() {
        // Some environments don't emit verbose per-test lines.
        let result = cmd_result(true, "", "Ran 5 tests in 0.001s\n\nOK\n");
        let tr = parse_unittest_output(&result);
        assert!(tr.all_passed);
        assert_eq!(tr.passed, 0, "can't count without verbose lines");
        assert_eq!(tr.failed, 0);
    }

    #[test]
    fn has_python_tests_detects_patterns() {
        assert!(has_python_tests("def test_foo(): pass"));
        assert!(has_python_tests("class TestUser(unittest.TestCase):"));
        assert!(has_python_tests("import unittest"));
        assert!(has_python_tests("from unittest import TestCase"));
    }

    #[test]
    fn has_python_tests_returns_false_for_plain_code() {
        assert!(!has_python_tests("def greet(): pass"));
        assert!(!has_python_tests("class User:\n    pass"));
        assert!(!has_python_tests("x = 42"));
    }

    // Import pre-flight.
    // These tests spawn a real `python` process, so they're only meaningful
    // when python is installed. If it isn't, `check_python_imports` returns
    // a non-empty `check_error` and the tests assert only the fallback
    // behavior (empty `missing` list, no crash).

    fn write_temp_py(tag: &str, body: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("claudette-import-check-{tag}.py"));
        std::fs::write(&path, body).expect("write temp file");
        path
    }

    #[test]
    fn check_python_imports_allows_stdlib_only() {
        let path = write_temp_py(
            "stdlib",
            "import os\nimport sys\nfrom pathlib import Path\n",
        );
        let result = check_python_imports(&path);
        let _ = std::fs::remove_file(&path);
        if result.check_error.is_empty() {
            assert!(
                result.missing.is_empty(),
                "stdlib imports should resolve, got missing: {:?}",
                result.missing
            );
        }
    }

    #[test]
    fn check_python_imports_flags_obvious_miss() {
        let path = write_temp_py("miss", "import claudette_definitely_not_a_real_module\n");
        let result = check_python_imports(&path);
        let _ = std::fs::remove_file(&path);
        if result.check_error.is_empty() {
            assert_eq!(
                result.missing,
                vec!["claudette_definitely_not_a_real_module".to_string()],
                "expected the bogus module name, got missing={:?}",
                result.missing,
            );
        }
    }

    #[test]
    fn check_python_imports_skips_relative_imports() {
        // `from . import foo` must not be reported as missing — those are
        // resolved by Python's package system at test time, not by
        // __import__(name).
        let path = write_temp_py("relative", "from . import sibling\nimport os\n");
        let result = check_python_imports(&path);
        let _ = std::fs::remove_file(&path);
        if result.check_error.is_empty() {
            assert!(
                result.missing.is_empty(),
                "relative imports should be skipped, got missing: {:?}",
                result.missing,
            );
        }
    }
}
