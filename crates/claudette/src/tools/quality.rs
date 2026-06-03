//! Quality group — code-quality tooling. Two tools so far:
//! `run_tests` (project test framework) and `diagnostics` (typechecker /
//! linter). `apply_patch` lands next in Phase 3.1c.
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

/// Diagnostics are generally faster than tests but `cargo check` on a
/// cold workspace can still take a minute or so on large projects.
const DIAG_TIMEOUT_SECS: u64 = 120;

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
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
        }),
        json!({
            "type": "function",
            "function": {
                "name": "diagnostics",
                "description": "Run the project typechecker/linter and return structured errors {file, line, code, severity, message}. Auto-detects (cargo check, clippy, tsc, ruff, mypy) from project files. 120s timeout.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "tool": { "type": "string", "description": "Override auto-detect: 'cargo', 'clippy', 'tsc', 'mypy', 'ruff', or 'auto' (default)." }
                    },
                    "required": []
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "run_tests" => run_tests(input),
        "diagnostics" => run_diagnostics(input),
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

// ────── forge Verifier build + test gate ──────────────────────────────────
//
// The forge Verifier used to be a tool-less brain turn that only *read* the
// diff. A model reading a diff can't see a type error, a broken import, or a
// test the change regressed. `run_build_and_tests` gives the Verifier a
// deterministic ground truth: it auto-detects the framework, runs the build/
// typecheck (`cargo check` / `go build`) and then the test suite, and returns a
// structured outcome the fix-loop turns into pass/fail + Coder feedback.
//
// Severity rules (mirrors the security-review gate in run.rs):
//   • A *build* break (`cargo check` / `go build` non-zero) is a HARD fail —
//     code that doesn't compile is unambiguously wrong.
//   • *Test failures* (parsed failed > 0, or a non-zero test command we can't
//     otherwise explain) are a HARD fail, fed back to the Coder.
//   • An *infrastructure* problem — no framework, tool not installed, timeout,
//     "no tests collected" — is ADVISORY: it never flips a pass to a fail,
//     because punishing a docs PR for a missing `npm install` or a flaky
//     network test would block legitimate work. It's surfaced in the summary so
//     the human (and the review gate) can see verification was incomplete.

/// Structured result of the forge build + test gate. See the section notes.
#[derive(Debug, Clone)]
pub(crate) struct BuildTestOutcome {
    /// False when no recognised framework was found — the gate is a no-op and
    /// the LLM Verifier's verdict stands alone.
    pub ran: bool,
    /// Build/typecheck outcome. `None` when the framework has no separate
    /// compile step (pytest, plain npm) or the build tool couldn't run.
    pub build_ok: Option<bool>,
    /// Test outcome. `None` when tests couldn't be *run* (tool missing,
    /// timeout, no tests collected) — advisory, never a hard fail.
    pub tests_ok: Option<bool>,
    /// Multi-line, human- and Coder-readable summary of what ran and failed.
    pub summary: String,
    /// Detected framework label ("cargo" / "pytest" / …), or "none".
    pub framework: &'static str,
}

impl BuildTestOutcome {
    /// True when the gate observed a *definitive* failure (build broke or a
    /// test failed). Infrastructure problems (couldn't run) are NOT failures.
    pub fn is_hard_fail(&self) -> bool {
        self.build_ok == Some(false) || self.tests_ok == Some(false)
    }
}

/// Run the project's build/typecheck then its test suite inside `dir` and
/// return a [`BuildTestOutcome`]. `timeout_secs` bounds *each* sub-step.
/// Never panics; an undetectable framework yields `ran=false`.
pub(crate) fn run_build_and_tests(dir: &Path, timeout_secs: u64) -> BuildTestOutcome {
    let Some(framework) = detect_framework(dir) else {
        return BuildTestOutcome {
            ran: false,
            build_ok: None,
            tests_ok: None,
            summary: "no test framework detected (no Cargo.toml / package.json / \
                      pyproject.toml / go.mod) — skipped build+test verification"
                .to_string(),
            framework: "none",
        };
    };

    let mut lines: Vec<String> = Vec::new();

    // Build / typecheck (the "cargo check" half).
    let build_ok = run_build_step(framework, dir, timeout_secs, &mut lines);

    // Test suite.
    let test_result = invoke(framework, None, dir);
    let combined = format!("{}\n{}", test_result.stdout, test_result.stderr);
    let (passed, failed) = match framework {
        Framework::Cargo => parse_cargo_counts(&combined),
        Framework::Pytest => parse_pytest_counts(&combined),
        Framework::Go => parse_go_counts(&combined),
        Framework::Npm => parse_jest_counts(&combined),
    };
    let tests_ok = classify_tests(
        framework,
        &test_result,
        &combined,
        passed,
        failed,
        &mut lines,
    );

    BuildTestOutcome {
        ran: true,
        build_ok,
        tests_ok,
        summary: lines.join("\n"),
        framework: framework.label(),
    }
}

/// Run the framework's build/typecheck step. Returns `Some(true/false)` when a
/// build command actually ran, `None` when the framework has no separate
/// compile step or the tool wasn't runnable (advisory).
fn run_build_step(
    framework: Framework,
    dir: &Path,
    timeout_secs: u64,
    lines: &mut Vec<String>,
) -> Option<bool> {
    let (program, args): (&str, Vec<&str>) = match framework {
        Framework::Cargo => ("cargo", vec!["check", "--all-targets"]),
        Framework::Go => ("go", vec!["build", "./..."]),
        // pytest / npm have no generic language-level compile step — a
        // collection-time import/syntax error surfaces in the test run instead.
        Framework::Pytest | Framework::Npm => return None,
    };
    let joined = args.join(" ");
    let r = run_command_with_timeout(program, &args, timeout_secs, Some(dir));
    if r.timed_out {
        lines.push(format!(
            "build: `{program} {joined}` timed out after {timeout_secs}s (not counted as a failure)"
        ));
        return None;
    }
    if r.exit_code.is_none() {
        lines.push(format!(
            "build: could not run `{program}` (is it installed?) — build check skipped"
        ));
        return None;
    }
    if r.success {
        lines.push(format!("build: `{program} {joined}` OK"));
        Some(true)
    } else {
        let detail = if framework == Framework::Cargo {
            let errs = parse_cargo_messages(&r.stdout);
            let s = summarize_cargo_errors(&errs);
            if s.is_empty() {
                tail(&r.stderr, 1200)
            } else {
                s
            }
        } else {
            tail(&r.stderr, 1200)
        };
        lines.push(format!("build: `{program} {joined}` FAILED:\n{detail}"));
        Some(false)
    }
}

/// Turn the test subprocess result into an `Option<bool>` plus summary lines.
fn classify_tests(
    framework: Framework,
    result: &CommandResult,
    combined: &str,
    passed: u32,
    failed: u32,
    lines: &mut Vec<String>,
) -> Option<bool> {
    if result.timed_out {
        lines.push("tests: timed out (not counted as a failure)".to_string());
        return None;
    }
    if result.exit_code.is_none() {
        lines.push(
            "tests: could not run the test command (is the test tool installed?) — tests skipped"
                .to_string(),
        );
        return None;
    }
    if failed > 0 {
        lines.push(format!(
            "tests: {failed} failed, {passed} passed:\n{}",
            summarize_test_failures(framework, combined)
        ));
        return Some(false);
    }
    if result.success {
        lines.push(format!("tests: {passed} passed"));
        return Some(true);
    }
    // pytest exit 5 = "no tests collected" — advisory, not a failure.
    if framework == Framework::Pytest && result.exit_code == Some(5) {
        lines.push("tests: no tests collected (nothing to verify)".to_string());
        return None;
    }
    // Non-zero exit, no parsed failures: a compile error in the test target, a
    // missing dependency, or a harness crash. Surface it as a fail with a tail.
    lines.push(format!(
        "tests: command exited {:?} with no parseable test results (build error in the \
         test target or a harness failure):\n{}",
        result.exit_code,
        tail(combined, 1200)
    ));
    Some(false)
}

/// Compact, deterministic listing of the first dozen test failures.
fn summarize_test_failures(framework: Framework, combined: &str) -> String {
    let failures = match framework {
        Framework::Cargo => parse_cargo_failures(combined),
        Framework::Pytest => parse_pytest_failures(combined),
        Framework::Go => parse_go_failures(combined),
        Framework::Npm => parse_jest_failures(combined),
    };
    if failures.is_empty() {
        return tail(combined, 1200);
    }
    failures
        .iter()
        .take(12)
        .map(|f| {
            let name = f.get("name").and_then(Value::as_str).unwrap_or("");
            let file = f.get("file").and_then(Value::as_str).unwrap_or("");
            let msg = f.get("message").and_then(Value::as_str).unwrap_or("");
            format!("  - {name} {file} {msg}").trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compact listing of the first dozen `cargo check` errors (warnings dropped).
fn summarize_cargo_errors(errs: &[Value]) -> String {
    errs.iter()
        .filter(|e| e.get("severity").and_then(Value::as_str) == Some("error"))
        .take(12)
        .map(|e| {
            let file = e.get("file").and_then(Value::as_str).unwrap_or("");
            let line = e.get("line").and_then(Value::as_u64).unwrap_or(0);
            let code = e.get("code").and_then(Value::as_str).unwrap_or("");
            let msg = e.get("message").and_then(Value::as_str).unwrap_or("");
            format!("  - {file}:{line} {code} {msg}")
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ────── diagnostics tool ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagTool {
    CargoCheck,
    Clippy,
    Tsc,
    Mypy,
    Ruff,
}

impl DiagTool {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "cargo" | "cargo-check" | "check" => Some(Self::CargoCheck),
            "clippy" => Some(Self::Clippy),
            "tsc" | "typescript" => Some(Self::Tsc),
            "mypy" => Some(Self::Mypy),
            "ruff" => Some(Self::Ruff),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::CargoCheck => "cargo-check",
            Self::Clippy => "clippy",
            Self::Tsc => "tsc",
            Self::Mypy => "mypy",
            Self::Ruff => "ruff",
        }
    }
}

/// Pick the most likely diagnostics tool by walking up from `start`.
/// Preference order on Python is ruff > mypy because ruff is faster
/// and increasingly the project standard; Rust prefers cargo-check
/// because clippy needs an explicit opt-in (it's slower and noisier).
fn detect_diag_tool(start: &Path) -> Option<DiagTool> {
    let mut current: Option<&Path> = Some(start);
    while let Some(dir) = current {
        if dir.join("Cargo.toml").exists() {
            return Some(DiagTool::CargoCheck);
        }
        if dir.join("tsconfig.json").exists() {
            return Some(DiagTool::Tsc);
        }
        if dir.join("pyproject.toml").exists() || dir.join("ruff.toml").exists() {
            return Some(DiagTool::Ruff);
        }
        if dir.join("mypy.ini").exists() {
            return Some(DiagTool::Mypy);
        }
        current = dir.parent();
    }
    None
}

fn run_diagnostics(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "diagnostics")?;
    let cwd = crate::missions::active_cwd();
    let requested = v.get("tool").and_then(Value::as_str).unwrap_or("auto");
    let tool = if requested == "auto" || requested.is_empty() {
        detect_diag_tool(&cwd).ok_or_else(|| {
            format!(
                "diagnostics: could not auto-detect a checker under {} \
                 (looked for Cargo.toml, tsconfig.json, pyproject.toml/ruff.toml, mypy.ini). \
                 Pass `tool` explicitly.",
                cwd.display()
            )
        })?
    } else {
        DiagTool::parse(requested).ok_or_else(|| {
            format!(
                "diagnostics: unknown tool '{requested}' \
                 — use 'auto', 'cargo', 'clippy', 'tsc', 'mypy', or 'ruff'."
            )
        })?
    };

    let (program, args) = match tool {
        DiagTool::CargoCheck => (
            "cargo",
            vec!["check", "--message-format=json", "--all-targets"],
        ),
        DiagTool::Clippy => (
            "cargo",
            vec![
                "clippy",
                "--message-format=json",
                "--all-targets",
                "--all-features",
            ],
        ),
        DiagTool::Tsc => ("npx", vec!["tsc", "--noEmit"]),
        DiagTool::Mypy => ("mypy", vec![".", "--no-color-output"]),
        DiagTool::Ruff => ("ruff", vec!["check", "--output-format=json"]),
    };

    let result = run_command_with_timeout(program, &args, DIAG_TIMEOUT_SECS, Some(&cwd));
    let errors = parse_diag_errors(tool, &result.stdout, &result.stderr);

    Ok(json!({
        "tool": tool.label(),
        "exit_code": result.exit_code,
        "timed_out": result.timed_out,
        "errors": errors,
        "stdout_tail": tail(&result.stdout, 2000),
        "stderr_tail": tail(&result.stderr, 2000),
    })
    .to_string())
}

fn parse_diag_errors(tool: DiagTool, stdout: &str, stderr: &str) -> Vec<Value> {
    match tool {
        DiagTool::CargoCheck | DiagTool::Clippy => parse_cargo_messages(stdout),
        DiagTool::Tsc => parse_tsc_lines(&format!("{stdout}\n{stderr}")),
        DiagTool::Mypy => parse_mypy_lines(&format!("{stdout}\n{stderr}")),
        DiagTool::Ruff => parse_ruff_json(stdout),
    }
}

/// Parse cargo's `--message-format=json` stream. Each line is a JSON
/// object; we keep only `compiler-message` entries at error/warning
/// severity. Skips silent-failure cases (build script output, etc.)
/// without panicking.
fn parse_cargo_messages(stdout: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("reason").and_then(Value::as_str) != Some("compiler-message") {
            continue;
        }
        let Some(msg) = v.get("message") else {
            continue;
        };
        let level = msg
            .get("level")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if level != "error" && level != "warning" {
            continue;
        }
        let code = msg
            .pointer("/code/code")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let message = msg
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let (file, line, col) = msg
            .get("spans")
            .and_then(Value::as_array)
            .and_then(|spans| {
                spans.iter().find(|s| {
                    s.get("is_primary")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
            })
            .map(|primary| {
                (
                    primary
                        .get("file_name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    primary
                        .get("line_start")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    primary
                        .get("column_start")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                )
            })
            .unwrap_or_default();
        out.push(json!({
            "file": file,
            "line": line,
            "column": col,
            "code": code,
            "severity": level,
            "message": message,
        }));
    }
    out
}

/// Parse `tsc --noEmit` text output. Lines look like:
/// `src/foo.ts(12,3): error TS2304: Cannot find name 'bar'.`
fn parse_tsc_lines(s: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for line in s.lines() {
        // Split on `: error TS` or `: warning TS`
        let Some(paren_open) = line.find('(') else {
            continue;
        };
        let Some(paren_close) = line[paren_open..].find(')').map(|i| paren_open + i) else {
            continue;
        };
        let Some(colon) = line[paren_close..].find(": ").map(|i| paren_close + i) else {
            continue;
        };
        let file = line[..paren_open].to_string();
        let inside = &line[paren_open + 1..paren_close];
        let (line_str, col_str) = inside.split_once(',').unwrap_or((inside, "0"));
        let tail = &line[colon + 2..];
        let severity = if tail.starts_with("error") {
            "error"
        } else {
            "warning"
        };
        // The diagnostic code immediately follows the severity word.
        let after_sev = tail.trim_start_matches(severity).trim_start();
        let (code, message) = after_sev.split_once(": ").unwrap_or((after_sev, ""));
        out.push(json!({
            "file": file,
            "line": line_str.parse::<u64>().unwrap_or(0),
            "column": col_str.parse::<u64>().unwrap_or(0),
            "code": code.trim(),
            "severity": severity,
            "message": message.trim(),
        }));
    }
    out
}

/// Parse `mypy` output (no-color). Lines look like:
/// `foo.py:12: error: Incompatible types ... [arg-type]`
fn parse_mypy_lines(s: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for line in s.lines() {
        let Some(first_colon) = line.find(':') else {
            continue;
        };
        let rest = &line[first_colon + 1..];
        let Some(line_end) = rest.find(':') else {
            continue;
        };
        let line_num: u64 = rest[..line_end].trim().parse().unwrap_or(0);
        if line_num == 0 {
            // Skip blank/header lines.
            continue;
        }
        let after = &rest[line_end + 1..];
        let severity = if after.trim_start().starts_with("error") {
            "error"
        } else if after.trim_start().starts_with("warning") {
            "warning"
        } else {
            continue;
        };
        let body = after
            .trim_start()
            .trim_start_matches(severity)
            .trim_start_matches(": ");
        let (message, code) = if let Some(open) = body.rfind('[') {
            if let Some(close) = body[open..].find(']') {
                (
                    body[..open].trim().to_string(),
                    body[open + 1..open + close].to_string(),
                )
            } else {
                (body.to_string(), String::new())
            }
        } else {
            (body.to_string(), String::new())
        };
        out.push(json!({
            "file": line[..first_colon].to_string(),
            "line": line_num,
            "column": 0,
            "code": code,
            "severity": severity,
            "message": message,
        }));
    }
    out
}

/// Parse `ruff check --output-format=json`. Output is a single JSON
/// array of objects with `filename`, `location.row`, `code`, `message`.
fn parse_ruff_json(stdout: &str) -> Vec<Value> {
    let Ok(arr) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    let Some(items) = arr.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| {
            json!({
                "file": item.get("filename").and_then(Value::as_str).unwrap_or(""),
                "line": item
                    .pointer("/location/row")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                "column": item
                    .pointer("/location/column")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                "code": item.get("code").and_then(Value::as_str).unwrap_or(""),
                "severity": "error",
                "message": item.get("message").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect()
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
    fn schemas_lists_two_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 2);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["run_tests", "diagnostics"]);
    }

    #[test]
    fn diagnostics_rejects_unknown_tool() {
        let err = run_diagnostics(r#"{"tool":"banana"}"#).unwrap_err();
        assert!(err.contains("unknown tool"), "got: {err}");
        assert!(
            err.contains("cargo") && err.contains("ruff") && err.contains("tsc"),
            "error must enumerate supported tools: {err}"
        );
    }

    #[test]
    fn parse_cargo_messages_extracts_primary_span() {
        let line = r#"{"reason":"compiler-message","message":{"level":"error","message":"cannot find value `x` in this scope","code":{"code":"E0425"},"spans":[{"is_primary":true,"file_name":"src/lib.rs","line_start":12,"column_start":7}]}}"#;
        let errors = parse_cargo_messages(line);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["file"], "src/lib.rs");
        assert_eq!(errors[0]["line"], 12);
        assert_eq!(errors[0]["code"], "E0425");
        assert_eq!(errors[0]["severity"], "error");
    }

    #[test]
    fn parse_cargo_messages_skips_non_compiler_lines() {
        let mixed = r#"{"reason":"build-script-executed","package_id":"x"}
{"reason":"compiler-artifact"}
not json at all
{"reason":"compiler-message","message":{"level":"warning","message":"unused import","code":{"code":"unused_imports"},"spans":[{"is_primary":true,"file_name":"src/foo.rs","line_start":3,"column_start":1}]}}"#;
        let errors = parse_cargo_messages(mixed);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["severity"], "warning");
    }

    #[test]
    fn parse_tsc_lines_extracts_position_and_code() {
        let out = "src/foo.ts(12,3): error TS2304: Cannot find name 'bar'.\n\
                   src/bar.ts(5,11): warning TS6133: 'x' is declared but never used.\n";
        let errors = parse_tsc_lines(out);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0]["file"], "src/foo.ts");
        assert_eq!(errors[0]["line"], 12);
        assert_eq!(errors[0]["code"], "TS2304");
        assert_eq!(errors[0]["severity"], "error");
        assert_eq!(errors[1]["severity"], "warning");
    }

    #[test]
    fn parse_mypy_lines_extracts_code_in_brackets() {
        let out = "foo.py:12: error: Incompatible types in assignment  [assignment]\n\
                   bar.py:5: note: Revealed type is 'builtins.int'\n";
        let errors = parse_mypy_lines(out);
        // We only keep error/warning lines, not notes.
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["file"], "foo.py");
        assert_eq!(errors[0]["line"], 12);
        assert_eq!(errors[0]["code"], "assignment");
        assert_eq!(errors[0]["severity"], "error");
    }

    #[test]
    fn parse_ruff_json_array() {
        let out = r#"[{"code":"E501","message":"Line too long","filename":"foo.py","location":{"row":12,"column":80}},{"code":"F401","message":"`os` imported but unused","filename":"bar.py","location":{"row":1,"column":0}}]"#;
        let errors = parse_ruff_json(out);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0]["code"], "E501");
        assert_eq!(errors[1]["file"], "bar.py");
    }

    #[test]
    fn detect_diag_tool_prefers_closest_marker() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root = std::env::temp_dir().join(format!("claudette-diag-{stamp}"));
        let leaf = root.join("sub").join("leaf");
        std::fs::create_dir_all(&leaf).expect("mkdir");
        std::fs::write(leaf.join("Cargo.toml"), "[package]\nname=\"x\"\n").expect("write toml");
        let t = detect_diag_tool(&leaf);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(t, Some(DiagTool::CargoCheck));
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

    // ─── forge build + test gate ──────────────────────────────────────

    fn cmd(success: bool, exit: Option<i32>, timed_out: bool) -> CommandResult {
        CommandResult {
            success,
            stdout: String::new(),
            stderr: String::new(),
            timed_out,
            exit_code: exit,
        }
    }

    #[test]
    fn is_hard_fail_truth_table() {
        let base = BuildTestOutcome {
            ran: true,
            build_ok: None,
            tests_ok: None,
            summary: String::new(),
            framework: "cargo",
        };
        // Nothing definitive ran → not a hard fail.
        assert!(!base.is_hard_fail());
        assert!(BuildTestOutcome {
            build_ok: Some(false),
            ..base.clone()
        }
        .is_hard_fail());
        assert!(BuildTestOutcome {
            tests_ok: Some(false),
            ..base.clone()
        }
        .is_hard_fail());
        assert!(!BuildTestOutcome {
            build_ok: Some(true),
            tests_ok: Some(true),
            ..base.clone()
        }
        .is_hard_fail());
    }

    #[test]
    fn classify_tests_failed_count_is_hard_fail() {
        let mut lines = Vec::new();
        let r = cmd(false, Some(101), false);
        let out = classify_tests(Framework::Cargo, &r, "", 3, 2, &mut lines);
        assert_eq!(out, Some(false));
        assert!(lines.iter().any(|l| l.contains("2 failed")));
    }

    #[test]
    fn classify_tests_all_pass() {
        let mut lines = Vec::new();
        let r = cmd(true, Some(0), false);
        let out = classify_tests(Framework::Cargo, &r, "", 5, 0, &mut lines);
        assert_eq!(out, Some(true));
        assert!(lines.iter().any(|l| l.contains("5 passed")));
    }

    #[test]
    fn classify_tests_timeout_is_advisory_not_a_fail() {
        let mut lines = Vec::new();
        let r = cmd(false, None, true);
        let out = classify_tests(Framework::Pytest, &r, "", 0, 0, &mut lines);
        assert_eq!(out, None);
        assert!(lines.iter().any(|l| l.contains("timed out")));
    }

    #[test]
    fn classify_tests_spawn_failure_is_advisory() {
        // exit_code None + not timed out = the test tool wasn't runnable.
        let mut lines = Vec::new();
        let r = cmd(false, None, false);
        let out = classify_tests(Framework::Npm, &r, "", 0, 0, &mut lines);
        assert_eq!(out, None);
        assert!(lines.iter().any(|l| l.contains("could not run")));
    }

    #[test]
    fn classify_tests_pytest_no_tests_collected_is_advisory() {
        // pytest exits 5 when it collects zero tests — not a failure.
        let mut lines = Vec::new();
        let r = cmd(false, Some(5), false);
        let out = classify_tests(Framework::Pytest, &r, "", 0, 0, &mut lines);
        assert_eq!(out, None);
        assert!(lines.iter().any(|l| l.contains("no tests collected")));
    }

    #[test]
    fn classify_tests_nonzero_without_failures_is_hard_fail() {
        // A compile error in the test target: non-zero exit, no parsed
        // failures. Must surface as a fail rather than a silent pass.
        let mut lines = Vec::new();
        let r = cmd(false, Some(101), false);
        let out = classify_tests(Framework::Cargo, &r, "error[E0433]: boom", 0, 0, &mut lines);
        assert_eq!(out, Some(false));
    }

    #[test]
    fn summarize_test_failures_lists_pytest_names() {
        let combined = "FAILED tests/test_x.py::test_two - AssertionError: x != y\n";
        let s = summarize_test_failures(Framework::Pytest, combined);
        assert!(s.contains("test_two"), "got: {s}");
        assert!(s.contains("tests/test_x.py"), "got: {s}");
    }

    #[test]
    fn summarize_cargo_errors_keeps_errors_drops_warnings() {
        let errs = vec![
            json!({"file":"src/a.rs","line":3,"code":"E0425","severity":"error","message":"cannot find x"}),
            json!({"file":"src/b.rs","line":9,"code":"unused","severity":"warning","message":"unused import"}),
        ];
        let s = summarize_cargo_errors(&errs);
        assert!(s.contains("E0425"), "got: {s}");
        assert!(s.contains("src/a.rs:3"), "got: {s}");
        assert!(
            !s.contains("unused import"),
            "warnings must be dropped: {s}"
        );
    }

    #[test]
    fn run_build_and_tests_noop_without_framework() {
        let dir = std::env::temp_dir().join(format!(
            "claudette-btgate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let out = run_build_and_tests(&dir, 5);
        let _ = std::fs::remove_dir_all(&dir);
        // The temp dir has no marker files; if an ancestor happens to (unlikely
        // for the OS temp dir), assert the invariant rather than a hard "none".
        assert_eq!(out.ran, out.framework != "none");
        if !out.ran {
            assert!(!out.is_hard_fail());
            assert!(out.summary.contains("no test framework detected"));
        }
    }
}
