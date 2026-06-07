//! Patch group — `apply_patch` (sprint v0.6.0 Phase 3.1c). Lives in the
//! Quality group alongside `run_tests` and `diagnostics`.
//!
//! Accepts a multi-file unified diff and applies every hunk atomically:
//! either every hunk lands and the on-disk files are rewritten, or
//! nothing changes and the caller gets a per-hunk error report. The
//! `dry_run` flag stops at the staging step and emits the same report.
//!
//! Intentionally minimal — only standard unified diffs (`--- a/path`,
//! `+++ b/path`, `@@ -L,N +L,N @@`) with context/`-`/`+` lines. No fuzz
//! matching, no renames, no binary patches. The aim is to be a safer
//! `edit_file` replacement for multi-line edits where the brain emits a
//! diff already (Claude Code and Aider both do this).
//!
//! Path safety: paths are validated through `super::validate_edit_path`
//! the same way `edit_file` does — $HOME-gated in the interactive secretary,
//! but confined to the mission tree while a forge/brownfield mission is
//! active (roast RC-B), so the autonomous Coder can't patch files outside it.

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use super::{parse_json_input, validate_edit_path};

/// Lossless usize → i64 conversion; we only ever apply this to
/// `Vec::len()` of small in-memory line buffers, so the cast cannot
/// truthfully overflow in practice but clippy's pedantic-cast lint
/// flags it. Keep the helper local so we don't sprinkle `as i64` casts.
fn ilen(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

pub(super) fn schemas() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "apply_patch",
            "description": "Apply a unified diff atomically (all hunks must apply or nothing changes). Multi-file. Set `dry_run` to validate without writing. Standard `--- a/path` / `+++ b/path` / `@@` format only — no renames or binary patches.",
            "parameters": {
                "type": "object",
                "properties": {
                    "diff":    { "type": "string", "description": "The unified diff text to apply." },
                    "dry_run": { "type": "boolean", "description": "If true, validate every hunk without writing. Default false." }
                },
                "required": ["diff"]
            }
        }
    })]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "apply_patch" => run_apply_patch(input),
        _ => return None,
    };
    Some(result)
}

#[derive(Debug, Clone)]
struct Hunk {
    /// One-based line in the original file where the hunk starts (the `-L`
    /// value from `@@ -L,N +L,N @@`).
    old_start: usize,
    /// `old` and `new` are the literal context+deletion and context+addition
    /// lines, in order, **without** the leading marker char.
    old_lines: Vec<String>,
    new_lines: Vec<String>,
}

#[derive(Debug, Clone)]
struct FileDiff {
    path: String,
    hunks: Vec<Hunk>,
}

fn run_apply_patch(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "apply_patch")?;
    let diff = v
        .get("diff")
        .and_then(Value::as_str)
        .ok_or("apply_patch: missing 'diff'")?;
    let dry_run = v.get("dry_run").and_then(Value::as_bool).unwrap_or(false);

    if diff.trim().is_empty() {
        return Err("apply_patch: 'diff' is empty".to_string());
    }

    let files = parse_diff(diff)?;
    if files.is_empty() {
        return Err(
            "apply_patch: no files in diff (expected '--- a/path' / '+++ b/path' headers)"
                .to_string(),
        );
    }

    // Stage every file's new contents in memory first so an error on the
    // tenth hunk doesn't leave the first nine on disk. `pending` maps the
    // validated path → the would-be new content.
    let mut pending: Vec<(PathBuf, String)> = Vec::with_capacity(files.len());
    let mut applied_hunks: Vec<Value> = Vec::new();

    for file in &files {
        let path = validate_edit_path(&file.path)
            .map_err(|e| format!("apply_patch: {}: {e}", file.path))?;
        let original = fs::read_to_string(&path)
            .map_err(|e| format!("apply_patch: read {} failed: {e}", path.display()))?;
        let (new_content, hunks_applied) = apply_hunks(&original, &file.hunks)
            .map_err(|e| format!("apply_patch: {}: {e}", file.path))?;
        applied_hunks.push(json!({
            "path": file.path.clone(),
            "hunks": hunks_applied,
        }));
        pending.push((path, new_content));
    }

    // Atomic write — write a sibling tmp for each file, then rename. If
    // any rename fails partway through we can't fully roll back, but the
    // dry_run path lets the caller validate first, which is the bigger
    // win over a real two-phase commit.
    if !dry_run {
        for (path, content) in &pending {
            let tmp = path.with_extension("claudette-apply.tmp");
            fs::write(&tmp, content)
                .map_err(|e| format!("apply_patch: write tmp {} failed: {e}", tmp.display()))?;
            fs::rename(&tmp, path).map_err(|e| {
                let _ = fs::remove_file(&tmp);
                format!("apply_patch: rename to {} failed: {e}", path.display())
            })?;
        }
    }

    Ok(json!({
        "ok": true,
        "dry_run": dry_run,
        "files": applied_hunks,
    })
    .to_string())
}

/// Parse a multi-file unified diff into per-file hunk lists.
///
/// We accept (and ignore) `diff --git` headers and the `index ...` line
/// that git emits. Path detection looks for the `+++ b/path` line — the
/// `b/` prefix is stripped if present.
fn parse_diff(diff: &str) -> Result<Vec<FileDiff>, String> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_hunks: Vec<Hunk> = Vec::new();
    let mut current_hunk: Option<Hunk> = None;

    let flush_file = |files: &mut Vec<FileDiff>,
                      current_path: &mut Option<String>,
                      current_hunks: &mut Vec<Hunk>,
                      current_hunk: &mut Option<Hunk>| {
        if let Some(h) = current_hunk.take() {
            current_hunks.push(h);
        }
        if let Some(p) = current_path.take() {
            files.push(FileDiff {
                path: p,
                hunks: std::mem::take(current_hunks),
            });
        }
    };

    for line in diff.lines() {
        if line.starts_with("+++ ") {
            // New file. Flush any pending hunks under the previous path.
            flush_file(
                &mut files,
                &mut current_path,
                &mut current_hunks,
                &mut current_hunk,
            );
            let raw = line.trim_start_matches("+++ ").trim();
            let path = raw.strip_prefix("b/").unwrap_or(raw).to_string();
            if path == "/dev/null" {
                return Err(format!("file deletion not supported: {line}"));
            }
            current_path = Some(path);
        } else if line.starts_with("--- ") {
            // Old-file header. We don't need the path (we pull it from
            // `+++ b/...`) but skipping it cleanly closes any hunk we
            // were accumulating, so flush.
            if let Some(h) = current_hunk.take() {
                current_hunks.push(h);
            }
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            if current_path.is_none() {
                return Err(format!(
                    "found '@@' hunk header before any '+++ b/path' header: {line}"
                ));
            }
            if let Some(h) = current_hunk.take() {
                current_hunks.push(h);
            }
            // rest looks like `-L[,N] +L[,N] @@ optional_context`. We only
            // need `-L`; `N` is recoverable from the body and we trust
            // the body, not the header.
            let mut tokens = rest.split_whitespace();
            let old = tokens
                .next()
                .ok_or_else(|| format!("malformed hunk header: {line}"))?;
            // `-L,N` — strip the leading `-`, take the part before `,`.
            let old_loc = old.strip_prefix('-').unwrap_or(old);
            let old_start: usize = old_loc
                .split(',')
                .next()
                .unwrap_or("0")
                .parse()
                .map_err(|_| format!("malformed hunk header: {line}"))?;
            current_hunk = Some(Hunk {
                old_start: old_start.max(1),
                old_lines: Vec::new(),
                new_lines: Vec::new(),
            });
        } else if let Some(ref mut hunk) = current_hunk {
            match line.chars().next() {
                Some(' ') => {
                    // Context — keep on both sides.
                    let body = &line[1..];
                    hunk.old_lines.push(body.to_string());
                    hunk.new_lines.push(body.to_string());
                }
                Some('-') => {
                    if line.starts_with("--- ") {
                        // Another file header inside an active hunk — shouldn't
                        // happen, but be defensive.
                        continue;
                    }
                    hunk.old_lines.push(line[1..].to_string());
                }
                Some('+') => {
                    if line.starts_with("+++ ") {
                        continue;
                    }
                    hunk.new_lines.push(line[1..].to_string());
                }
                Some('\\') => {
                    // `\ No newline at end of file` marker — ignore. We treat
                    // every file as if it has a trailing newline so the
                    // round-trip handles standard editor output uniformly.
                }
                _ => {
                    // Blank line in mid-hunk (some tools emit these). Treat
                    // as a blank context line.
                    hunk.old_lines.push(String::new());
                    hunk.new_lines.push(String::new());
                }
            }
        }
        // Lines outside any hunk (commit message, "diff --git", "index ...",
        // etc.) are silently ignored.
    }

    // Flush the trailing in-progress file/hunk.
    flush_file(
        &mut files,
        &mut current_path,
        &mut current_hunks,
        &mut current_hunk,
    );

    Ok(files)
}

/// Apply every hunk to `original`. Returns the new content plus a per-hunk
/// summary. Bails on the first hunk that doesn't apply (caller treats
/// this as "no changes were written").
fn apply_hunks(original: &str, hunks: &[Hunk]) -> Result<(String, Vec<Value>), String> {
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    let mut summary: Vec<Value> = Vec::new();

    // Apply hunks in order. Track a `drift` (delta between hunk header
    // line numbers and current file line numbers) so subsequent hunks
    // line up even after earlier ones added/removed lines.
    let mut drift: i64 = 0;

    for (idx, hunk) in hunks.iter().enumerate() {
        let expected_start = ilen(hunk.old_start) + drift - 1; // zero-based
        if expected_start < 0 || usize::try_from(expected_start).unwrap_or(usize::MAX) > lines.len()
        {
            return Err(format!(
                "hunk {} at line {} is outside the file (have {} lines)",
                idx + 1,
                hunk.old_start,
                lines.len()
            ));
        }
        let start_idx = usize::try_from(expected_start).unwrap_or(0);
        let end_idx = start_idx + hunk.old_lines.len();
        if end_idx > lines.len() {
            return Err(format!(
                "hunk {} at line {} extends past EOF (need {} lines, have {})",
                idx + 1,
                hunk.old_start,
                hunk.old_lines.len(),
                lines.len() - start_idx
            ));
        }
        for (offset, expected) in hunk.old_lines.iter().enumerate() {
            let actual = &lines[start_idx + offset];
            if actual != expected {
                return Err(format!(
                    "hunk {} context mismatch at line {} (expected {:?}, got {:?})",
                    idx + 1,
                    hunk.old_start + offset,
                    expected,
                    actual,
                ));
            }
        }
        // Splice: replace old_lines.len() lines with new_lines.
        lines.splice(start_idx..end_idx, hunk.new_lines.iter().cloned());
        drift += ilen(hunk.new_lines.len()) - ilen(hunk.old_lines.len());
        summary.push(json!({
            "hunk": idx + 1,
            "line": hunk.old_start,
            "removed": hunk.old_lines.len(),
            "added": hunk.new_lines.len(),
        }));
    }

    // Preserve the file's dominant line ending. `original.lines()` above
    // stripped every '\r', so re-joining with bare "\n" silently rewrote a
    // CRLF file (Windows default) to LF — one tiny patch produced a whole-file
    // diff. Re-encode with the EOL the file actually uses. (roast 2026-06-02)
    let eol = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let trailing_newline = original.ends_with('\n');
    let mut out = lines.join(eol);
    if trailing_newline {
        out.push_str(eol);
    }
    Ok((out, summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn home_join(label: &str) -> String {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("{home}/claudette-patch-{label}-{nanos}.txt")
    }

    #[test]
    fn schemas_lists_one_tool() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 1);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["apply_patch"]);
    }

    #[test]
    fn apply_patch_rejects_missing_diff() {
        let err = run_apply_patch("{}").unwrap_err();
        assert!(err.contains("missing 'diff'"), "got: {err}");
    }

    #[test]
    fn apply_patch_rejects_empty_diff() {
        let err = run_apply_patch(r#"{"diff":""}"#).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn apply_patch_rejects_diff_without_file_header() {
        let err = run_apply_patch(r#"{"diff":"just some text\nno headers\n"}"#).unwrap_err();
        assert!(err.contains("no files in diff"), "got: {err}");
    }

    #[test]
    fn parse_diff_extracts_b_prefix_path() {
        let diff = "--- a/foo.txt\n+++ b/foo.txt\n@@ -1,1 +1,1 @@\n-old\n+new\n";
        let files = parse_diff(diff).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "foo.txt");
    }

    #[test]
    fn parse_diff_supports_multi_file() {
        let diff = "--- a/one.txt\n+++ b/one.txt\n@@ -1,1 +1,1 @@\n-a\n+A\n\
                    --- a/two.txt\n+++ b/two.txt\n@@ -1,1 +1,1 @@\n-b\n+B\n";
        let files = parse_diff(diff).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "one.txt");
        assert_eq!(files[1].path, "two.txt");
    }

    #[test]
    fn apply_hunks_replaces_single_line() {
        let original = "alpha\nbeta\ngamma\n";
        let hunk = Hunk {
            old_start: 2,
            old_lines: vec!["beta".to_string()],
            new_lines: vec!["BETA".to_string()],
        };
        let (out, summary) = apply_hunks(original, &[hunk]).unwrap();
        assert_eq!(out, "alpha\nBETA\ngamma\n");
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0]["removed"], 1);
        assert_eq!(summary[0]["added"], 1);
    }

    #[test]
    fn apply_hunks_preserves_crlf_line_endings() {
        // Regression (roast 2026-06-02): apply_hunks rewrote CRLF files to LF,
        // turning a one-line patch into a whole-file diff on Windows.
        let original = "alpha\r\nbeta\r\ngamma\r\n";
        let hunk = Hunk {
            old_start: 2,
            old_lines: vec!["beta".to_string()],
            new_lines: vec!["BETA".to_string()],
        };
        let (out, _) = apply_hunks(original, &[hunk]).unwrap();
        assert_eq!(out, "alpha\r\nBETA\r\ngamma\r\n");
        // No bare LF survived the round-trip.
        assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
    }

    #[test]
    fn apply_hunks_handles_drift_between_hunks() {
        let original = "1\n2\n3\n4\n5\n6\n";
        // First hunk inserts a line at the top — second hunk's line numbers
        // refer to the pre-edit positions.
        let h1 = Hunk {
            old_start: 1,
            old_lines: vec!["1".to_string()],
            new_lines: vec!["1".to_string(), "1.5".to_string()],
        };
        let h2 = Hunk {
            old_start: 5,
            old_lines: vec!["5".to_string()],
            new_lines: vec!["FIVE".to_string()],
        };
        let (out, _) = apply_hunks(original, &[h1, h2]).unwrap();
        assert_eq!(out, "1\n1.5\n2\n3\n4\nFIVE\n6\n");
    }

    #[test]
    fn apply_hunks_errors_on_context_mismatch() {
        let original = "alpha\nbeta\n";
        let hunk = Hunk {
            old_start: 1,
            old_lines: vec!["WRONG".to_string()],
            new_lines: vec!["NEW".to_string()],
        };
        let err = apply_hunks(original, &[hunk]).unwrap_err();
        assert!(err.contains("context mismatch"), "got: {err}");
    }

    #[test]
    fn apply_hunks_errors_on_past_eof() {
        let original = "only one line\n";
        let hunk = Hunk {
            old_start: 5,
            old_lines: vec!["x".to_string()],
            new_lines: vec!["y".to_string()],
        };
        let err = apply_hunks(original, &[hunk]).unwrap_err();
        assert!(
            err.contains("outside the file") || err.contains("past EOF"),
            "got: {err}"
        );
    }

    #[test]
    fn apply_patch_dry_run_does_not_touch_disk() {
        let _eg = crate::test_env_lock(); // home-resolving: serialize vs temp-home swaps
        let path = home_join("dryrun");
        fs::write(&path, "alpha\nbeta\n").unwrap();

        let diff = format!("--- a/{path}\n+++ b/{path}\n@@ -1,1 +1,1 @@\n-alpha\n+ALPHA\n");
        let input = json!({ "diff": diff, "dry_run": true }).to_string();
        let result = run_apply_patch(&input);
        let after = fs::read_to_string(&path).ok();
        let _ = fs::remove_file(&path);
        assert!(result.is_ok(), "got: {result:?}");
        assert_eq!(
            after.as_deref(),
            Some("alpha\nbeta\n"),
            "dry_run must not modify the file"
        );
    }

    #[test]
    fn apply_patch_writes_when_not_dry_run() {
        let _eg = crate::test_env_lock(); // home-resolving
        let path = home_join("write");
        fs::write(&path, "alpha\nbeta\n").unwrap();

        let diff = format!("--- a/{path}\n+++ b/{path}\n@@ -1,2 +1,2 @@\n alpha\n-beta\n+BETA\n");
        let input = json!({ "diff": diff }).to_string();
        let result = run_apply_patch(&input);
        let after = fs::read_to_string(&path).ok();
        let _ = fs::remove_file(&path);
        assert!(result.is_ok(), "got: {result:?}");
        assert_eq!(after.as_deref(), Some("alpha\nBETA\n"));
    }

    #[test]
    fn apply_patch_is_atomic_across_files() {
        let _eg = crate::test_env_lock(); // home-resolving
        let path_good = home_join("atomic-good");
        let path_bad = home_join("atomic-bad");
        fs::write(&path_good, "alpha\n").unwrap();
        fs::write(&path_bad, "this is wrong\n").unwrap();

        // First hunk would succeed; second targets nonexistent context, so
        // the whole apply must roll back and leave path_good unchanged.
        let diff = format!(
            "--- a/{path_good}\n+++ b/{path_good}\n@@ -1,1 +1,1 @@\n-alpha\n+ALPHA\n\
             --- a/{path_bad}\n+++ b/{path_bad}\n@@ -1,1 +1,1 @@\n-WRONG\n+RIGHT\n"
        );
        let input = json!({ "diff": diff }).to_string();
        let result = run_apply_patch(&input);
        let after_good = fs::read_to_string(&path_good).ok();
        let after_bad = fs::read_to_string(&path_bad).ok();
        let _ = fs::remove_file(&path_good);
        let _ = fs::remove_file(&path_bad);
        assert!(result.is_err(), "expected atomic failure: {result:?}");
        // Good file must NOT have been touched — that's the atomic guarantee.
        assert_eq!(after_good.as_deref(), Some("alpha\n"));
        assert_eq!(after_bad.as_deref(), Some("this is wrong\n"));
    }
}
