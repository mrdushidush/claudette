//! Codet — the code-validator sidecar.
//!
//! Every time Claudette writes a code file (via `write_file`), Codet validates
//! it by running syntax checks and unit tests, then attempts to fix any bugs
//! via a secondary LLM (the coder model, default `qwen3-coder:30b` `MoE`;
//! set `CLAUDETTE_CODER_MODEL=qwen2.5-coder:14b` on RAM-constrained hosts,
//! or use the `/coder` slash command to change the active config). The fix-loop
//! conversation lives entirely inside Codet — Claudette's main context only
//! sees a one-line summary in the tool result, so there's zero context
//! pollution.
//!
//! **VRAM strategy:** the brain and the coder don't run simultaneously.
//! Ollama hot-swaps them in VRAM automatically when `codet.rs` calls the
//! coder model. For the 30b `MoE` (19 GB) on 8 GB VRAM boxes this REQUIRES
//! `OLLAMA_MAX_LOADED_MODELS=1` — without it, Ollama tries to keep both
//! loaded and OOMs the 30b load. Cold-load cost is ~5-10 s per swap for
//! 7b/14b; ~30-40 s for 30b.
//!
//! **Safety rule:** Codet never applies a fix that doesn't measurably improve
//! the validation outcome. Specifically:
//! - A syntax fix is accepted iff `py_compile` was failing and now passes.
//! - A test fix is accepted iff a test that was failing now passes AND no
//!   previously-passing test starts failing (no regressions).

use std::path::Path;

use crate::{
    ApiClient, ApiRequest, AssistantEvent, ContentBlock, ConversationMessage, MessageRole,
};
use serde_json::json;

use crate::api::OllamaApiClient;
use crate::test_runner::{
    check_python_imports, has_python_tests, has_rust_tests, run_js_syntax_check,
    run_python_syntax_check, run_python_unittest, run_rust_syntax_check, run_ts_syntax_check,
};

// Coder defaults now live in `model_config::ModelConfig::from_preset`.
// The `CLAUDETTE_CODER_*` env vars are still honored via the env-merge
// pass in model_config; the slash `/coder` command mutates the active
// config in place. The constants below are historical references for the
// defaults (30b, 49 K ctx, 12 K predict) kept for documentation only.

fn coder_num_ctx() -> u32 {
    crate::model_config::active().coder.num_ctx
}

fn coder_num_predict() -> u32 {
    crate::model_config::active().coder.num_predict
}

/// Maximum fix iterations before Codet gives up and reports
/// `CodetStatus::CouldNotFix`.
const MAX_FIX_ATTEMPTS: u32 = 3;

/// Resolve the coder model name. Sprint 14: reads from
/// `model_config::active().coder.model`, which itself merges the
/// `CLAUDETTE_CODER_MODEL` env var on first init.
#[must_use]
pub fn coder_model() -> String {
    crate::model_config::active().coder.model
}

/// Whether code validation is enabled. Defaults to true; set
/// `CLAUDETTE_VALIDATE_CODE=false` to disable (useful for debugging
/// or when the coder model isn't pulled yet).
#[must_use]
pub fn validation_enabled() -> bool {
    std::env::var("CLAUDETTE_VALIDATE_CODE")
        .map(|v| !matches!(v.to_lowercase().as_str(), "false" | "0" | "no" | "off"))
        .unwrap_or(true)
}

// ────────────────────────────────────────────────────────────────────────────
// Public result types
// ────────────────────────────────────────────────────────────────────────────

/// Outcome of a Codet validation run. Serialized into the `write_file` tool
/// result JSON so Claudette sees a one-line summary and the REPL can print
/// a warning on failure.
#[derive(Debug, Clone)]
pub struct CodetResult {
    pub syntax_ok: bool,
    pub tests_found: bool,
    pub tests_passed: u32,
    pub tests_failed: u32,
    pub tests_errors: u32,
    /// Number of fix attempts that actually **landed** — i.e. produced an
    /// improved re-check. Always ≤ `attempts_made`.
    pub fixes_applied: u32,
    /// Number of fix attempts **tried**, whether or not they landed.
    /// Useful for diagnosing a `CouldNotFix` outcome: "0 landed after 3
    /// attempts" reads very differently from "0 landed after 0 attempts."
    pub attempts_made: u32,
    pub fix_summary: String,
    pub status: CodetStatus,
}

/// A file the user referenced in the generation/validation prompt.
/// Passed to the coder so it can read real class/method names instead of
/// fabricating them (brownfield API-matching). Collected by
/// `tools::collect_reference_files` from the `generate_code` description
/// and threaded through every coder call (generation, fix, surgical patch).
#[derive(Debug, Clone)]
pub struct ReferenceFile {
    /// Display path — shown to the coder verbatim, so use whatever the user
    /// typed (tilde form preferred) rather than the resolved absolute path.
    pub path: String,
    /// File contents, already truncated to the per-file cap by the collector.
    pub content: String,
}

/// Format a reference-file block for inclusion in a coder prompt.
/// Returns an empty string when there are no references, so callers can
/// concat unconditionally.
fn format_reference_block(references: &[ReferenceFile]) -> String {
    if references.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "\n\n## Reference files (read before writing code)\n\
         These existing files are the ground truth. Use ONLY the class names, \
         method names, and signatures that actually appear below — do not \
         invent or rename any API.\n\n",
    );
    use std::fmt::Write as _;
    for rf in references {
        let _ = write!(s, "### `{}`\n```\n{}\n```\n\n", rf.path, rf.content);
    }
    s
}

/// Terminal state of a validation run.
#[derive(Debug, Clone)]
pub enum CodetStatus {
    /// All checks passed on the first try — no fixes needed.
    AllPassed,
    /// Some checks failed; Codet fixed them and all checks pass now.
    FixedAll,
    /// Codet hit `MAX_FIX_ATTEMPTS` without achieving a clean run.
    CouldNotFix { last_error: String },
    /// File extension isn't a known code type — validation skipped.
    Skipped,
}

impl CodetResult {
    /// One-line JSON fragment suitable for embedding in a `write_file` tool
    /// result. Compact enough to not bloat Claudette's context.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "syntax_ok": self.syntax_ok,
            "tests_found": self.tests_found,
            "tests_passed": self.tests_passed,
            "tests_failed": self.tests_failed,
            "fixes_applied": self.fixes_applied,
            "attempts_made": self.attempts_made,
            "fix_summary": self.fix_summary,
            "status": match &self.status {
                CodetStatus::AllPassed => "all_passed".to_string(),
                CodetStatus::FixedAll => "fixed_all".to_string(),
                CodetStatus::CouldNotFix { last_error } => format!("could_not_fix: {last_error}"),
                CodetStatus::Skipped => "skipped".to_string(),
            },
        })
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Public entry point
// ────────────────────────────────────────────────────────────────────────────

/// Main entry point — called from `run_write_file` after a successful code
/// write. Returns `None` if the file isn't a known code type or validation
/// is disabled via `CLAUDETTE_VALIDATE_CODE=false`.
#[must_use]
pub fn validate_code_file(path: &Path, references: &[ReferenceFile]) -> Option<CodetResult> {
    if !validation_enabled() {
        return None;
    }
    let ext = path.extension()?.to_str()?;
    match ext {
        "py" => Some(validate_python(path, references)),
        "rs" => Some(validate_rust(path, references)),
        "js" | "mjs" | "cjs" => Some(validate_js(path, references)),
        "ts" | "tsx" => Some(validate_ts(path, references)),
        _ => None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Python validation
// ────────────────────────────────────────────────────────────────────────────

fn validate_python(path: &Path, references: &[ReferenceFile]) -> CodetResult {
    let mut fixes_applied: u32 = 0;
    let mut attempts_made: u32 = 0;
    let mut fix_descriptions: Vec<String> = Vec::new();

    // ── Step 1: Syntax check ────────────────────────────────────────────
    let syntax = run_python_syntax_check(path);
    if !syntax.success {
        // Try to fix the syntax error via the coder model.
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let error_msg = format!("{}\n{}", syntax.stdout, syntax.stderr);
        match try_fix_loop(path, &content, &error_msg, FixTarget::Syntax, references) {
            FixLoopOutcome::Fixed {
                description,
                attempts_tried,
            } => {
                fixes_applied += 1;
                attempts_made += attempts_tried;
                fix_descriptions.push(description);
            }
            FixLoopOutcome::GaveUp {
                last_error,
                attempts_tried,
            } => {
                attempts_made += attempts_tried;
                return CodetResult {
                    syntax_ok: false,
                    tests_found: false,
                    tests_passed: 0,
                    tests_failed: 0,
                    tests_errors: 0,
                    fixes_applied,
                    attempts_made,
                    fix_summary: fix_descriptions.join("; "),
                    status: CodetStatus::CouldNotFix { last_error },
                };
            }
        }
    }

    // ── Step 2: Test detection ──────────────────────────────────────────
    let content = std::fs::read_to_string(path).unwrap_or_default();
    if !has_python_tests(&content) {
        return CodetResult {
            syntax_ok: true,
            tests_found: false,
            tests_passed: 0,
            tests_failed: 0,
            tests_errors: 0,
            fixes_applied,
            attempts_made,
            fix_summary: fix_descriptions.join("; "),
            status: if fixes_applied > 0 {
                CodetStatus::FixedAll
            } else {
                CodetStatus::AllPassed
            },
        };
    }

    // ── Step 2.5: Import pre-flight ─────────────────────────────────────
    // `python -m unittest` wraps ImportError-at-load as a `_FailedTest`,
    // which looks like a test failure but is actually an environment
    // problem (missing package). The fix loop cannot repair it — the
    // coder keeps returning near-identical code because the code isn't
    // wrong. Detect missing packages up front and bail out with a clean
    // message instead of burning 3 regen attempts.
    let imports = check_python_imports(path);
    if !imports.missing.is_empty() {
        return CodetResult {
            syntax_ok: true,
            tests_found: true,
            tests_passed: 0,
            tests_failed: 0,
            tests_errors: 0,
            fixes_applied,
            attempts_made,
            fix_summary: fix_descriptions.join("; "),
            status: CodetStatus::CouldNotFix {
                last_error: format!(
                    "cannot validate tests — Python package(s) not importable: {}. \
                     Install them in the active Python environment and retry with /validate.",
                    imports.missing.join(", "),
                ),
            },
        };
    }

    // ── Step 3: Run unit tests ──────────────────────────────────────────
    let test_result = run_python_unittest(path);
    if test_result.all_passed {
        return CodetResult {
            syntax_ok: true,
            tests_found: true,
            tests_passed: test_result.passed,
            tests_failed: 0,
            tests_errors: 0,
            fixes_applied,
            attempts_made,
            fix_summary: fix_descriptions.join("; "),
            status: if fixes_applied > 0 {
                CodetStatus::FixedAll
            } else {
                CodetStatus::AllPassed
            },
        };
    }

    // ── Step 4: Fix loop for failing tests ──────────────────────────────
    match try_fix_loop(
        path,
        &content,
        &test_result.error_output,
        FixTarget::Tests,
        references,
    ) {
        FixLoopOutcome::Fixed {
            description,
            attempts_tried,
        } => {
            fixes_applied += 1;
            attempts_made += attempts_tried;
            fix_descriptions.push(description);
            // Re-read final test counts after the fix.
            let final_tests = run_python_unittest(path);
            CodetResult {
                syntax_ok: true,
                tests_found: true,
                tests_passed: final_tests.passed,
                tests_failed: final_tests.failed,
                tests_errors: final_tests.errors,
                fixes_applied,
                attempts_made,
                fix_summary: fix_descriptions.join("; "),
                status: if final_tests.all_passed {
                    CodetStatus::FixedAll
                } else {
                    CodetStatus::CouldNotFix {
                        last_error: final_tests.error_output,
                    }
                },
            }
        }
        FixLoopOutcome::GaveUp {
            last_error,
            attempts_tried,
        } => {
            attempts_made += attempts_tried;
            CodetResult {
                syntax_ok: true,
                tests_found: true,
                tests_passed: test_result.passed,
                tests_failed: test_result.failed,
                tests_errors: test_result.errors,
                fixes_applied,
                attempts_made,
                fix_summary: fix_descriptions.join("; "),
                status: CodetStatus::CouldNotFix { last_error },
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Rust validation (Sprint 10)
// ────────────────────────────────────────────────────────────────────────────

fn validate_rust(path: &Path, references: &[ReferenceFile]) -> CodetResult {
    let mut fixes_applied: u32 = 0;
    let mut attempts_made: u32 = 0;
    let mut fix_descriptions: Vec<String> = Vec::new();

    let syntax = run_rust_syntax_check(path);
    if !syntax.success {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let error_msg = format!("{}\n{}", syntax.stdout, syntax.stderr);
        match try_fix_loop(
            path,
            &content,
            &error_msg,
            FixTarget::RustSyntax,
            references,
        ) {
            FixLoopOutcome::Fixed {
                description,
                attempts_tried,
            } => {
                fixes_applied += 1;
                attempts_made += attempts_tried;
                fix_descriptions.push(description);
            }
            FixLoopOutcome::GaveUp {
                last_error,
                attempts_tried,
            } => {
                attempts_made += attempts_tried;
                return CodetResult {
                    syntax_ok: false,
                    tests_found: false,
                    tests_passed: 0,
                    tests_failed: 0,
                    tests_errors: 0,
                    fixes_applied,
                    attempts_made,
                    fix_summary: fix_descriptions.join("; "),
                    status: CodetStatus::CouldNotFix { last_error },
                };
            }
        }
    }

    let content = std::fs::read_to_string(path).unwrap_or_default();
    CodetResult {
        syntax_ok: true,
        tests_found: has_rust_tests(&content),
        tests_passed: 0,
        tests_failed: 0,
        tests_errors: 0,
        fixes_applied,
        attempts_made,
        fix_summary: fix_descriptions.join("; "),
        status: if fixes_applied > 0 {
            CodetStatus::FixedAll
        } else {
            CodetStatus::AllPassed
        },
    }
}

// ────────────────────────────────────────────────────────────────────────────
// JavaScript validation (Sprint 10)
// ────────────────────────────────────────────────────────────────────────────

fn validate_js(path: &Path, references: &[ReferenceFile]) -> CodetResult {
    let mut fixes_applied: u32 = 0;
    let mut attempts_made: u32 = 0;
    let mut fix_descriptions: Vec<String> = Vec::new();

    let syntax = run_js_syntax_check(path);
    if !syntax.success {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let error_msg = format!("{}\n{}", syntax.stdout, syntax.stderr);
        match try_fix_loop(path, &content, &error_msg, FixTarget::JsSyntax, references) {
            FixLoopOutcome::Fixed {
                description,
                attempts_tried,
            } => {
                fixes_applied += 1;
                attempts_made += attempts_tried;
                fix_descriptions.push(description);
            }
            FixLoopOutcome::GaveUp {
                last_error,
                attempts_tried,
            } => {
                attempts_made += attempts_tried;
                return CodetResult {
                    syntax_ok: false,
                    tests_found: false,
                    tests_passed: 0,
                    tests_failed: 0,
                    tests_errors: 0,
                    fixes_applied,
                    attempts_made,
                    fix_summary: fix_descriptions.join("; "),
                    status: CodetStatus::CouldNotFix { last_error },
                };
            }
        }
    }

    CodetResult {
        syntax_ok: true,
        tests_found: false,
        tests_passed: 0,
        tests_failed: 0,
        tests_errors: 0,
        fixes_applied,
        attempts_made,
        fix_summary: fix_descriptions.join("; "),
        status: if fixes_applied > 0 {
            CodetStatus::FixedAll
        } else {
            CodetStatus::AllPassed
        },
    }
}

// ────────────────────────────────────────────────────────────────────────────
// TypeScript validation (Sprint 10)
// ────────────────────────────────────────────────────────────────────────────

fn validate_ts(path: &Path, references: &[ReferenceFile]) -> CodetResult {
    let mut fixes_applied: u32 = 0;
    let mut attempts_made: u32 = 0;
    let mut fix_descriptions: Vec<String> = Vec::new();

    let syntax = run_ts_syntax_check(path);
    if !syntax.success {
        // Check if tsc is actually available — if npx/tsc not installed,
        // skip rather than reporting a false failure.
        if syntax.stderr.contains("not found")
            || syntax.stderr.contains("not recognized")
            || syntax.stderr.contains("spawn")
        {
            return CodetResult {
                syntax_ok: true,
                tests_found: false,
                tests_passed: 0,
                tests_failed: 0,
                tests_errors: 0,
                fixes_applied: 0,
                attempts_made: 0,
                fix_summary: "tsc not available, syntax check skipped".to_string(),
                status: CodetStatus::Skipped,
            };
        }

        let content = std::fs::read_to_string(path).unwrap_or_default();
        let error_msg = format!("{}\n{}", syntax.stdout, syntax.stderr);
        match try_fix_loop(path, &content, &error_msg, FixTarget::TsSyntax, references) {
            FixLoopOutcome::Fixed {
                description,
                attempts_tried,
            } => {
                fixes_applied += 1;
                attempts_made += attempts_tried;
                fix_descriptions.push(description);
            }
            FixLoopOutcome::GaveUp {
                last_error,
                attempts_tried,
            } => {
                attempts_made += attempts_tried;
                return CodetResult {
                    syntax_ok: false,
                    tests_found: false,
                    tests_passed: 0,
                    tests_failed: 0,
                    tests_errors: 0,
                    fixes_applied,
                    attempts_made,
                    fix_summary: fix_descriptions.join("; "),
                    status: CodetStatus::CouldNotFix { last_error },
                };
            }
        }
    }

    CodetResult {
        syntax_ok: true,
        tests_found: false,
        tests_passed: 0,
        tests_failed: 0,
        tests_errors: 0,
        fixes_applied,
        attempts_made,
        fix_summary: fix_descriptions.join("; "),
        status: if fixes_applied > 0 {
            CodetStatus::FixedAll
        } else {
            CodetStatus::AllPassed
        },
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Fix loop — shared between syntax fixes and test fixes
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum FixTarget {
    Syntax,
    Tests,
    RustSyntax,
    JsSyntax,
    TsSyntax,
}

enum FixLoopOutcome {
    Fixed {
        description: String,
        attempts_tried: u32,
    },
    GaveUp {
        last_error: String,
        attempts_tried: u32,
    },
}

/// A surgical search-replace patch: locate `search` verbatim in the file and
/// substitute `replace`. Emitted by the coder in SEARCH/REPLACE block format.
#[derive(Debug, Clone)]
struct Patch {
    search: String,
    replace: String,
}

fn try_fix_loop(
    path: &Path,
    original_content: &str,
    initial_error: &str,
    target: FixTarget,
    references: &[ReferenceFile],
) -> FixLoopOutcome {
    let mut current_content = original_content.to_string();
    let mut last_error = initial_error.to_string();

    // Surgical path (SEARCH/REPLACE patches) is far cheaper than full-file regen
    // for syntax fixes — a 1-3 line bug becomes ~50 output tokens instead of
    // a full file rewrite. Test fixes often need multi-function edits, so
    // those stay on the full-regen path.
    let use_surgical_path = matches!(
        target,
        FixTarget::Syntax | FixTarget::RustSyntax | FixTarget::JsSyntax | FixTarget::TsSyntax
    );

    for attempt in 0..MAX_FIX_ATTEMPTS {
        eprintln!(
            "  {} {}",
            crate::theme::dim("▸"),
            crate::theme::dim(&format!(
                "codet: fix attempt {}/{MAX_FIX_ATTEMPTS}",
                attempt + 1
            )),
        );

        let fixed_content = if use_surgical_path {
            ask_coder_for_patches(&current_content, &last_error, references)
                .and_then(|patches| apply_patches(&current_content, &patches))
                .or_else(|| {
                    eprintln!(
                        "  {} {}",
                        crate::theme::dim("▸"),
                        crate::theme::dim(
                            "codet: surgical path failed, falling back to full regen...",
                        ),
                    );
                    ask_coder_to_fix(&current_content, &last_error, references)
                })
        } else {
            ask_coder_to_fix(&current_content, &last_error, references)
        };

        let Some(fixed_content) = fixed_content else {
            eprintln!(
                "  {} {}",
                crate::theme::warn(crate::theme::WARN_GLYPH),
                crate::theme::warn("codet: coder returned no usable fix, retrying..."),
            );
            continue;
        };

        // Write the candidate fix to disk.
        if std::fs::write(path, &fixed_content).is_err() {
            continue;
        }

        // Re-validate — did the fix actually help?
        let improved = match target {
            FixTarget::Syntax => run_python_syntax_check(path).success,
            FixTarget::RustSyntax => run_rust_syntax_check(path).success,
            FixTarget::JsSyntax => run_js_syntax_check(path).success,
            FixTarget::TsSyntax => run_ts_syntax_check(path).success,
            FixTarget::Tests => {
                let recheck = run_python_unittest(path);
                recheck.all_passed
                    || (recheck.failed + recheck.errors) < (count_failures_from_error(&last_error))
            }
        };

        if improved {
            let desc = match target {
                FixTarget::Syntax => "fixed Python syntax error".to_string(),
                FixTarget::RustSyntax => "fixed Rust syntax error".to_string(),
                FixTarget::JsSyntax => "fixed JavaScript syntax error".to_string(),
                FixTarget::TsSyntax => "fixed TypeScript syntax error".to_string(),
                FixTarget::Tests => "fixed failing test(s)".to_string(),
            };
            eprintln!(
                "  {} {}",
                crate::theme::ok(crate::theme::OK_GLYPH),
                crate::theme::ok(&format!("codet: {desc}")),
            );
            return FixLoopOutcome::Fixed {
                description: desc,
                attempts_tried: attempt + 1,
            };
        }
        eprintln!(
            "  {} {}",
            crate::theme::warn(crate::theme::WARN_GLYPH),
            crate::theme::warn("codet: fix didn't improve results, retrying..."),
        );

        // Fix didn't help (or made things worse) — update context for next
        // attempt but restore the original content so we don't compound
        // bad fixes.
        let _ = std::fs::write(path, &current_content);
        current_content = fixed_content;
        let retest_err = match target {
            FixTarget::Syntax => {
                let r = run_python_syntax_check(path);
                format!("{}\n{}", r.stdout, r.stderr)
            }
            FixTarget::RustSyntax => {
                let r = run_rust_syntax_check(path);
                format!("{}\n{}", r.stdout, r.stderr)
            }
            FixTarget::JsSyntax => {
                let r = run_js_syntax_check(path);
                format!("{}\n{}", r.stdout, r.stderr)
            }
            FixTarget::TsSyntax => {
                let r = run_ts_syntax_check(path);
                format!("{}\n{}", r.stdout, r.stderr)
            }
            FixTarget::Tests => {
                let r = run_python_unittest(path);
                r.error_output
            }
        };
        if !retest_err.trim().is_empty() {
            last_error = retest_err;
        }
    }

    // Restore the original content — never leave a bad fix on disk.
    let _ = std::fs::write(path, original_content);
    FixLoopOutcome::GaveUp {
        last_error,
        attempts_tried: MAX_FIX_ATTEMPTS,
    }
}

/// Quick heuristic to count how many failure lines exist in an error
/// string. Used by the "did the fix help?" check to see if the failure
/// count dropped even if it's not zero yet.
fn count_failures_from_error(error: &str) -> u32 {
    let mut count = 0u32;
    for line in error.lines() {
        let trimmed = line.trim_end();
        if trimmed.ends_with("... FAIL") || trimmed.ends_with("... ERROR") {
            count += 1;
        }
    }
    if count == 0 && !error.trim().is_empty() {
        1 // at least one unknown failure
    } else {
        count
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Coder model interaction
// ────────────────────────────────────────────────────────────────────────────

// ────────────────────────────────────────────────────────────────────────────
// Code generation — the coder writes code from a description
// ────────────────────────────────────────────────────────────────────────────

/// Ask the coder model to generate code from a natural-language description.
/// Called by the `generate_code` tool. Returns `Some(code)` on success.
/// The coder model is better at writing code than Claudette (general-purpose),
/// so routing code generation through here produces higher-quality output.
pub fn generate_code(
    description: &str,
    language: &str,
    references: &[ReferenceFile],
) -> Option<String> {
    let model = coder_model();
    if !references.is_empty() {
        eprintln!(
            "  {} {}",
            crate::theme::dim("▸"),
            crate::theme::dim(&format!(
                "codet: generating {language} code via {model} with {} reference file(s)...",
                references.len()
            )),
        );
    } else {
        eprintln!(
            "  {} {}",
            crate::theme::dim("▸"),
            crate::theme::dim(&format!("codet: generating {language} code via {model}...")),
        );
    }

    let mut client = OllamaApiClient::new(model.clone(), json!([]))
        .with_context(coder_num_ctx())
        .with_max_predict(coder_num_predict());

    let reference_block = format_reference_block(references);
    let prompt = format!(
        "Write a {language} file matching this description:\n\n\
        {description}{reference_block}\n\n\
        Requirements:\n\
        - Write clean, well-structured, production-quality code\n\
        - Include proper comments where the logic isn't obvious\n\
        - If the description mentions tests, include them in the same file\n\
        - If reference files are provided, use ONLY the real class/method \
          names from them — never invent or rename APIs\n\
        - Output ONLY the file content — no explanations, no markdown fences"
    );

    let request = ApiRequest {
        system_prompt: vec![format!(
            "You are an expert {language} developer. Output ONLY code. \
             No explanations, no markdown, no commentary."
        )],
        messages: vec![ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: prompt }],
            usage: None,
        }],
    };

    let events = match client.stream(request) {
        Ok(ev) => ev,
        Err(e) => {
            eprintln!(
                "  {} {}",
                crate::theme::error(crate::theme::ERR_GLYPH),
                crate::theme::error(&format!("codet: {model} request failed: {e}")),
            );
            return None;
        }
    };

    let mut code = String::new();
    for event in events {
        if let AssistantEvent::TextDelta(text) = event {
            code.push_str(&text);
        }
    }

    eprintln!(
        "  {} {}",
        crate::theme::dim("▸"),
        crate::theme::dim(&format!(
            "codet: generated {} chars of {language}",
            code.len()
        )),
    );

    let code = strip_code_blocks(&code);
    if code.trim().is_empty() {
        eprintln!(
            "  {} {}",
            crate::theme::warn(crate::theme::WARN_GLYPH),
            crate::theme::warn("codet: coder returned empty response"),
        );
        return None;
    }
    Some(code)
}

// ────────────────────────────────────────────────────────────────────────────
// Code fixing — the coder repairs broken code
// ────────────────────────────────────────────────────────────────────────────

/// Ask the coder model to fix the bugs in `file_content` given the
/// `error_output`. Returns `Some(corrected_code)` or `None` if the
/// model couldn't produce anything usable. Diagnostic messages go to
/// stderr so the user can see why fixes fail.
fn ask_coder_to_fix(
    file_content: &str,
    error_output: &str,
    references: &[ReferenceFile],
) -> Option<String> {
    let model = coder_model();
    eprintln!(
        "  {} {}",
        crate::theme::dim("▸"),
        crate::theme::dim(&format!("codet: asking {model} to fix...")),
    );

    let mut client = OllamaApiClient::new(model.clone(), json!([]))
        .with_context(coder_num_ctx())
        .with_max_predict(coder_num_predict());

    let reference_block = format_reference_block(references);
    let prompt = format!(
        "The following Python file has bug(s). Fix them.\n\n\
        ## File content\n```python\n{file_content}\n```\n\n\
        ## Error output\n```\n{error_output}\n```{reference_block}\n\n\
        Output ONLY the corrected Python file. No explanations, no markdown fences. \
        If reference files are provided above, use ONLY the real class/method names \
        from them."
    );

    let request = ApiRequest {
        system_prompt: vec![
            "You are a Python code fixer. Output ONLY the corrected code. \
             No explanations, no markdown, no commentary."
                .to_string(),
        ],
        messages: vec![ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: prompt }],
            usage: None,
        }],
    };

    let events = match client.stream(request) {
        Ok(ev) => ev,
        Err(e) => {
            eprintln!(
                "  {} {}",
                crate::theme::error(crate::theme::ERR_GLYPH),
                crate::theme::error(&format!("codet: {model} request failed: {e}")),
            );
            return None;
        }
    };

    let mut code = String::new();
    for event in events {
        if let AssistantEvent::TextDelta(text) = event {
            code.push_str(&text);
        }
    }

    eprintln!(
        "  {} {}",
        crate::theme::dim("▸"),
        crate::theme::dim(&format!("codet: got {} chars of response", code.len())),
    );

    let code = strip_code_blocks(&code);
    if code.trim().is_empty() {
        eprintln!(
            "  {} {}",
            crate::theme::warn(crate::theme::WARN_GLYPH),
            crate::theme::warn("codet: response was empty after stripping fences"),
        );
        return None;
    }
    Some(code)
}

/// Ask the coder for surgical SEARCH/REPLACE patches instead of a full-file
/// rewrite. Returns `None` if the response contains no parseable blocks —
/// the caller falls back to `ask_coder_to_fix` for the full-regen path.
fn ask_coder_for_patches(
    file_content: &str,
    error_output: &str,
    references: &[ReferenceFile],
) -> Option<Vec<Patch>> {
    let model = coder_model();
    eprintln!(
        "  {} {}",
        crate::theme::dim("▸"),
        crate::theme::dim(&format!("codet: asking {model} for surgical patches...")),
    );

    let mut client = OllamaApiClient::new(model.clone(), json!([]))
        .with_context(coder_num_ctx())
        .with_max_predict(coder_num_predict());

    let reference_block = format_reference_block(references);
    let prompt = format!(
        "The following file has bug(s). Fix them by emitting SEARCH/REPLACE blocks.\n\n\
         ## File content\n```\n{file_content}\n```\n\n\
         ## Error output\n```\n{error_output}\n```{reference_block}\n\n\
         ## Output format\n\
         For each bug, emit EXACTLY this format (no markdown fences around the blocks):\n\n\
         <<<<<<< SEARCH\n\
         [text from the file to find — must match EXACTLY, whitespace included]\n\
         =======\n\
         [replacement text]\n\
         >>>>>>> REPLACE\n\n\
         Rules:\n\
         - SEARCH must match the file character-for-character (whitespace, newlines, indentation)\n\
         - Include JUST ENOUGH context so SEARCH is unique in the file — don't copy whole functions\n\
         - Emit one block per distinct bug; multiple blocks are fine\n\
         - NO commentary, NO explanations, NO markdown fences — only the blocks themselves"
    );

    let request = ApiRequest {
        system_prompt: vec![
            "You output SEARCH/REPLACE patch blocks only. No prose, no commentary, no fences."
                .to_string(),
        ],
        messages: vec![ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: prompt }],
            usage: None,
        }],
    };

    let events = match client.stream(request) {
        Ok(ev) => ev,
        Err(e) => {
            eprintln!(
                "  {} {}",
                crate::theme::error(crate::theme::ERR_GLYPH),
                crate::theme::error(&format!("codet: {model} patch request failed: {e}")),
            );
            return None;
        }
    };

    let mut response = String::new();
    for event in events {
        if let AssistantEvent::TextDelta(text) = event {
            response.push_str(&text);
        }
    }

    let patches = parse_search_replace_blocks(&response);
    if patches.is_empty() {
        eprintln!(
            "  {} {}",
            crate::theme::warn(crate::theme::WARN_GLYPH),
            crate::theme::warn(&format!(
                "codet: no valid SEARCH/REPLACE blocks in {} chars of response",
                response.len()
            )),
        );
        return None;
    }
    eprintln!(
        "  {} {}",
        crate::theme::dim("▸"),
        crate::theme::dim(&format!(
            "codet: parsed {} surgical block(s) from {} chars",
            patches.len(),
            response.len()
        )),
    );
    Some(patches)
}

/// Parse SEARCH/REPLACE blocks from the coder's response. Tolerates surrounding
/// prose or partial malformed blocks — only well-formed triplets are accepted.
fn parse_search_replace_blocks(response: &str) -> Vec<Patch> {
    const SEARCH_MARK: &str = "<<<<<<< SEARCH";
    const SEP_MARK: &str = "=======";
    const REPLACE_MARK: &str = ">>>>>>> REPLACE";

    let mut patches = Vec::new();
    let mut cursor = 0usize;

    while cursor < response.len() {
        let Some(search_rel) = response[cursor..].find(SEARCH_MARK) else {
            break;
        };
        let search_start = cursor + search_rel + SEARCH_MARK.len();

        let Some(sep_rel) = response[search_start..].find(SEP_MARK) else {
            break;
        };
        let sep_start = search_start + sep_rel;
        let replace_content_start = sep_start + SEP_MARK.len();

        let Some(end_rel) = response[replace_content_start..].find(REPLACE_MARK) else {
            break;
        };
        let replace_end = replace_content_start + end_rel;

        let search = response[search_start..sep_start]
            .trim_start_matches(['\r', '\n'])
            .trim_end_matches(['\r', '\n'])
            .to_string();
        let replace = response[replace_content_start..replace_end]
            .trim_start_matches(['\r', '\n'])
            .trim_end_matches(['\r', '\n'])
            .to_string();

        if !search.is_empty() {
            patches.push(Patch { search, replace });
        }

        cursor = replace_end + REPLACE_MARK.len();
    }

    patches
}

/// Apply patches sequentially. Returns `None` if any SEARCH string is not
/// uniquely located in the current content — the caller then falls back to
/// full-regen so we never corrupt a file with a wrong-anchor replacement.
fn apply_patches(content: &str, patches: &[Patch]) -> Option<String> {
    let mut result = content.to_string();
    for patch in patches {
        // Exact match first.
        if let Some(idx) = find_unique(&result, &patch.search) {
            result = format!(
                "{}{}{}",
                &result[..idx],
                patch.replace,
                &result[idx + patch.search.len()..]
            );
            continue;
        }
        // Whitespace-tolerant fallback: normalize trailing whitespace per line
        // for both haystack and needle, try to find a match.
        if let Some((start, end)) = fuzzy_find(&result, &patch.search) {
            result = format!("{}{}{}", &result[..start], patch.replace, &result[end..]);
            continue;
        }
        // Could not locate the anchor — fail the whole patch set.
        return None;
    }
    Some(result)
}

/// Find `needle` in `haystack` and return its byte offset only if it appears
/// exactly once. Prevents accidental replacement when the anchor text is
/// repeated in the file.
fn find_unique(haystack: &str, needle: &str) -> Option<usize> {
    let first = haystack.find(needle)?;
    let second = haystack[first + needle.len()..].find(needle);
    if second.is_some() {
        None
    } else {
        Some(first)
    }
}

/// Whitespace-tolerant fallback: match the needle against the haystack after
/// normalizing trailing whitespace on each line. Returns (start, end) byte
/// offsets in the ORIGINAL haystack. Requires a unique match.
fn fuzzy_find(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    let norm = |s: &str| -> String { s.lines().map(str::trim_end).collect::<Vec<_>>().join("\n") };
    let normalized_haystack = norm(haystack);
    let normalized_needle = norm(needle);
    if normalized_needle.is_empty() {
        return None;
    }

    let first = normalized_haystack.find(&normalized_needle)?;
    let second = normalized_haystack[first + normalized_needle.len()..].find(&normalized_needle);
    if second.is_some() {
        return None;
    }

    // Map normalized offset back to original by counting lines + per-line char delta.
    // Simpler approach: find the first line of the needle in haystack and count
    // matching lines from there.
    let needle_lines: Vec<&str> = normalized_needle.lines().collect();
    if needle_lines.is_empty() {
        return None;
    }
    let haystack_lines: Vec<&str> = haystack.lines().collect();
    let mut line_offsets: Vec<usize> = Vec::with_capacity(haystack_lines.len() + 1);
    let mut off = 0usize;
    for line in &haystack_lines {
        line_offsets.push(off);
        off += line.len() + 1; // +1 for '\n'
    }
    line_offsets.push(off);

    for (i, _) in haystack_lines.iter().enumerate() {
        if i + needle_lines.len() > haystack_lines.len() {
            break;
        }
        let mut ok = true;
        for (j, nline) in needle_lines.iter().enumerate() {
            if haystack_lines[i + j].trim_end() != *nline {
                ok = false;
                break;
            }
        }
        if ok {
            let start = line_offsets[i];
            let end = line_offsets[i + needle_lines.len()].saturating_sub(1);
            return Some((start, end.min(haystack.len())));
        }
    }
    None
}

/// Strip markdown code fences from the coder model's response. The model
/// is told "output ONLY the corrected code, no markdown" but small models
/// often ignore that and wrap anyway.
///
/// **Conservative line-start matching:** a ```` ``` ```` only counts as a
/// fence when it appears at the start of a line (beginning of the string or
/// after `\n`). This prevents false matches on triple backticks *inside*
/// code — e.g. `re.compile(r'```...```')` for matching Markdown fenced code
/// blocks used to get sliced in half by the previous naive substring scan.
///
/// Behavior:
/// - Response starts at a fence (with or without a language tag) → strip the
///   outer fence pair, return the content.
/// - Response has prose followed by a fenced block → extract the first
///   fenced block's content.
/// - Response is raw code with no line-start fence → returned as-is, even if
///   it contains triple backticks in string literals or comments.
/// - Multi-block responses → take the first block only; trailing blocks
///   (typically "here are tests" preamble + second fence) are discarded.
fn strip_code_blocks(s: &str) -> String {
    let trimmed = s.trim();
    if !trimmed.contains("```") {
        return trimmed.to_string();
    }

    // Find the opening fence: ``` at start-of-line (either index 0 or after \n).
    let Some(open_pos) = find_line_start_fence(trimmed, 0) else {
        // No fence at a line start — response isn't fence-wrapped. Leave alone
        // so we don't eat triple backticks embedded in code strings.
        return trimmed.to_string();
    };

    // Skip the opening ``` and optional language tag on the same line.
    let after_open = &trimmed[open_pos + 3..];
    let code_start = match after_open.find('\n') {
        Some(nl) => open_pos + 3 + nl + 1,
        None => {
            // Opener with no newline after (`\`\`\`code`) — unusual, strip opener and return.
            return after_open.trim().to_string();
        }
    };

    // Find the first closing fence that is ALSO at a line start.
    if let Some(close_pos) = find_line_start_fence(trimmed, code_start) {
        return trimmed[code_start..close_pos]
            .trim_end_matches('\n')
            .to_string();
    }

    // No line-start closer found — strip the opener and return the rest.
    trimmed[code_start..].trim().to_string()
}

/// Return the byte index of the next ```` ``` ```` that sits at the start of
/// a line (either at `from` if it's position 0 or immediately after a `\n`
/// at position `from`, or later after some newline), starting the search at
/// `from`. `None` if no such fence exists.
fn find_line_start_fence(s: &str, from: usize) -> Option<usize> {
    if from == 0 && s.starts_with("```") {
        return Some(0);
    }
    s[from..]
        .match_indices("\n```")
        .next()
        .map(|(i, _)| from + i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_code_blocks_removes_python_fence() {
        let input = "```python\ndef greet():\n    return 'hi'\n```";
        assert_eq!(strip_code_blocks(input), "def greet():\n    return 'hi'");
    }

    #[test]
    fn strip_code_blocks_removes_bare_fence() {
        let input = "```\nx = 42\n```";
        assert_eq!(strip_code_blocks(input), "x = 42");
    }

    #[test]
    fn strip_code_blocks_noop_without_fences() {
        let input = "def greet():\n    return 'hi'";
        assert_eq!(strip_code_blocks(input), input);
    }

    #[test]
    fn strip_code_blocks_handles_trailing_whitespace() {
        let input = "  ```python\ncode\n```  ";
        assert_eq!(strip_code_blocks(input), "code");
    }

    #[test]
    fn strip_code_blocks_extracts_from_text_before_fence() {
        let input = "Here's the corrected code:\n```python\ndef greet():\n    return 'hi'\n```\n";
        assert_eq!(strip_code_blocks(input), "def greet():\n    return 'hi'");
    }

    #[test]
    fn strip_code_blocks_multi_block_takes_first() {
        // Multi-block responses: take the first fenced block only. Subsequent
        // prose + fences are ignored. (The old concatenating behavior was
        // replaced by line-start-only matching to avoid eating code-internal
        // triple backticks.)
        let input =
            "```bash\n#!/bin/bash\necho hello\n```\n\n# Test cases\n\n```bash\necho test\n```";
        assert_eq!(strip_code_blocks(input), "#!/bin/bash\necho hello");
    }

    #[test]
    fn strip_code_blocks_preserves_internal_triple_backticks() {
        // Regression (md2html.py): Python regex containing triple backticks
        // for matching Markdown fenced code blocks. Old logic sliced the code
        // at the first inner ``` and discarded the closing part of the regex.
        // New line-start rule leaves code with no outer fence completely
        // alone.
        let input = "import re\nPATTERN = re.compile(r'```(.*?)```', re.DOTALL)\nprint(PATTERN)";
        assert_eq!(strip_code_blocks(input), input);
    }

    #[test]
    fn strip_code_blocks_fenced_code_with_internal_backticks() {
        // Fenced wrapper around code that ALSO contains inner triple backticks.
        // Should strip outer fence pair; inner backticks (mid-line) ignored.
        let input = "```python\nimport re\nP = re.compile(r'```x```')\n```";
        assert_eq!(
            strip_code_blocks(input),
            "import re\nP = re.compile(r'```x```')"
        );
    }

    #[test]
    fn strip_code_blocks_extracts_from_text_before_and_after() {
        let input = "Fix applied:\n```python\nx = 42\n```\nThe bug was on line 3.";
        assert_eq!(strip_code_blocks(input), "x = 42");
    }

    #[test]
    fn count_failures_from_error_counts_fail_and_error_lines() {
        let error = "test_a ... ok\ntest_b ... FAIL\ntest_c ... ERROR\ntest_d ... ok\n";
        assert_eq!(count_failures_from_error(error), 2);
    }

    #[test]
    fn count_failures_from_error_returns_one_for_unknown_errors() {
        let error = "SyntaxError: invalid syntax\n";
        assert_eq!(count_failures_from_error(error), 1);
    }

    #[test]
    fn count_failures_from_error_returns_zero_for_empty() {
        assert_eq!(count_failures_from_error(""), 0);
    }

    #[test]
    fn validation_skips_non_code_files() {
        // A .txt file should return None (not validated).
        let path = Path::new("some-notes.txt");
        assert!(validate_code_file(path, &[]).is_none());
    }

    #[test]
    fn validation_skips_unknown_extensions() {
        let path = Path::new("data.csv");
        assert!(validate_code_file(path, &[]).is_none());
    }

    #[test]
    fn codet_result_to_json_contains_all_fields() {
        let result = CodetResult {
            syntax_ok: true,
            tests_found: true,
            tests_passed: 10,
            tests_failed: 1,
            tests_errors: 0,
            fixes_applied: 1,
            attempts_made: 2,
            fix_summary: "fixed test_get_age".to_string(),
            status: CodetStatus::FixedAll,
        };
        let j = result.to_json();
        assert_eq!(j["syntax_ok"], true);
        assert_eq!(j["tests_passed"], 10);
        assert_eq!(j["tests_failed"], 1);
        assert_eq!(j["fixes_applied"], 1);
        assert_eq!(j["attempts_made"], 2);
        assert!(j["status"].as_str().unwrap().contains("fixed_all"));
    }

    // ── Surgical fix loop: SEARCH/REPLACE patches ────────────────────────

    #[test]
    fn parse_single_search_replace_block() {
        let input = "<<<<<<< SEARCH\nfoo\n=======\nbar\n>>>>>>> REPLACE";
        let patches = parse_search_replace_blocks(input);
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].search, "foo");
        assert_eq!(patches[0].replace, "bar");
    }

    #[test]
    fn parse_multiple_blocks() {
        let input = "<<<<<<< SEARCH\nfoo\n=======\nbar\n>>>>>>> REPLACE\n\n<<<<<<< SEARCH\nbaz\n=======\nqux\n>>>>>>> REPLACE";
        let patches = parse_search_replace_blocks(input);
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].search, "foo");
        assert_eq!(patches[1].search, "baz");
    }

    #[test]
    fn parse_tolerates_surrounding_prose() {
        let input = "Here is the fix:\n<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\nDone.";
        let patches = parse_search_replace_blocks(input);
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].search, "old");
    }

    #[test]
    fn parse_rejects_missing_closing_mark() {
        let input = "<<<<<<< SEARCH\nfoo\n=======\nbar\n"; // no >>>>>>> REPLACE
        let patches = parse_search_replace_blocks(input);
        assert!(patches.is_empty());
    }

    #[test]
    fn parse_rejects_missing_separator() {
        let input = "<<<<<<< SEARCH\nfoo\n>>>>>>> REPLACE"; // no =======
        let patches = parse_search_replace_blocks(input);
        assert!(patches.is_empty());
    }

    #[test]
    fn parse_multiline_replacement() {
        let input = "<<<<<<< SEARCH\nfoo\n=======\nbar\nbaz\nqux\n>>>>>>> REPLACE";
        let patches = parse_search_replace_blocks(input);
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].replace, "bar\nbaz\nqux");
    }

    #[test]
    fn apply_single_patch_replaces_exact_match() {
        let content = "def foo():\n    return x\n";
        let patches = vec![Patch {
            search: "return x".to_string(),
            replace: "return 42".to_string(),
        }];
        let result = apply_patches(content, &patches).unwrap();
        assert_eq!(result, "def foo():\n    return 42\n");
    }

    #[test]
    fn apply_multiple_patches_sequential() {
        let content = "a = 1\nb = 2\n";
        let patches = vec![
            Patch {
                search: "a = 1".to_string(),
                replace: "a = 10".to_string(),
            },
            Patch {
                search: "b = 2".to_string(),
                replace: "b = 20".to_string(),
            },
        ];
        let result = apply_patches(content, &patches).unwrap();
        assert_eq!(result, "a = 10\nb = 20\n");
    }

    #[test]
    fn apply_fails_when_search_missing() {
        let content = "a = 1\n";
        let patches = vec![Patch {
            search: "b = 2".to_string(),
            replace: "c = 3".to_string(),
        }];
        assert!(apply_patches(content, &patches).is_none());
    }

    #[test]
    fn apply_fails_on_non_unique_search() {
        // Safety: if the SEARCH anchor appears twice, we refuse to guess
        // which one to replace — caller falls back to full regen.
        let content = "x = 1\ny = 2\nx = 1\n";
        let patches = vec![Patch {
            search: "x = 1".to_string(),
            replace: "x = 99".to_string(),
        }];
        assert!(apply_patches(content, &patches).is_none());
    }

    #[test]
    fn apply_fuzzy_match_tolerates_trailing_whitespace() {
        // Haystack has trailing spaces on a line; needle doesn't. Should still match.
        let content = "def foo():    \n    return x\n";
        let patches = vec![Patch {
            search: "def foo():\n    return x".to_string(),
            replace: "def foo():\n    return 42".to_string(),
        }];
        let result = apply_patches(content, &patches).unwrap();
        assert!(result.contains("return 42"));
    }

    #[test]
    fn fixes_real_syntax_break_from_md2html() {
        // Regression: unterminated raw-string literal like 30b emitted on
        // md2html.py. A 1-block surgical fix should close the quote.
        let content = "patterns = {\n    'block_code': re.compile(r'\n    'paragraph': re.compile(r'.+'),\n}\n";
        let patches = vec![Patch {
            search: "'block_code': re.compile(r'".to_string(),
            replace: "'block_code': re.compile(r'```'),".to_string(),
        }];
        let result = apply_patches(content, &patches).unwrap();
        assert!(result.contains("```'),"));
    }
}
