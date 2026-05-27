//! Fuzzy patch group — `apply_diff` (ported from Beast `beast-tools::fuzzy_patch`).
//!
//! The looser cousin of `apply_patch`. Where `apply_patch` (see `patch.rs`)
//! demands a byte-exact unified diff and rejects on any context drift,
//! `apply_diff` takes a `before` block and an `after` block and swaps the
//! first occurrence — falling back to a whitespace-tolerant line-trim match
//! when the exact block isn't found. This is the edit primitive LLMs are
//! reliable at: they reproduce the *shape* of a block but routinely get the
//! indentation, trailing whitespace, or `\r\n` vs `\n` slightly wrong, which
//! makes strict unified-diff application fail almost every time.
//!
//! Two passes:
//! 1. **Exact** — `content.match_indices(before)`. Byte-for-byte. Errors if
//!    the block appears in more than one place (ambiguous — widen `before`).
//! 2. **Line-trim** — split both sides into lines, find the contiguous
//!    window whose trimmed lines match `before`'s trimmed lines in order,
//!    and replace the full original window (preserving the file's original
//!    line endings outside the replaced region).
//!
//! Path safety: paths are validated through `super::validate_read_path` the
//! same way `apply_patch`/`edit_file` do, so we can't escape the `$HOME` /
//! `CLAUDETTE_WORKSPACE` sandbox.

use std::fs;

use serde_json::{json, Value};

use super::{parse_json_input, validate_read_path};

#[derive(Debug, Clone, PartialEq, Eq)]
enum FuzzyError {
    NotFound,
    EmptyBefore,
    Ambiguous,
}

pub(super) fn schemas() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "apply_diff",
            "description": "Replace a `before` block with an `after` block inside `path`. Whitespace-drift tolerant: exact match first, then a line-trim fallback that ignores indentation / trailing-whitespace / CRLF differences. Prefer this over `apply_patch` for targeted edits — it succeeds where a strict unified diff fails on context drift. The `before` block must be unique in the file (widen it with more surrounding lines if the call reports it is ambiguous).",
            "parameters": {
                "type": "object",
                "properties": {
                    "path":   { "type": "string", "description": "File to edit (inside the sandbox / active mission)." },
                    "before": { "type": "string", "description": "The exact block to find and replace. Must occur exactly once." },
                    "after":  { "type": "string", "description": "The replacement block." }
                },
                "required": ["path", "before", "after"]
            }
        }
    })]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "apply_diff" => run_apply_diff(input),
        _ => return None,
    };
    Some(result)
}

fn run_apply_diff(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "apply_diff")?;
    let raw_path = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("apply_diff: missing 'path'")?;
    let before = v
        .get("before")
        .and_then(Value::as_str)
        .ok_or("apply_diff: missing 'before'")?;
    let after = v
        .get("after")
        .and_then(Value::as_str)
        .ok_or("apply_diff: missing 'after'")?;

    let path = validate_read_path(raw_path).map_err(|e| format!("apply_diff: {raw_path}: {e}"))?;
    let original = fs::read_to_string(&path)
        .map_err(|e| format!("apply_diff: read {} failed: {e}", path.display()))?;

    match fuzzy_replace(&original, before, after) {
        Ok(new_content) => {
            // Atomic write via sibling tmp + rename, matching apply_patch.
            let tmp = path.with_extension("claudette-diff.tmp");
            fs::write(&tmp, &new_content)
                .map_err(|e| format!("apply_diff: write tmp {} failed: {e}", tmp.display()))?;
            fs::rename(&tmp, &path).map_err(|e| {
                let _ = fs::remove_file(&tmp);
                format!("apply_diff: rename to {} failed: {e}", path.display())
            })?;
            // Mirror the git tool's "▸" call logging so apply_diff usage is
            // visible on stderr (forge observability + harness detection).
            eprintln!(
                "  {} {}",
                crate::theme::dim("▸"),
                crate::theme::dim(&format!(
                    "apply_diff: {raw_path} ({} → {} bytes)",
                    original.len(),
                    new_content.len()
                )),
            );
            Ok(json!({
                "ok": true,
                "path": raw_path,
                "bytes_before": original.len(),
                "bytes_after": new_content.len(),
            })
            .to_string())
        }
        Err(e) => {
            let msg = match e {
                FuzzyError::NotFound => format!(
                    "apply_diff: 'before' block not found in {raw_path} (tried exact + line-trim \
                     match). Re-read the file and copy the block exactly, or widen the context."
                ),
                FuzzyError::Ambiguous => format!(
                    "apply_diff: 'before' block matched in multiple places in {raw_path} — \
                     ambiguous. Add more surrounding lines so the block is unique."
                ),
                FuzzyError::EmptyBefore => {
                    "apply_diff: 'before' is empty (nothing to find)".to_string()
                }
            };
            eprintln!(
                "  {} {}",
                crate::theme::dim("▸"),
                crate::theme::dim(&format!("apply_diff: {raw_path} failed — {msg}")),
            );
            Err(msg)
        }
    }
}

/// Replace the first occurrence of `before` in `content` with `after`.
/// Pass 1: byte-for-byte exact. Pass 2: line-trim fallback.
///
/// Returns `Err(Ambiguous)` if the block matches in more than one place.
fn fuzzy_replace(content: &str, before: &str, after: &str) -> Result<String, FuzzyError> {
    if before.is_empty() {
        return Err(FuzzyError::EmptyBefore);
    }

    // Pass 1: exact match. Check for multiplicity.
    let mut matches = content.match_indices(before);
    if let Some((idx, _)) = matches.next() {
        if matches.next().is_some() {
            return Err(FuzzyError::Ambiguous);
        }
        // Preserve line-boundary semantics: when the matched `before` span
        // ends with a newline but `after` doesn't, the caller almost
        // certainly meant for the line break to stick — else the next file
        // line glues onto the new content.
        let after_norm = if before.ends_with('\n') && !after.is_empty() && !after.ends_with('\n') {
            let mut s = after.to_string();
            s.push('\n');
            std::borrow::Cow::Owned(s)
        } else {
            std::borrow::Cow::Borrowed(after)
        };
        return Ok(splice(content, idx, before.len(), &after_norm));
    }

    // Pass 2: line-trim fallback.
    let content_lines: Vec<&str> = content.split_inclusive('\n').collect();
    let before_lines: Vec<&str> = before.split_inclusive('\n').collect();
    let m = before_lines.len();
    let n = content_lines.len();
    if m == 0 || n < m {
        return Err(FuzzyError::NotFound);
    }
    let before_trim: Vec<&str> = before_lines.iter().map(|l| l.trim()).collect();
    let mut first_hit: Option<usize> = None;
    let mut hit_count = 0;
    for i in 0..=(n - m) {
        let window_matches = (0..m).all(|j| content_lines[i + j].trim() == before_trim[j]);
        if window_matches {
            if first_hit.is_none() {
                first_hit = Some(i);
            }
            hit_count += 1;
            if hit_count > 1 {
                return Err(FuzzyError::Ambiguous);
            }
        }
    }
    let Some(i) = first_hit else {
        return Err(FuzzyError::NotFound);
    };

    // Reconstruct: pre-window lines + after + post-window lines.
    let mut out = String::with_capacity(content.len() + after.len());
    for line in &content_lines[..i] {
        out.push_str(line);
    }
    out.push_str(after);
    if !after.ends_with('\n')
        && content_lines[i..i + m].last().is_some_and(|l| l.ends_with('\n'))
    {
        out.push('\n');
    }
    for line in &content_lines[i + m..] {
        out.push_str(line);
    }
    Ok(out)
}

fn splice(content: &str, start: usize, len: usize, insert: &str) -> String {
    let mut out = String::with_capacity(content.len() - len + insert.len());
    out.push_str(&content[..start]);
    out.push_str(insert);
    out.push_str(&content[start + len..]);
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
        assert_eq!(names, ["apply_diff"]);
    }

    #[test]
    fn exact_match_replaces() {
        let content = "fn main() {\n    println!(\"hello\");\n}\n";
        let before = "    println!(\"hello\");\n";
        let after = "    println!(\"world\");\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert!(got.contains("world"));
        assert!(!got.contains("hello"));
    }

    #[test]
    fn exact_multiple_matches_is_ambiguous() {
        let content = "alpha\nalpha\nalpha\n";
        let err = fuzzy_replace(content, "alpha\n", "beta\n").unwrap_err();
        assert_eq!(err, FuzzyError::Ambiguous);
    }

    #[test]
    fn whitespace_drift_falls_back_to_line_trim() {
        // Model emitted 4-space indent but file actually uses 2-space.
        let content = "fn foo() {\n  let x = 1;\n  let y = 2;\n}\n";
        let before = "    let x = 1;\n    let y = 2;\n";
        let after = "    let x = 99;\n    let y = 100;\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert!(got.contains("let x = 99"));
        assert!(got.contains("let y = 100"));
        assert!(!got.contains("let x = 1"));
    }

    #[test]
    fn crlf_in_content_lf_in_diff_falls_back() {
        let content = "alpha\r\nbeta\r\ngamma\r\n";
        let before = "alpha\nbeta\n";
        let after = "ALPHA\nBETA\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert!(got.contains("ALPHA"));
        assert!(got.contains("BETA"));
        assert!(got.contains("gamma\r\n"));
    }

    #[test]
    fn empty_before_errors() {
        let err = fuzzy_replace("anything", "", "x").unwrap_err();
        assert_eq!(err, FuzzyError::EmptyBefore);
    }

    #[test]
    fn missing_before_returns_not_found() {
        let err = fuzzy_replace("alpha\nbeta\n", "delta\n", "x").unwrap_err();
        assert_eq!(err, FuzzyError::NotFound);
    }

    #[test]
    fn line_trim_ambiguity_detected() {
        let content = "    x = 1\n  x = 1\n";
        let err = fuzzy_replace(content, "x = 1\n", "x = 2\n").unwrap_err();
        assert_eq!(err, FuzzyError::Ambiguous);
    }

    #[test]
    fn replacement_preserves_surrounding_content() {
        let content = "preamble\nold body\npostamble\n";
        let got = fuzzy_replace(content, "old body\n", "new body\n").unwrap();
        assert_eq!(got, "preamble\nnew body\npostamble\n");
    }

    #[test]
    fn after_without_trailing_newline_gets_one() {
        let content = "alpha\nold\nbeta\n";
        let got = fuzzy_replace(content, "old\n", "new").unwrap();
        assert!(got.contains("\nnew\n"), "got: {got:?}");
    }

    #[test]
    fn last_block_in_file_with_no_trailing_newline() {
        let content = "alpha\nbeta\nfinal";
        let before = "final";
        let after = "FINAL";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert!(got.ends_with("FINAL"));
    }

    #[test]
    fn run_apply_diff_rejects_missing_before() {
        let err = run_apply_diff(r#"{"path":"x","after":"y"}"#).unwrap_err();
        assert!(err.contains("missing 'before'"), "got: {err}");
    }
}
