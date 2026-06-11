//! Near-miss diagnostics for the block-edit tools (`apply_diff`,
//! `edit_file`).
//!
//! When a `before`/`old_text` block isn't found, the bare "not found —
//! copy the block exactly" error sends small local brains down
//! wrong-hypothesis rabbit holes. Dogfood T2 (2026-06-11) burned ~15
//! iterations theorizing about CRLF and trailing whitespace when the real
//! problem was doubled backslashes (`r"^\\s*fn"` in the tool call where the
//! file has `r"^\s*fn"`) — the classic JSON-escaping confusion every local
//! model hits when raw-string regexes pass through tool-call JSON.
//!
//! [`near_miss_hint`] turns that one-shot diagnosable failure into an
//! actionable message:
//! 1. **Over-escaped backslashes** — if de-doubling `\\` → `\` in the
//!    model's block produces a match, say exactly that.
//! 2. **Closest line-window** — otherwise find the content window whose
//!    trimmed lines best match the block's and report the first differing
//!    line, file-side vs block-side.
//!
//! Pure functions over strings; both tools call this only on their failure
//! path, so the O(lines × block-lines) scan never taxes a successful edit.

/// Don't scan pathologically large files on the failure path.
const MAX_CONTENT_BYTES: usize = 256 * 1024;
/// Blocks longer than this are unlikely to be near-misses worth a window
/// scan (and the model should be sending smaller edits anyway).
const MAX_BLOCK_LINES: usize = 64;
/// Cap quoted fragments in the hint so the tool_result stays readable.
const SNIPPET_CHARS: usize = 120;

/// Diagnose why `block` failed to match inside `content`. Returns a
/// ready-to-append sentence, or `None` when there is no credible near-miss
/// (the generic "re-read and copy exactly" advice is then the best we have).
pub(super) fn near_miss_hint(content: &str, block: &str) -> Option<String> {
    if block.is_empty() || content.len() > MAX_CONTENT_BYTES {
        return None;
    }

    // 1. Over-escaped backslashes — check first because it is the #1
    //    local-model failure mode and the fix is a one-line instruction.
    if block.contains("\\\\") {
        let dedoubled = block.replace("\\\\", "\\");
        if block_found(content, &dedoubled) {
            let sample = block
                .lines()
                .find(|l| l.contains("\\\\"))
                .map(|l| truncate(l.trim()))
                .unwrap_or_default();
            return Some(format!(
                "Your block over-escapes backslashes: de-doubling `\\\\` to \
                 `\\` makes it match the file. The file has SINGLE \
                 backslashes where your block doubles them (e.g. your \
                 `{sample}`). Re-send the same edit with single backslashes."
            ));
        }
    }

    // 2. Closest line-window by trimmed-line equality.
    let content_lines: Vec<&str> = content.lines().collect();
    let block_trim: Vec<&str> = block.lines().map(str::trim).collect();
    let m = block_trim.len();
    if m == 0 || m > MAX_BLOCK_LINES || content_lines.len() < m {
        return None;
    }
    let mut best_score = 0usize;
    let mut best_start = 0usize;
    for i in 0..=(content_lines.len() - m) {
        let score = (0..m)
            .filter(|&j| content_lines[i + j].trim() == block_trim[j])
            .count();
        if score > best_score {
            best_score = score;
            best_start = i;
        }
    }
    // Require at least half the lines to line up — a lower score is not a
    // near-miss, and pointing at it would mislead more than the generic
    // advice. (Single-line blocks therefore need an exact trimmed match,
    // which only triggers the whitespace arm below.)
    if best_score == 0 || best_score * 2 < m {
        return None;
    }

    let first_line = best_start + 1; // 1-based for the message
    let last_line = best_start + m;
    let diff = (0..m).find(|&j| content_lines[best_start + j].trim() != block_trim[j]);
    match diff {
        Some(j) => Some(format!(
            "Closest match: lines {first_line}-{last_line} ({best_score}/{m} \
             lines already match). First difference at line {}: file has \
             `{}` but your block has `{}`.",
            best_start + j + 1,
            truncate(content_lines[best_start + j].trim()),
            truncate(block_trim[j]),
        )),
        // Every line matches after trimming — the mismatch is pure
        // whitespace/indentation (edit_file's exact matcher hits this).
        None => Some(format!(
            "Lines {first_line}-{last_line} match your block except for \
             whitespace/indentation — re-read those lines and copy them \
             exactly."
        )),
    }
}

/// True when `block` appears in `content`, either as an exact substring or
/// as a contiguous window of trimmed-equal lines (the same tolerance the
/// edit tools' fuzzy passes use).
fn block_found(content: &str, block: &str) -> bool {
    if content.contains(block) {
        return true;
    }
    let content_lines: Vec<&str> = content.lines().collect();
    let block_trim: Vec<&str> = block.lines().map(str::trim).collect();
    let m = block_trim.len();
    if m == 0 || content_lines.len() < m {
        return false;
    }
    (0..=(content_lines.len() - m))
        .any(|i| (0..m).all(|j| content_lines[i + j].trim() == block_trim[j]))
}

/// Trim a quoted fragment to [`SNIPPET_CHARS`] characters (char-boundary
/// safe), appending an ellipsis when cut.
fn truncate(s: &str) -> String {
    if s.chars().count() <= SNIPPET_CHARS {
        return s.to_string();
    }
    let cut: String = s.chars().take(SNIPPET_CHARS).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_escaped_backslashes_detected() {
        // The dogfood T2 failure verbatim: file has single backslashes in a
        // raw-string regex, the model's block doubles them.
        let content = "fn pat() {\n    let re = r\"^\\s*fn\\s+\\w+\";\n    re\n}\n";
        let block = "    let re = r\"^\\\\s*fn\\\\s+\\\\w+\";\n    re\n";
        let hint = near_miss_hint(content, block).expect("must diagnose over-escaping");
        assert!(hint.contains("over-escapes backslashes"), "got: {hint}");
        assert!(hint.contains("\\\\s"), "sample line should appear: {hint}");
    }

    #[test]
    fn dedouble_that_still_does_not_match_is_not_reported() {
        // Block has `\\` but even the de-doubled version isn't in the file —
        // must not claim over-escaping.
        let content = "alpha\nbeta\ngamma\n";
        let block = "let re = r\"^\\\\d+\";\n";
        let hint = near_miss_hint(content, block);
        assert!(
            hint.is_none() || !hint.as_ref().unwrap().contains("over-escapes"),
            "got: {hint:?}"
        );
    }

    #[test]
    fn closest_window_reports_first_difference() {
        let content = "fn main() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n}\n";
        // 3 of 4 lines match; the `b` line differs.
        let block = "fn main() {\n    let a = 1;\n    let b = 99;\n    let c = 3;\n";
        let hint = near_miss_hint(content, block).expect("must find the near window");
        assert!(hint.contains("Closest match: lines 1-4"), "got: {hint}");
        assert!(hint.contains("3/4"), "got: {hint}");
        assert!(hint.contains("First difference at line 3"), "got: {hint}");
        assert!(hint.contains("let b = 2;"), "file side missing: {hint}");
        assert!(hint.contains("let b = 99;"), "block side missing: {hint}");
    }

    #[test]
    fn whitespace_only_mismatch_reported_as_such() {
        // Trimmed lines all equal — the failure is indentation only (this is
        // edit_file's exact-match blind spot).
        let content = "if x {\n    do_it();\n}\n";
        let block = "if x {\n        do_it();\n}";
        let hint = near_miss_hint(content, block).expect("must diagnose whitespace");
        assert!(
            hint.contains("except for whitespace/indentation"),
            "got: {hint}"
        );
    }

    #[test]
    fn unrelated_block_yields_no_hint() {
        let content = "alpha\nbeta\ngamma\n";
        let block = "fn totally() {\n    different();\n}\n";
        assert_eq!(near_miss_hint(content, block), None);
    }

    #[test]
    fn below_half_match_yields_no_hint() {
        // Only 1 of 4 lines matches — not a credible near-miss.
        let content = "one\ntwo\nthree\nfour\nfive\n";
        let block = "one\nX\nY\nZ\n";
        assert_eq!(near_miss_hint(content, block), None);
    }

    #[test]
    fn oversized_content_is_skipped() {
        let content = "x\n".repeat(200_000); // 400 KB > cap
        let block = "x\ny\n";
        assert_eq!(near_miss_hint(&content, block), None);
    }

    #[test]
    fn empty_block_yields_no_hint() {
        assert_eq!(near_miss_hint("anything\n", ""), None);
    }

    #[test]
    fn long_lines_are_truncated_in_the_hint() {
        let long_a = format!("let value = \"{}A\";", "a".repeat(200));
        let long_b = format!("let value = \"{}B\";", "a".repeat(200));
        let content = format!("start\n{long_a}\nend\n");
        let block = format!("start\n{long_b}\nend\n");
        let hint = near_miss_hint(&content, &block).expect("must diagnose");
        assert!(hint.contains('…'), "snippets must be truncated: {hint}");
    }
}
