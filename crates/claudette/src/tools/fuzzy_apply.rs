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
//! Path safety: paths are validated through `super::validate_edit_path` the
//! same way `apply_patch`/`edit_file` do — $HOME-gated for the interactive
//! secretary, but confined to the mission tree while a forge/brownfield
//! mission is active (roast RC-B), so the autonomous Coder can't patch files
//! outside it.

use std::fs;

use serde_json::{json, Value};

use super::{parse_json_input, validate_edit_path};

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

    let path = validate_edit_path(raw_path).map_err(|e| format!("apply_diff: {raw_path}: {e}"))?;
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
                FuzzyError::NotFound => {
                    // Near-miss diagnostics (dogfood T2): a bare "not found"
                    // sends small brains down CRLF/whitespace rabbit holes
                    // when the real cause is usually over-escaped backslashes
                    // or one drifted line. Name the difference when we can.
                    let hint =
                        super::near_miss::near_miss_hint(&original, before).unwrap_or_else(|| {
                            "Re-read the file and copy the block exactly, or widen the context."
                                .to_string()
                        });
                    format!(
                        "apply_diff: 'before' block not found in {raw_path} (tried exact + \
                         line-trim match). {hint}"
                    )
                }
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

/// True if the byte span `[idx, idx+len)` in `content` covers whole lines:
/// it starts at the beginning of a line (start-of-file or right after a `\n`)
/// and ends at the end of a line (end-of-file or right after a `\n`).
///
/// Pass 1 requires this (roast RC-E C1/C2): without it, `match_indices`
/// happily matches a `before` that occurs only *inside* a comment or string,
/// or mid-token, and silently edits the wrong place. Line-anchoring confines
/// the exact pass to genuine block replacements; sub-line text that the model
/// wants to change must be supplied as its whole line (the line-trim pass then
/// handles indentation drift).
fn line_anchored(content: &str, idx: usize, len: usize) -> bool {
    let b = content.as_bytes();
    let start_ok = idx == 0 || b.get(idx.wrapping_sub(1)) == Some(&b'\n');
    let end = idx + len;
    let end_ok = end == content.len() || b.get(end - 1) == Some(&b'\n');
    start_ok && end_ok
}

/// Re-encode every line ending in `text` to CRLF (`crlf=true`) or LF
/// (`crlf=false`). Collapses CRLF→LF first so the result is uniform. Keeps
/// the replacement region's line endings consistent with the file it's being
/// spliced into (roast RC-E H1/M3 — previously an LF `after` spliced into a
/// CRLF file produced a mixed-EOL hunk).
fn normalize_eol(text: &str, crlf: bool) -> String {
    let lf = text.replace("\r\n", "\n");
    if crlf {
        lf.replace('\n', "\r\n")
    } else {
        lf
    }
}

/// The leading run of spaces/tabs at the start of `line`, excluding the line
/// terminator. (`'\r'`/`'\n'` aren't whitespace for this purpose — they stop
/// the scan, so a blank line `"  \n"` yields `"  "`.)
fn leading_ws(line: &str) -> &str {
    let end = line
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(line.len());
    &line[..end]
}

/// The longest common leading substring of `a` and `b`. Both args are runs of
/// indentation whitespace, so this is always a char boundary.
fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let n = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    &a[..n]
}

/// Rebase `block`'s outermost indentation onto `target_indent`, preserving the
/// block's internal relative nesting. The block's own base indent (the common
/// leading-whitespace prefix of its non-blank lines) is stripped from each line
/// and replaced with `target_indent`; blank lines keep only their terminator.
///
/// Pass 2 matches on TRIMMED lines, so the model's `after` carries whatever
/// indentation the model guessed. Splicing it verbatim silently corrupts
/// whitespace-significant languages (Python/YAML/Makefile: an IndentationError
/// or a changed scope) and writes inconsistent indentation everywhere else,
/// reported `ok:true` (roast 2026-05-31 / issue #26 §A). Rebasing to the matched
/// window's actual indent fixes that while keeping the edit's structure.
fn reindent_to(block: &str, target_indent: &str) -> String {
    let lines: Vec<&str> = block.split_inclusive('\n').collect();
    // The block's base indent = the common leading-ws prefix of its non-blank
    // lines (blank lines carry no meaningful indentation).
    let mut base: Option<&str> = None;
    for line in &lines {
        if line.trim().is_empty() {
            continue;
        }
        let ws = leading_ws(line);
        base = Some(match base {
            None => ws,
            Some(prev) => common_prefix(prev, ws),
        });
    }
    let base = base.unwrap_or("");
    let mut out = String::with_capacity(block.len() + target_indent.len() * lines.len());
    for line in &lines {
        if line.trim().is_empty() {
            // Preserve a blank line as just its break (no trailing indent);
            // normalize_eol re-encodes the terminator afterwards.
            if line.ends_with('\n') {
                out.push('\n');
            }
            continue;
        }
        out.push_str(target_indent);
        out.push_str(line.strip_prefix(base).unwrap_or(line));
    }
    out
}

/// Replace the first occurrence of `before` in `content` with `after`.
/// Pass 1: line-anchored exact match. Pass 2: line-trim fallback. Both count
/// *all* candidate placements (overlapping included) and return `Ambiguous`
/// when more than one matches, so a genuinely ambiguous edit is rejected
/// rather than silently applied to the first hit (roast RC-E C3).
fn fuzzy_replace(content: &str, before: &str, after: &str) -> Result<String, FuzzyError> {
    if before.is_empty() {
        return Err(FuzzyError::EmptyBefore);
    }

    // Pass 1: line-anchored exact match. Scan ALL occurrences (advancing by 1
    // byte so self-overlapping repeats are counted, not collapsed by
    // `match_indices`'s non-overlapping stride), keeping only line-anchored
    // ones.
    let mut hits: Vec<usize> = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = content[from..].find(before) {
        let idx = from + rel;
        if line_anchored(content, idx, before.len()) {
            hits.push(idx);
        }
        from = idx + 1;
    }
    match hits.len() {
        0 => {} // fall through to the line-trim pass
        1 => {
            let idx = hits[0];
            // The matched span is byte-identical to `before`, so derive the
            // region's EOL style from `before` (fall back to the file's
            // dominant EOL for a single-line `before`) and re-encode `after`
            // to match.
            let crlf = if before.contains("\r\n") {
                true
            } else if before.contains('\n') {
                false
            } else {
                content.contains("\r\n")
            };
            let mut after_norm = normalize_eol(after, crlf);
            // Preserve line-boundary semantics: a `before` that ends with a
            // newline but an `after` that doesn't would glue the next file
            // line onto the new content — re-add the (correctly-encoded) break.
            if before.ends_with('\n') && !after_norm.is_empty() && !after_norm.ends_with('\n') {
                after_norm.push_str(if crlf { "\r\n" } else { "\n" });
            }
            return Ok(splice(content, idx, before.len(), &after_norm));
        }
        _ => return Err(FuzzyError::Ambiguous),
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

    // Reconstruct: pre-window lines + after + post-window lines. First rebase
    // `after`'s indentation onto the matched window (issue #26 §A) — Pass 2
    // matched on trimmed lines, so `after` carries the model's guessed indent;
    // splicing it verbatim corrupts whitespace-significant languages. Then
    // re-encode to the window's EOL style so a CRLF file keeps CRLF inside the
    // replaced region (roast RC-E H1).
    let file_indent = leading_ws(content_lines[i]);
    let reindented = reindent_to(after, file_indent);
    let window_crlf = content_lines[i..i + m].iter().any(|l| l.ends_with("\r\n"));
    let mut after_norm = normalize_eol(&reindented, window_crlf);
    if !after_norm.is_empty()
        && !after_norm.ends_with('\n')
        && content_lines[i..i + m]
            .last()
            .is_some_and(|l| l.ends_with('\n'))
    {
        after_norm.push_str(if window_crlf { "\r\n" } else { "\n" });
    }
    let mut out = String::with_capacity(content.len() + after_norm.len());
    for line in &content_lines[..i] {
        out.push_str(line);
    }
    out.push_str(&after_norm);
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
        // issue #26 §A: the result must adopt the FILE's 2-space indent, not the
        // model's 4-space `after` — splicing verbatim would corrupt indentation.
        assert_eq!(
            got, "fn foo() {\n  let x = 99;\n  let y = 100;\n}\n",
            "after must be re-indented to the matched window: {got:?}"
        );
    }

    #[test]
    fn line_trim_reindents_python_block_to_file_indent() {
        // issue #26 §A: whitespace-significant language. The file body is indented
        // 4 spaces; the model's `after` guessed 2 spaces. Verbatim splice would
        // produce an IndentationError; the re-indent must rebase to 4 spaces.
        let content = "def f():\n    x = 1\n    y = 2\n";
        let before = "  x = 1\n  y = 2\n"; // model used 2-space (matches on trim)
        let after = "  x = 10\n  y = 20\n  z = 30\n"; // model's 2-space `after`
        let got = fuzzy_replace(content, before, after).unwrap();
        assert_eq!(
            got, "def f():\n    x = 10\n    y = 20\n    z = 30\n",
            "block must be rebased to the file's 4-space indent: {got:?}"
        );
    }

    #[test]
    fn line_trim_preserves_internal_relative_nesting_when_reindenting() {
        // A nested block: the rebase keeps the block's INTERNAL step (the `if`
        // body sits one level deeper than the `if`) while moving the whole block
        // to the file's outer indent.
        let content = "def f():\n    if a:\n        b()\n";
        let before = "  if a:\n      b()\n"; // model 2-space outer, 6-space inner
        let after = "  if a:\n      c()\n"; // same shape, body changed
        let got = fuzzy_replace(content, before, after).unwrap();
        // Outer `if` rebased to 4 spaces; inner kept its +4 relative step → 8.
        assert_eq!(
            got, "def f():\n    if a:\n        c()\n",
            "internal nesting must be preserved across the rebase: {got:?}"
        );
    }

    #[test]
    fn crlf_in_content_lf_in_diff_falls_back() {
        let content = "alpha\r\nbeta\r\ngamma\r\n";
        let before = "alpha\nbeta\n";
        let after = "ALPHA\nBETA\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        // roast RC-E H1: the replaced region must keep the file's CRLF, not
        // become LF (which produced a mixed-EOL hunk git flags as churn).
        assert_eq!(got, "ALPHA\r\nBETA\r\ngamma\r\n", "got: {got:?}");
    }

    #[test]
    fn exact_pass_does_not_edit_a_substring_inside_a_comment() {
        // roast RC-E C1: `before` occurs only inside a comment. It must NOT be
        // silently edited; the real (different) code line is left for the
        // model to target with its full line.
        let content = "// TODO: set rate = 0.05 properly\nrate = 0.10\n";
        let before = "rate = 0.05";
        let after = "rate = 0.20";
        let err = fuzzy_replace(content, before, after).unwrap_err();
        assert_eq!(err, FuzzyError::NotFound, "must not edit the comment");
    }

    #[test]
    fn exact_pass_does_not_edit_mid_token() {
        // roast RC-E C2: a partial-line `before` ("ax=10" inside "max=10")
        // must not splice mid-token.
        let content = "max=10\n";
        let err = fuzzy_replace(content, "ax=10", "ax=99").unwrap_err();
        assert_eq!(err, FuzzyError::NotFound);
    }

    #[test]
    fn overlapping_repeats_are_ambiguous() {
        // roast RC-E C3: "ab\nab\n" matches lines (0,1) AND (1,2). The old
        // non-overlapping match_indices saw one match and silently picked the
        // first; now both are counted and the edit is rejected as ambiguous.
        let content = "ab\nab\nab\n";
        let err = fuzzy_replace(content, "ab\nab\n", "X\n").unwrap_err();
        assert_eq!(err, FuzzyError::Ambiguous);
    }

    #[test]
    fn whole_line_before_without_trailing_newline_still_matches_via_trim() {
        // A whole line supplied without its trailing newline isn't line-anchored
        // in the exact pass, but the line-trim pass still finds it.
        let content = "alpha\nfoo\nbeta\n";
        let got = fuzzy_replace(content, "foo", "bar").unwrap();
        assert_eq!(got, "alpha\nbar\nbeta\n");
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

    #[test]
    fn not_found_error_carries_near_miss_hint() {
        // Dogfood T2: a `before` with doubled backslashes must produce the
        // over-escaping diagnosis end-to-end, not the generic "copy exactly".
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = format!("{home}/claudette-diff-nearmiss-{nanos}.txt");
        let original = "fn pat() {\n    let re = r\"^\\s*fn\";\n}\n";
        fs::write(&path, original).unwrap();

        let input = json!({
            "path": &path,
            "before": "    let re = r\"^\\\\s*fn\";\n",
            "after": "    let re = r\"^\\\\s*struct\";\n"
        })
        .to_string();
        let result = run_apply_diff(&input);
        let _ = fs::remove_file(&path);

        let err = result.expect_err("expected not-found error");
        assert!(err.contains("'before' block not found"), "got: {err}");
        assert!(err.contains("over-escapes backslashes"), "got: {err}");
    }
}
