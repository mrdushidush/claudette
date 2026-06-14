//! Human-readable, colored previews of edit-tool inputs for the danger gate.
//!
//! The `[y/N]` permission prompt used to dump an edit tool's raw JSON payload
//! (`{"path":"…","before":"…\n…","after":"…"}`) on one line, with the `\n` and
//! `\\` escaped — a wall of text that's effectively unreviewable. This turns the
//! payload of `apply_diff` / `edit_file` / `apply_patch` into a unified-diff
//! style block instead: a file header, red removals (`-`), green additions
//! (`+`), and a little dim context around the changed lines, with real newlines.
//!
//! Color is handled by the global `colored` override (see [`crate::theme::init`]),
//! so piped / non-TTY output renders as plain text. Nothing is truncated — the
//! full before/after (or full diff) is shown, preserving the property the raw
//! line-dump had: an adversarial payload can't hide content past a preview edge.

use serde_json::Value;

use crate::theme;

/// Build a colored, unified-diff-style preview of an edit tool's `input`, or
/// `None` when `tool_name` is not an edit tool or the input doesn't parse (the
/// caller falls back to the raw line dump). Returned lines are colored but NOT
/// indented — the caller adds its own leading whitespace.
#[must_use]
pub fn render(tool_name: &str, input: &str) -> Option<Vec<String>> {
    match tool_name {
        "apply_diff" => render_replacement(input, "before", "after"),
        "edit_file" => render_replacement(input, "old_text", "new_text"),
        "apply_patch" => render_unified(input),
        _ => None,
    }
}

/// Render a find-and-replace edit (apply_diff / edit_file) as a single hunk:
/// the common leading/trailing lines become dim context, the differing middle
/// shows as `-` (old) then `+` (new).
fn render_replacement(input: &str, old_key: &str, new_key: &str) -> Option<Vec<String>> {
    let v: Value = serde_json::from_str(input).ok()?;
    let path = v.get("path").and_then(Value::as_str)?;
    let old = v.get(old_key).and_then(Value::as_str)?;
    let new = v.get(new_key).and_then(Value::as_str)?;

    let old_lines: Vec<&str> = old.split('\n').collect();
    let new_lines: Vec<&str> = new.split('\n').collect();

    // Trim common leading/trailing lines so only the changed middle is marked,
    // surrounded by a little dim context — a clean single hunk.
    let prefix = common_prefix_len(&old_lines, &new_lines);
    let max_suffix = (old_lines.len() - prefix).min(new_lines.len() - prefix);
    let suffix = common_suffix_len(&old_lines[prefix..], &new_lines[prefix..]).min(max_suffix);

    let mut out = Vec::new();
    out.push(theme::accent(path).to_string());

    for line in &old_lines[..prefix] {
        out.push(theme::dim(&format!("  {line}")).to_string());
    }
    for line in &old_lines[prefix..old_lines.len() - suffix] {
        out.push(theme::diff_del(&format!("- {line}")).to_string());
    }
    for line in &new_lines[prefix..new_lines.len() - suffix] {
        out.push(theme::diff_add(&format!("+ {line}")).to_string());
    }
    for line in &old_lines[old_lines.len() - suffix..] {
        out.push(theme::dim(&format!("  {line}")).to_string());
    }
    Some(out)
}

/// Render an apply_patch unified diff with per-line coloring: file headers and
/// `@@` hunk markers stand out, `+`/`-` lines are green/red, context is dim.
fn render_unified(input: &str) -> Option<Vec<String>> {
    let v: Value = serde_json::from_str(input).ok()?;
    let diff = v.get("diff").and_then(Value::as_str)?;

    let mut out = Vec::new();
    if v.get("dry_run").and_then(Value::as_bool) == Some(true) {
        out.push(theme::dim("(dry run — validate only)").to_string());
    }
    for line in diff.split('\n') {
        let colored = if line.starts_with("+++")
            || line.starts_with("---")
            || line.starts_with("diff ")
            || line.starts_with("index ")
        {
            theme::accent(line).to_string()
        } else if line.starts_with("@@") {
            theme::info(line).to_string()
        } else if line.starts_with('+') {
            theme::diff_add(line).to_string()
        } else if line.starts_with('-') {
            theme::diff_del(line).to_string()
        } else {
            theme::dim(line).to_string()
        };
        out.push(colored);
    }
    Some(out)
}

fn common_prefix_len(a: &[&str], b: &[&str]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

fn common_suffix_len(a: &[&str], b: &[&str]) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: a `+`/`-` line for the change, full content shown.
    #[test]
    fn apply_diff_renders_a_hunk() {
        let input = r#"{"path":"src/lib.rs","before":"let x = 1;\n","after":"let x = 2;\n"}"#;
        let out = render("apply_diff", input).expect("apply_diff should render");
        let joined = out.join("\n");
        assert!(joined.contains("src/lib.rs"), "header missing: {joined}");
        assert!(joined.contains("- let x = 1;"), "removal missing: {joined}");
        assert!(
            joined.contains("+ let x = 2;"),
            "addition missing: {joined}"
        );
    }

    /// Common leading/trailing lines become context, not noise: only the
    /// changed middle is marked `-`/`+`.
    #[test]
    fn common_lines_become_context_not_markers() {
        let input = r#"{"path":"f","before":"a\nb\nc","after":"a\nB\nc"}"#;
        let out = render("apply_diff", input).expect("should render");
        let joined = out.join("\n");
        assert!(joined.contains("- b"), "changed old line missing: {joined}");
        assert!(joined.contains("+ B"), "changed new line missing: {joined}");
        // 'a' and 'c' are unchanged context — never marked - or +.
        assert!(
            !joined.contains("- a") && !joined.contains("+ a"),
            "{joined}"
        );
        assert!(
            !joined.contains("- c") && !joined.contains("+ c"),
            "{joined}"
        );
    }

    #[test]
    fn edit_file_uses_old_new_text_keys() {
        let input = r#"{"path":"f","old_text":"foo","new_text":"bar"}"#;
        let out = render("edit_file", input).expect("edit_file should render");
        let joined = out.join("\n");
        assert!(joined.contains("- foo"), "{joined}");
        assert!(joined.contains("+ bar"), "{joined}");
    }

    #[test]
    fn apply_patch_colors_unified_diff() {
        let input = r#"{"diff":"--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n"}"#;
        let out = render("apply_patch", input).expect("apply_patch should render");
        let joined = out.join("\n");
        assert!(
            joined.contains("@@ -1 +1 @@"),
            "hunk header missing: {joined}"
        );
        assert!(
            joined.contains("-old") && joined.contains("+new"),
            "{joined}"
        );
    }

    #[test]
    fn non_edit_tool_returns_none() {
        assert!(render("bash", r#"{"command":"ls"}"#).is_none());
        assert!(render("read_file", r#"{"path":"f"}"#).is_none());
    }

    #[test]
    fn unparseable_or_incomplete_input_returns_none() {
        assert!(render("apply_diff", "not json").is_none());
        assert!(render("apply_diff", r#"{"path":"f","before":"x"}"#).is_none());
        assert!(render("apply_patch", r#"{"dry_run":true}"#).is_none());
    }
}
