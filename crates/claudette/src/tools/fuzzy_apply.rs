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
    /// `after` mixes tabs and spaces so a line's indentation can be related to
    /// neither the block's anchor nor the file's. Refusing beats guessing: the
    /// old fallback invented an indent the caller never sent (roast EDIT-02).
    IndentMismatch,
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
            // No-op guard (dogfood 2026-06-13): a before/after that produce
            // byte-identical content changed nothing. Reporting ok:true here is
            // a false success that spirals small brains — they "fixed" the file,
            // see success, but nothing moved, so they re-send the same edit. The
            // display layer collapses `\\`->`\`, so an over-escaped block looks
            // identical to the model and it cannot see why. Fail loudly instead.
            if new_content == original {
                let msg = format!(
                    "apply_diff: no change — 'before' and 'after' produce identical \
                     content in {raw_path}, so nothing was written. Re-read the file \
                     to see its CURRENT bytes: what you intend to change may already \
                     be present, or your two blocks are the same. Do NOT re-send this \
                     edit unchanged."
                );
                eprintln!(
                    "  {} {}",
                    crate::theme::dim("▸"),
                    crate::theme::dim(&format!("apply_diff: {raw_path} no-op")),
                );
                return Err(msg);
            }
            // Pre-image before the write so `/undo` can restore this file
            // (roast EDIT-03: apply_diff had no snapshot, which made `/undo`
            // a no-op for the primary edit path). Fail-closed: no snapshot,
            // no write. The file is known to exist — it was read above.
            crate::transcript::snapshot_to_trash(&path).map_err(|e| {
                format!(
                    "apply_diff: pre-image snapshot failed, refusing to edit {}: {e}",
                    path.display()
                )
            })?;
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
                    "apply_diff: 'before' is blank (nothing to find) — it must \
                     contain at least one non-whitespace character"
                        .to_string()
                }
                FuzzyError::IndentMismatch => format!(
                    "apply_diff: the 'after' block mixes tabs and spaces in a way that \
                     cannot be lined up with its own first line, so it cannot be \
                     re-indented into {raw_path} without inventing whitespace. Re-send \
                     'after' using the file's actual indentation."
                ),
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

/// The leading whitespace of a block's first non-blank line.
///
/// Shared so the caller can ask "is `after` already at the file's indent?" using
/// exactly the anchor `reindent_to` would rebase against — two different answers
/// to that question is how a block gets rebased onto itself (roast EDIT-02).
fn anchor_indent(block: &str) -> &str {
    block
        .split_inclusive('\n')
        .find(|l| !l.trim().is_empty())
        .map_or("", |l| leading_ws(l))
}

/// Rebase `block` so its FIRST non-blank line sits at `target_indent`, shifting
/// every other line by the SAME amount so the block keeps its internal relative
/// nesting — including lines that dedent BELOW the first line (a closing brace,
/// a `},`).
///
/// Pass 2 matches on TRIMMED lines, so the model's `after` carries whatever
/// indentation the model guessed. Splicing it verbatim silently corrupts
/// whitespace-significant languages (Python/YAML/Makefile: an IndentationError
/// or a changed scope) and writes inconsistent indentation everywhere else,
/// reported `ok:true` (roast 2026-05-31 / issue #26 §A). Rebasing to the matched
/// window's actual indent fixes that while keeping the edit's structure.
///
/// The anchor is the block's FIRST non-blank line, because it aligns with the
/// matched window's first line — which is where the caller measured
/// `target_indent`. (An earlier version anchored on the block's *minimum* indent;
/// when the first line was deeper than that minimum — e.g. an edit ending in a
/// less-indented `}` — every line was shifted right by the difference, silently
/// mis-indenting the whole block. Dogfood Tasks 9/10.)
///
/// Refuses with `IndentMismatch` rather than guessing when a line's indentation
/// neither extends nor prefixes the anchor. The old fallback pushed
/// `target_indent` onto such a line, INVENTING whitespace the caller never sent
/// (roast EDIT-02).
fn reindent_to(block: &str, target_indent: &str) -> Result<String, FuzzyError> {
    let lines: Vec<&str> = block.split_inclusive('\n').collect();
    // Anchor = the leading whitespace of the first non-blank line. Every line is
    // re-based by the same (target − anchor) delta so relative nesting survives.
    let anchor = anchor_indent(block);
    let mut out = String::with_capacity(block.len() + target_indent.len() * lines.len());
    for line in &lines {
        if line.trim().is_empty() {
            // Preserve a blank line VERBATIM. Collapsing it to a bare break
            // drops whitespace that may be CONTENT rather than layout — a blank
            // line inside a heredoc or a multi-line string literal is part of
            // the string's value (roast EDIT-02).
            out.push_str(line);
            continue;
        }
        let ws = leading_ws(line);
        let body = &line[ws.len()..];
        if let Some(extra) = ws.strip_prefix(anchor) {
            // At the anchor depth or deeper: target indent + the extra nesting.
            out.push_str(target_indent);
            out.push_str(extra);
        } else if let Some(removed) = anchor.strip_prefix(ws) {
            // Shallower than the anchor (a dedent line such as a closing brace):
            // trim the same trailing run off the target indent so it dedents in
            // step. Indentation is ASCII spaces/tabs, so byte-slicing is safe.
            let keep = target_indent.len().saturating_sub(removed.len());
            out.push_str(&target_indent[..keep]);
        } else {
            // Indentation styles diverge (mixed tabs/spaces). Refuse: inventing
            // an indent for this line is what corrupted files silently.
            return Err(FuzzyError::IndentMismatch);
        }
        out.push_str(body);
    }
    Ok(out)
}

/// Replace the first occurrence of `before` in `content` with `after`.
/// Pass 1: line-anchored exact match. Pass 2: line-trim fallback. Both count
/// *all* candidate placements (overlapping included) and return `Ambiguous`
/// when more than one matches, so a genuinely ambiguous edit is rejected
/// rather than silently applied to the first hit (roast RC-E C3).
fn fuzzy_replace(content: &str, before: &str, after: &str) -> Result<String, FuzzyError> {
    // Blank, not just empty: Pass 2 compares TRIMMED lines, so a `before` of
    // "   " or "\n" trims to "" and matches ANY blank line in the file. With
    // exactly one blank line the ambiguity check never fires and the block is
    // injected at an arbitrary position, reported `ok:true` (roast EDIT-05).
    // This one guard is sufficient: if `before.trim()` is non-empty then at
    // least one of its lines has a non-empty trim, so no Pass-2 window can be
    // made entirely of blank lines.
    if before.trim().is_empty() {
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
    // Derive the indent from the first NON-BLANK line of the matched window.
    // `leading_ws("\n")` is "", so a window that starts on a blank line used to
    // re-base the whole replacement to column 0 — silently lifting it out of its
    // scope (a Python method stopped being a method; YAML keys hoisted to
    // document root), reported `ok:true` (roast EDIT-01).
    let file_indent = content_lines[i..i + m]
        .iter()
        .find(|l| !l.trim().is_empty())
        .map_or("", |l| leading_ws(l));
    // Align or refuse (roast EDIT-02). When `after` already sits at the window's
    // indent there is nothing to rebase, so splice it VERBATIM. Re-indenting an
    // already-correct block is what rewrote the interior of string literals and
    // heredocs — changing a string's runtime value while reporting `ok:true`.
    let reindented = if anchor_indent(after) == file_indent {
        after.to_string()
    } else {
        reindent_to(after, file_indent)?
    };
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
    fn line_trim_keeps_indent_when_block_dedents_below_first_line() {
        // Regression (dogfood Tasks 9/10): the matched window's FIRST line is
        // deeper than a later line in the block (a closing brace / `},`). The
        // old rebase anchored on the block's MINIMUM indent, so it shifted the
        // whole block right by (first-line − min) and over-indented it. Anchoring
        // on the first line keeps the block exactly where it belongs.
        let content = "obj = {\n        \"a\": 1,\n        \"b\": 2,\n    }\nnext\n";
        // Trailing space on line 1 → Pass 1 (exact) misses, Pass 2 (line-trim)
        // handles it; the block's first line (8sp) is deeper than its `}` (4sp).
        let before = "        \"a\": 1, \n        \"b\": 2,\n    }\n";
        let after = "        \"a\": 1,\n        \"b\": 9,\n    }\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert_eq!(
            got, "obj = {\n        \"a\": 1,\n        \"b\": 9,\n    }\nnext\n",
            "block must keep its own indentation, not shift right: {got:?}"
        );
    }

    #[test]
    fn line_trim_python_if_else_keeps_dedented_else() {
        // Whitespace-significant case: the block starts inside an `if` body (8sp)
        // and includes the `else:` which dedents to 4sp. A whole-block right-shift
        // (the old bug) would push `else:` to 8sp — turning it into part of the
        // if-body, a silent scope change reported `ok:true`. Anchoring on the
        // first line keeps `else:` at its level.
        let content = "def f(x):\n    if x:\n        a = 1\n    else:\n        b = 2\n";
        let before = "        a = 1 \n    else:\n        b = 2\n"; // trailing space → Pass 2
        let after = "        a = 10\n    else:\n        b = 20\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert_eq!(
            got, "def f(x):\n    if x:\n        a = 10\n    else:\n        b = 20\n",
            "else: must stay at 4sp, not shift into the if-body: {got:?}"
        );
    }

    #[test]
    fn aligned_after_splices_verbatim_and_keeps_a_literal_blank_line() {
        // EDIT-02: `after` is already at the window's indent, so there is
        // nothing to rebase. The blank line inside the string literal carries
        // three spaces — that is the string's CONTENT. Rebasing collapsed it to
        // a bare newline and changed the value while reporting ok:true.
        let content = "def q():\n    sql = \"\"\"\n      SELECT 1\n   \n    \"\"\"\n";
        let before = "sql = \"\"\"\n      SELECT 1\n   \n    \"\"\"\n";
        let after = "    sql = \"\"\"\n      SELECT 2\n   \n    \"\"\"\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert_eq!(
            got, "def q():\n    sql = \"\"\"\n      SELECT 2\n   \n    \"\"\"\n",
            "an already-aligned after must be spliced byte-for-byte: {got:?}"
        );
    }

    #[test]
    fn shifted_block_keeps_a_blank_line_s_whitespace() {
        // The same protection on the SHIFT path: `after` is misaligned (2sp vs
        // the file's 4sp) so it IS rebased, but the blank line's own whitespace
        // is content and must survive the shift untouched.
        let content = "def f():\n    a = 1\n   \n    b = 2\n";
        let before = "a = 1\n   \nb = 2\n";
        let after = "  a = 10\n   \n  b = 20\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert_eq!(
            got, "def f():\n    a = 10\n   \n    b = 20\n",
            "the shift must not rewrite the blank line: {got:?}"
        );
    }

    #[test]
    fn aligned_block_with_mixed_indent_is_spliced_not_refused() {
        // The other half of "align or refuse": a Makefile-shaped file really
        // does mix a tab body with space-aligned continuation lines. Because
        // `after` already sits at the window's indent there is nothing to
        // rebase, so it must be spliced — NOT rejected by IndentMismatch.
        // Without the align-first branch, adding that error would regress every
        // legitimately mixed-indentation file.
        let content = "build:\n\techo one\n    echo two\ndone\n";
        let before = "echo one\necho two\n";
        let after = "\techo ONE\n    echo TWO\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert_eq!(
            got, "build:\n\techo ONE\n    echo TWO\ndone\n",
            "an aligned block must splice even when its own indent is mixed: {got:?}"
        );
    }

    #[test]
    fn mixed_tab_and_space_after_is_refused_not_guessed() {
        // EDIT-02: line 2's indent neither extends nor prefixes the tab anchor.
        // The old code pushed target_indent onto it, inventing an indentation
        // the model never sent. Refusing is the only honest answer.
        let content = "fn f() {\n        one\n        two\n}\n";
        let before = "one\ntwo\n";
        let after = "\tone\n    two\n";
        let err = fuzzy_replace(content, before, after).unwrap_err();
        assert_eq!(
            err,
            FuzzyError::IndentMismatch,
            "mixed tabs/spaces must refuse rather than invent an indent"
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

    // EDIT-01: a matched window whose FIRST line is blank must take its indent
    // from the first non-blank line, not from the blank one.
    //
    // Every `before` below carries a DELIBERATELY WRONG indent so the Pass 1
    // exact-match path cannot fire. That matters: Pass 1 returns early, and the
    // bug being tested lives in Pass 2. A `before` copied verbatim out of
    // `content` would make these tests vacuous.

    #[test]
    fn blank_first_line_window_keeps_python_method_nested() {
        let content = "class Foo:\n\n    def bar(self):\n        pass\n";
        let before = "\n  def bar(self):\n";
        let after = "\n  def bar(self):\n      return 42\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert!(
            got.contains("\n    def bar(self):\n"),
            "method must stay nested under the class: {got:?}"
        );
        assert!(
            !got.contains("\ndef bar(self):"),
            "method must not dedent to column 0: {got:?}"
        );
    }

    #[test]
    fn blank_first_line_window_keeps_yaml_keys_nested() {
        let content = "server:\n\n  host: localhost\n  port: 8080\n";
        let before = "\nhost: localhost\nport: 8080\n";
        let after = "\nhost: 127.0.0.1\nport: 9090\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert!(
            got.contains("\n  host: 127.0.0.1\n"),
            "keys must stay under `server:`: {got:?}"
        );
        assert!(
            !got.contains("\nhost: 127.0.0.1"),
            "keys must not hoist to document root: {got:?}"
        );
    }

    #[test]
    fn nonblank_first_line_window_is_unchanged_by_the_fix() {
        // Regression guard only: this window's first line is non-blank, so the
        // new code must pick exactly the indent the old code did.
        let content = "def f():\n    x = 1\n    y = 2\n";
        let before = "  x = 1\n  y = 2\n";
        let after = "  x = 10\n  y = 20\n";
        let got = fuzzy_replace(content, before, after).unwrap();
        assert_eq!(got, "def f():\n    x = 10\n    y = 20\n");
    }

    // EDIT-05: a `before` made only of whitespace is a wildcard against any
    // blank line. It must be refused, not applied.

    #[test]
    fn whitespace_only_before_is_refused() {
        let err = fuzzy_replace("alpha\n\nbeta\n", "   ", "INJECTED\n").unwrap_err();
        assert_eq!(err, FuzzyError::EmptyBefore);
    }

    #[test]
    fn newline_only_before_is_refused() {
        let err = fuzzy_replace("alpha\n\nbeta\n", "\n", "INJECTED\n").unwrap_err();
        assert_eq!(err, FuzzyError::EmptyBefore);
    }

    #[test]
    fn before_that_merely_starts_blank_is_still_accepted() {
        // The EDIT-05 guard must not over-reject: a leading blank line is fine
        // as long as the block carries real content somewhere.
        let content = "alpha\n\n  beta\n";
        let got = fuzzy_replace(content, "\nbeta\n", "\nGAMMA\n").unwrap();
        assert!(
            got.contains("GAMMA"),
            "real block must still apply: {got:?}"
        );
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
        // home-resolving: serialize against the temp-HOME swaps in
        // runtime/prompt.rs so $HOME doesn't change mid-test.
        let _eg = crate::test_env_lock();
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

    #[test]
    fn identical_before_after_is_a_loud_no_op() {
        // The dogfood 2026-06-13 spiral: before == after (the model could not
        // see its own doubled backslash). The edit must FAIL with a no-op error,
        // not report ok:true after writing nothing.
        // home-resolving: serialize against the temp-HOME swaps in
        // runtime/prompt.rs so $HOME doesn't change mid-test.
        let _eg = crate::test_env_lock();
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = format!("{home}/claudette-diff-noop-{nanos}.txt");
        let original = "fn main() {\n    let x = 1;\n}\n";
        fs::write(&path, original).unwrap();

        let input = json!({
            "path": &path,
            "before": "    let x = 1;\n",
            "after": "    let x = 1;\n"
        })
        .to_string();
        let result = run_apply_diff(&input);
        // The file must be untouched.
        let after_disk = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);

        let err = result.expect_err("identical before/after must be a no-op error");
        assert!(err.contains("no change"), "got: {err}");
        assert_eq!(after_disk, original, "file must not be modified by a no-op");
    }

    #[test]
    fn apply_diff_snapshots_a_pre_image() {
        crate::with_temp_home(|home| {
            let prev_ws = std::env::var("CLAUDETTE_WORKSPACE").ok();
            std::env::remove_var("CLAUDETTE_WORKSPACE");

            let file = home.join("diffme.txt");
            fs::write(&file, "alpha\nbeta\n").unwrap();
            let input = json!({
                "path": file.display().to_string(),
                "before": "beta",
                "after": "gamma",
            })
            .to_string();
            run_apply_diff(&input).unwrap();

            assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
            let trash = home.join(".claudette").join("trash");
            let entries: Vec<_> = fs::read_dir(&trash)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            assert_eq!(entries.len(), 1, "expected exactly one pre-image");
            assert_eq!(
                fs::read_to_string(&entries[0]).unwrap(),
                "alpha\nbeta\n",
                "pre-image must hold the ORIGINAL content"
            );

            match prev_ws {
                Some(v) => std::env::set_var("CLAUDETTE_WORKSPACE", v),
                None => std::env::remove_var("CLAUDETTE_WORKSPACE"),
            }
        });
    }
}
