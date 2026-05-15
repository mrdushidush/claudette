//! Harmony / Qwen-3.6-style chat-template separator stripping.
//!
//! Some LM Studio quants of Qwen-style models leak the channel/message
//! markers (`<|channel|>thought<|message|>`, `<|channel>thought<channel|>`,
//! `<|end|>`, …) that the chat template is supposed to consume internally.
//! Without this strip they show up as visible text in the response.
//!
//! Extracted from `api.rs` on 2026-05-15 — the parent was over 2800 lines
//! and the audit flagged the size as a maintainability problem. Harmony
//! stripping has no dependency on the HTTP client or any of the chat-body
//! shaping logic, so it lives here as a self-contained text utility.

/// Strip Harmony / Qwen-3.6-style chat-template separators that occasionally
/// leak through into the OpenAI-compat `content` field.
///
/// Conservative match rule: a token is treated as a separator only if it
/// has the shape `<…>` AND contains at least one `|` adjacent to either
/// angle bracket. So `<a>`, `<div>`, `<MyType>` etc. are left alone, while
/// `<|x|>`, `<|x>`, and `<x|>` are stripped.
///
/// Fenced code blocks (lines starting with ```` ``` ````) are skipped
/// entirely so a user asking about chat templates gets verbatim output
/// inside their code samples.
pub(super) fn strip_harmony_separators(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;

    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
        } else {
            out.push_str(&strip_harmony_from_segment(line));
        }
    }
    out
}

/// Strip Harmony separator tokens from a single non-fenced segment. Operates
/// on bytes for the scan (markers are pure ASCII) but slices the original
/// `&str` at known-ASCII boundaries so multi-byte UTF-8 elsewhere in the
/// segment is preserved untouched.
fn strip_harmony_from_segment(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut last_emit = 0;

    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some(end) = try_match_harmony_run(bytes, i) {
                out.push_str(&s[last_emit..i]);
                last_emit = end;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[last_emit..]);
    out
}

/// Match a Harmony separator "run" starting at `<`: either a solo separator
/// like `<|end|>`, or a `<marker>name<marker>` triplet like the Qwen-3.6
/// channel pair `<|channel>thought<channel|>`. The triplet form treats the
/// inner identifier as part of the markup so it gets stripped along with
/// the brackets, fixing the leak where `thought` (the channel name) showed
/// up as visible output.
fn try_match_harmony_run(bytes: &[u8], start: usize) -> Option<usize> {
    let after_first = try_match_harmony(bytes, start)?;

    // Look for the triplet shape: identifier immediately after the first
    // marker, followed by another marker. If both are present, swallow the
    // whole triplet. Otherwise strip just the solo marker.
    let mut i = after_first;
    while i < bytes.len() && (bytes[i].is_ascii_lowercase() || bytes[i] == b'_') {
        i += 1;
    }
    if i > after_first && i < bytes.len() && bytes[i] == b'<' {
        if let Some(end) = try_match_harmony(bytes, i) {
            return Some(end);
        }
    }

    Some(after_first)
}

/// If `bytes[start..]` opens a Harmony separator, return the byte index just
/// past its closing `>`. The accepted shape is `<` + optional `|` +
/// `[a-z_]+` + optional `|` + `>` with at least one `|` present, so
/// ordinary `<tag>` text is never matched.
fn try_match_harmony(bytes: &[u8], start: usize) -> Option<usize> {
    debug_assert_eq!(bytes[start], b'<');
    let mut i = start + 1;
    let mut has_pipe = false;

    if i < bytes.len() && bytes[i] == b'|' {
        has_pipe = true;
        i += 1;
    }

    let name_start = i;
    while i < bytes.len() && (bytes[i].is_ascii_lowercase() || bytes[i] == b'_') {
        i += 1;
    }
    if i == name_start {
        return None;
    }

    if i < bytes.len() && bytes[i] == b'|' {
        has_pipe = true;
        i += 1;
    }

    if i < bytes.len() && bytes[i] == b'>' && has_pipe {
        Some(i + 1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_harmony_removes_qwen36_channel_pair() {
        // The exact pattern reported on unsloth/qwen3.6-35b-a3b via LM Studio.
        let input = "<|channel>thought<channel|>\nthe actual reply";
        assert_eq!(strip_harmony_separators(input), "\nthe actual reply");
    }

    #[test]
    fn strip_harmony_removes_symmetric_markers() {
        // Triplet swallows the channel-name identifier between paired
        // markers, then the trailing solo `<|end|>` is stripped on its own.
        let input = "<|channel|>analysis<|message|>real content<|end|>";
        assert_eq!(strip_harmony_separators(input), "real content");
    }

    #[test]
    fn strip_harmony_handles_underscored_names() {
        let input = "before <|tool_call|> after";
        assert_eq!(strip_harmony_separators(input), "before  after");
    }

    #[test]
    fn strip_harmony_leaves_html_tags_alone() {
        // No `|` inside — looks like ordinary HTML/XML, leave it.
        let input = "<a href=\"x\"><b>bold</b></a>";
        assert_eq!(strip_harmony_separators(input), input);
    }

    #[test]
    fn strip_harmony_leaves_plain_angle_brackets_alone() {
        let input = "if x < y && y > z";
        assert_eq!(strip_harmony_separators(input), input);
    }

    #[test]
    fn strip_harmony_preserves_fenced_code() {
        // A user asking about chat templates should see the markers verbatim
        // inside their code block. Outside the block the markers are stripped.
        let input = "<|end|> outside\n```\n<|channel|>thought<|message|>\n```\n<|end|> after";
        let expected = " outside\n```\n<|channel|>thought<|message|>\n```\n after";
        assert_eq!(strip_harmony_separators(input), expected);
    }

    #[test]
    fn strip_harmony_handles_marker_at_start_of_line() {
        let input = "<|end|>";
        assert_eq!(strip_harmony_separators(input), "");
    }

    #[test]
    fn strip_harmony_preserves_multibyte_utf8() {
        // The scanner walks bytes but slices on `<` boundaries (always
        // ASCII), so emoji and other multi-byte content must round-trip.
        let input = "héllo 🦀 <|end|> wörld";
        assert_eq!(strip_harmony_separators(input), "héllo 🦀  wörld");
    }

    #[test]
    fn strip_harmony_unbalanced_marker_with_uppercase_is_left_alone() {
        // Name must be `[a-z_]+`, so `<|Channel|>` is not recognised.
        let input = "<|Channel|>";
        assert_eq!(strip_harmony_separators(input), input);
    }

    #[test]
    fn strip_harmony_no_op_on_clean_text() {
        let input = "hello world\nthis is fine";
        assert_eq!(strip_harmony_separators(input), input);
    }
}
