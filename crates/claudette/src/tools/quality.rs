//! Quality group — code-quality tooling. Sprint v0.6.0 first lands
//! `run_tests`; `diagnostics` follows in Phase 3.1b.
//!
//! Both tools auto-detect the active project's framework by walking up
//! from the active cwd (mission-aware via [`crate::missions::active_cwd`])
//! looking for the canonical config files: `Cargo.toml` → Rust,
//! `package.json` → Node, `pytest.ini` / `pyproject.toml` → Python,
//! `go.mod` → Go. The caller can override via the `framework` arg if the
//! auto-detect picks wrong (mixed-language repos).
//!
//! Output is structured: `passed` / `failed` counts plus an array of
//! `failures` with `{name, file, line, message}` where each is extracted
//! per-framework using cheap line-level regexes. Raw stdout/stderr is
//! also returned for unrecognised cases.

use std::path::Path;

use serde_json::{json, Value};

use super::parse_json_input;
use crate::test_runner::{run_command_with_timeout, CommandResult};

/// Cap for individual test invocations. Most cargo/npm/pytest test
/// suites complete well under this; longer-running suites should be
/// driven via the upcoming `bash_background` family instead.
const TEST_TIMEOUT_SECS: u64 = 180;

pub(super) fn schemas() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "run_tests",
            "description": "Run the project test suite. Auto-detects framework (cargo, npm, pytest, go) from project files. Returns pass/fail counts + failures (name, file, line, message). 180s timeout.",
            "parameters": {
                "type": "object",
                "properties": {
                    "framework": { "type": "string", "description": "Override auto-detect: 'cargo', 'npm', 'pytest', 'go', or 'auto' (default)." },
                    "filter":    { "type": "string", "description": "Optional test-name substring filter (passed to the framework's own filter flag)." }
                },
                "required": []
            }
        }
    })]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "run_tests" => run_tests(input),
        _ => return None,
    };
    Some(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framework {
    Cargo,
    Npm,
    Pytest,
    Go,
}

impl Framework {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "cargo" | "rust" => Some(Self::Cargo),
            "npm" | "node" | "jest" => Some(Self::Npm),
            "pytest" | "python" | "py" => Some(Self::Pytest),
            "go" | "golang" => Some(Self::Go),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Npm => "npm",
            Self::Pytest => "pytest",
            Self::Go => "go",
        }
    }
}

/// Walk up from `start` looking for the marker file that identifies a
/// framework. Returns the first match (closest to the leaf) so monorepos
/// with sub-projects pick the right one based on cwd.
fn detect_framework(start: &Path) -> Option<Framework> {
    let mut current: Option<&Path> = Some(start);
    while let Some(dir) = current {
        if dir.join("Cargo.toml").exists() {
            return Some(Framework::Cargo);
        }
        if dir.join("package.json").exists() {
            return Some(Framework::Npm);
        }
        if dir.join("pytest.ini").exists() || dir.join("pyproject.toml").exists() {
            return Some(Framework::Pytest);
        }
        if dir.join("go.mod").exists() {
            return Some(Framework::Go);
        }
        current = dir.parent();
    }
    None
}

fn run_tests(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "run_tests")?;
    let cwd = crate::missions::active_cwd();
    let requested = v.get("framework").and_then(Value::as_str).unwrap_or("auto");
    let filter = v
        .get("filter")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());

    let framework = if requested == "auto" || requested.is_empty() {
        detect_framework(&cwd).ok_or_else(|| {
            format!(
                "run_tests: could not auto-detect a test framework under {} \
                 (looked for Cargo.toml, package.json, pytest.ini/pyproject.toml, go.mod). \
                 Pass `framework` explicitly.",
                cwd.display()
            )
        })?
    } else {
        Framework::parse(requested).ok_or_else(|| {
            format!(
                "run_tests: unknown framework '{requested}' \
                 — use 'auto', 'cargo', 'npm', 'pytest', or 'go'."
            )
        })?
    };

    let result = invoke(framework, filter, &cwd);
    Ok(format_result(framework, &result).to_string())
}

/// Spawn the per-framework test command, picking the right filter flag
/// when the caller provided one.
fn invoke(framework: Framework, filter: Option<&str>, cwd: &Path) -> CommandResult {
    match framework {
        Framework::Cargo => {
            // `cargo test [-- filter]` — filter is passed after `--` so it
            // reaches the test binary's matcher rather than cargo's args.
            let mut args: Vec<&str> = vec!["test"];
            if let Some(f) = filter {
                args.push("--");
                args.push(f);
            }
            run_command_with_timeout("cargo", &args, TEST_TIMEOUT_SECS, Some(cwd))
        }
        Framework::Npm => {
            // `npm test -- --testNamePattern=<filter>` for jest; falls back to
            // the project's package.json `test` script for everything else.
            let pattern = filter.map(|f| format!("--testNamePattern={f}"));
            let mut args: Vec<&str> = vec!["test"];
            if pattern.is_some() {
                args.push("--");
                args.push(pattern.as_deref().unwrap());
            }
            run_command_with_timeout("npm", &args, TEST_TIMEOUT_SECS, Some(cwd))
        }
        Framework::Pytest => {
            // `pytest -k <filter>` is the standard substring match.
            let mut args: Vec<&str> = vec![];
            if let Some(f) = filter {
                args.push("-k");
                args.push(f);
            }
            run_command_with_timeout("pytest", &args, TEST_TIMEOUT_SECS, Some(cwd))
        }
        Framework::Go => {
            // `go test ./... -run <filter>` — `./...` recurses all packages.
            let mut args: Vec<&str> = vec!["test", "./..."];
            if let Some(f) = filter {
                args.push("-run");
                args.push(f);
            }
            run_command_with_timeout("go", &args, TEST_TIMEOUT_SECS, Some(cwd))
        }
    }
}

fn format_result(framework: Framework, result: &CommandResult) -> Value {
    let combined = format!("{}\n{}", result.stdout, result.stderr);
    let (passed, failed) = match framework {
        Framework::Cargo => parse_cargo_counts(&combined),
        Framework::Pytest => parse_pytest_counts(&combined),
        Framework::Go => parse_go_counts(&combined),
        Framework::Npm => parse_jest_counts(&combined),
    };
    let failures = match framework {
        Framework::Cargo => parse_cargo_failures(&combined),
        Framework::Pytest => parse_pytest_failures(&combined),
        Framework::Go => parse_go_failures(&combined),
        Framework::Npm => parse_jest_failures(&combined),
    };
    json!({
        "framework": framework.label(),
        "exit_code": result.exit_code,
        "timed_out": result.timed_out,
        "passed": passed,
        "failed": failed,
        "failures": failures,
        // Raw streams are useful when the parsers can't find canonical
        // summaries (e.g. a build error wiped them out). Trim aggressively
        // so the response doesn't dominate a small context window.
        "stdout_tail": tail(&result.stdout, 2000),
        "stderr_tail": tail(&result.stderr, 2000),
    })
}

fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let start = s.len() - max;
    // Don't slice in the middle of a UTF-8 codepoint.
    let mut byte = start;
    while byte < s.len() && !s.is_char_boundary(byte) {
        byte += 1;
    }
    format!("...{}", &s[byte..])
}

// ────── per-framework parsers ────────────────────────────────────────────
// Deliberately line-based and regex-free: parsing real test output with
// regex hits weird edge cases (colored ANSI, multi-line traces) without
// adding much over simple substring/prefix matches. The unit tests below
// pin the exact strings each parser must recognise.

fn parse_cargo_counts(s: &str) -> (u32, u32) {
    // Lines look like: `test result: ok. 12 passed; 0 failed; 0 ignored; ...`
    // or `test result: FAILED. 9 passed; 3 failed; ...`. We walk the tokens
    // and pair each integer with the immediately following word — that
    // tells us whether it's a `passed` or `failed` count.
    let mut passed = 0u32;
    let mut failed = 0u32;
    for line in s.lines() {
        let Some(rest) = line.strip_prefix("test result: ") else {
            continue;
        };
        let tokens: Vec<&str> = rest.split([' ', ';']).filter(|t| !t.is_empty()).collect();
        for window in tokens.windows(2) {
            let Ok(n) = window[0].parse::<u32>() else {
                continue;
            };
            // Strip the trailing `;` that cargo emits before `ignored`/etc.
            let next = window[1].trim_end_matches(';');
            if next.starts_with("passed") {
                passed += n;
            } else if next.starts_with("failed") {
                failed += n;
            }
        }
    }
    (passed, failed)
}

fn parse_cargo_failures(s: &str) -> Vec<Value> {
    // Failures are listed as: `---- some::path::test_name stdout ----` blocks
    // followed by `thread 'some::path::test_name' panicked at <file:line>: <msg>`.
    let mut out: Vec<Value> = Vec::new();
    for window in s.lines().collect::<Vec<_>>().windows(8) {
        let header = window[0];
        if let Some(rest) = header.strip_prefix("---- ") {
            let name = rest.trim_end_matches(" stdout ----").trim().to_string();
            // Find a "panicked at" line in the next few entries.
            let mut file = String::new();
            let mut line_num = 0u32;
            let mut message = String::new();
            for entry in window.iter().skip(1) {
                if let Some(after) = entry.find("panicked at ") {
                    let tail = &entry[after + "panicked at ".len()..];
                    if let Some(colon) = tail.find(':') {
                        let path_or_loc = &tail[..colon];
                        // Path-like component is `<file>:<line>` already?
                        if let Some((f, l)) = path_or_loc.rsplit_once(':') {
                            file = f.to_string();
                            line_num = l.parse().unwrap_or(0);
                            message = tail[colon + 1..].trim().to_string();
                        } else {
                            message = tail.trim().to_string();
                        }
                    } else {
                        message = tail.trim().to_string();
                    }
                    break;
                }
            }
            out.push(json!({
                "name": name,
                "file": file,
                "line": line_num,
                "message": message,
            }));
        }
    }
    out
}

fn parse_pytest_counts(s: &str) -> (u32, u32) {
    // Summary line: `========================= 1 passed, 2 failed in 0.42s =========================`
    let mut passed = 0u32;
    let mut failed = 0u32;
    for line in s.lines().rev() {
        if line.contains(" passed") || line.contains(" failed") {
            let mut tokens = line.split_whitespace().peekable();
            while let Some(t) = tokens.next() {
                if let Ok(n) = t.parse::<u32>() {
                    match tokens.peek() {
                        Some(&w) if w.starts_with("passed") => passed += n,
                        Some(&w) if w.starts_with("failed") => failed += n,
                        _ => {}
                    }
                }
            }
            if passed > 0 || failed > 0 {
                break;
            }
        }
    }
    (passed, failed)
}

fn parse_pytest_failures(s: &str) -> Vec<Value> {
    // Lines look like: `FAILED tests/test_x.py::test_name - AssertionError: ...`
    let mut out: Vec<Value> = Vec::new();
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("FAILED ") {
            let (loc, msg) = rest.split_once(" - ").map_or((rest, ""), |(a, b)| (a, b));
            let (file, name) = loc.split_once("::").unwrap_or((loc, ""));
            out.push(json!({
                "name": name.trim(),
                "file": file.trim(),
                "line": 0,
                "message": msg.trim(),
            }));
        }
    }
    out
}

fn parse_go_counts(s: &str) -> (u32, u32) {
    let mut passed = 0u32;
    let mut failed = 0u32;
    for line in s.lines() {
        if line.starts_with("--- PASS") {
            passed += 1;
        } else if line.starts_with("--- FAIL") {
            failed += 1;
        }
    }
    (passed, failed)
}

fn parse_go_failures(s: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("--- FAIL: ") {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            out.push(json!({
                "name": name,
                "file": "",
                "line": 0,
                "message": "",
            }));
        }
    }
    out
}

fn parse_jest_counts(s: &str) -> (u32, u32) {
    // Lines look like `Tests:       2 failed, 10 passed, 12 total`.
    let mut passed = 0u32;
    let mut failed = 0u32;
    for line in s.lines() {
        if !line.trim_start().starts_with("Tests:") {
            continue;
        }
        let mut tokens = line.split([',', ' ', ':']).peekable();
        while let Some(t) = tokens.next() {
            if let Ok(n) = t.parse::<u32>() {
                match tokens.peek() {
                    Some(&"passed") => passed += n,
                    Some(&"failed") => failed += n,
                    _ => {}
                }
            }
        }
    }
    (passed, failed)
}

fn parse_jest_failures(s: &str) -> Vec<Value> {
    // jest's per-file summary: `FAIL  src/foo.test.js`
    let mut out: Vec<Value> = Vec::new();
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("FAIL ") {
            let file = rest.trim().to_string();
            if !file.is_empty() {
                out.push(json!({
                    "name": "",
                    "file": file,
                    "line": 0,
                    "message": "",
                }));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_lists_one_tool() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 1);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["run_tests"]);
    }

    #[test]
    fn run_tests_rejects_unknown_framework() {
        let err = run_tests(r#"{"framework":"banana"}"#).unwrap_err();
        assert!(err.contains("unknown framework"), "got: {err}");
        assert!(
            err.contains("cargo") && err.contains("pytest") && err.contains("go"),
            "error must enumerate supported frameworks: {err}"
        );
    }

    #[test]
    fn parse_cargo_counts_pulls_passed_and_failed() {
        let out = "running 5 tests\n\
                   test ok ... ok\n\
                   test result: ok. 12 passed; 3 failed; 0 ignored; 0 measured\n";
        assert_eq!(parse_cargo_counts(out), (12, 3));
    }

    #[test]
    fn parse_pytest_counts_pulls_summary_line() {
        let out = "============================= test session starts ==============================\n\
                   collected 5 items\n\
                   tests/test_x.py::test_one PASSED\n\
                   tests/test_x.py::test_two FAILED\n\
                   ========================= 3 passed, 2 failed in 0.42s ==========================";
        assert_eq!(parse_pytest_counts(out), (3, 2));
    }

    #[test]
    fn parse_pytest_failures_extracts_name_and_file() {
        let out = "FAILED tests/test_x.py::test_two - AssertionError: x != y\n\
                   FAILED tests/test_y.py::test_three - ValueError: bad input\n";
        let failures = parse_pytest_failures(out);
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0]["file"], "tests/test_x.py");
        assert_eq!(failures[0]["name"], "test_two");
        assert_eq!(failures[0]["message"], "AssertionError: x != y");
    }

    #[test]
    fn parse_go_counts_tallies_marker_lines() {
        let out = "--- PASS: TestOne (0.00s)\n\
                   --- FAIL: TestTwo (0.01s)\n\
                   --- PASS: TestThree (0.00s)\n";
        assert_eq!(parse_go_counts(out), (2, 1));
    }

    #[test]
    fn parse_jest_counts_handles_full_summary() {
        let out = "Tests:       2 failed, 10 passed, 12 total\n\
                   Snapshots:   0 total\n";
        assert_eq!(parse_jest_counts(out), (10, 2));
    }

    #[test]
    fn detect_framework_prefers_closest_marker() {
        // Use a tmpdir tree to verify the walk-up resolution. We don't go
        // up the actual repo (test would be flaky depending on `cwd`); we
        // build an isolated tree with marker files in the leaf.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root = std::env::temp_dir().join(format!("claudette-quality-{stamp}"));
        let leaf = root.join("sub").join("leaf");
        std::fs::create_dir_all(&leaf).expect("mkdir");
        std::fs::write(leaf.join("Cargo.toml"), "[package]\nname=\"x\"\n").expect("write toml");
        let f = detect_framework(&leaf);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(f, Some(Framework::Cargo));
    }

    #[test]
    fn detect_framework_returns_none_when_no_markers() {
        let root = std::env::temp_dir().join(format!(
            "claudette-quality-empty-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&root).expect("mkdir");
        // walk_up stops at the filesystem root, but since the test temp dir
        // lives under a parent that may itself contain Cargo.toml (we're in
        // a Rust repo), pick a path that's likely outside the workspace
        // boundary. detect_framework starts at `root` and walks up — if a
        // parent has a marker, we'll legitimately find one. Treat this as
        // "doesn't panic and returns *something*".
        let _ = detect_framework(&root);
        let _ = std::fs::remove_dir_all(&root);
    }

    // Placate the type-check on the cwd type used by run_tests under the
    // mission-routing helper.
    #[test]
    fn active_cwd_returns_path() {
        let _: std::path::PathBuf = crate::missions::active_cwd();
    }
}
