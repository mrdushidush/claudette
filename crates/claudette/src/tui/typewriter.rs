//! Typewriter animation for fenced code blocks in the chat pane.
//!
//! Advances 12 bytes per tick (~240 chars/sec at 50 ms/frame). Adapted from
//! BCF `src/tui.rs` via `claudettes-forge` — same boundary-safe advance
//! logic, same CHARS_PER_TICK.
//!
//! Lifted as part of the import sweep 2026-05-19 (Phase 1 of
//! `docs/sprint_import_2026_05_19.md`). Helper is library-only until a
//! follow-up sprint wires it into `tui/render.rs` for streamed code-fence
//! rendering — until then `dead_code` is permitted so the lift compiles
//! clean and the wire-up sprint can move with one diff.

#![allow(dead_code)]

const CHARS_PER_TICK: usize = 12;

/// Animated state for a single code-block body.
///
/// Call `start(content)` when a new code block should be animated.
/// Call `advance()` once per frame tick. Read `display()` for the visible
/// slice to render. When `is_complete()` returns true, render the full block
/// normally (no cursor needed).
pub struct TypewriterState {
    /// The full code block body being animated.
    full: String,
    /// Byte position up to which the content is currently revealed.
    pos: usize,
}

impl TypewriterState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            full: String::new(),
            pos: 0,
        }
    }

    /// Start animating a new code block from the beginning.
    pub fn start(&mut self, content: String) {
        self.full = content;
        self.pos = 0;
    }

    /// True when there is content to animate and it has not finished yet.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.full.is_empty() && self.pos < self.full.len()
    }

    /// True when the animation has run to completion (or no content is set).
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.full.is_empty() || self.pos >= self.full.len()
    }

    /// Advance the animation by up to `CHARS_PER_TICK` bytes, always stopping
    /// on a valid UTF-8 char boundary.
    pub fn advance(&mut self) {
        if self.is_complete() {
            return;
        }
        let remaining = self.full.len().saturating_sub(self.pos);
        let target = self.pos + CHARS_PER_TICK.min(remaining);
        let safe = if target >= self.full.len() {
            self.full.len()
        } else {
            let mut pos = target;
            while pos < self.full.len() && !self.full.is_char_boundary(pos) {
                pos += 1;
            }
            pos
        };
        self.pos = safe;
    }

    /// The currently-visible slice of the code block body.
    #[must_use]
    pub fn display(&self) -> &str {
        let safe_pos = {
            let mut p = self.pos.min(self.full.len());
            while p > 0 && !self.full.is_char_boundary(p) {
                p -= 1;
            }
            p
        };
        &self.full[..safe_pos]
    }

    /// The full content (used to identify which code block to animate).
    #[must_use]
    pub fn full(&self) -> &str {
        &self.full
    }
}

impl Default for TypewriterState {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract the body of the first fenced code block from `text`.
///
/// Returns `None` if no complete `` ``` `` block is found.
#[must_use]
pub fn extract_first_code_block(text: &str) -> Option<String> {
    let mut in_block = false;
    let mut body = String::new();
    for line in text.lines() {
        if !in_block {
            if line.trim_start().starts_with("```") {
                in_block = true;
            }
        } else if line.trim() == "```" {
            return Some(body);
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    None
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_reveals_content_incrementally() {
        let mut tw = TypewriterState::new();
        tw.start("a".repeat(30));
        assert!(tw.is_active());
        tw.advance();
        assert_eq!(tw.display().len(), 12);
        tw.advance();
        assert_eq!(tw.display().len(), 24);
        tw.advance();
        assert!(tw.is_complete());
        assert_eq!(tw.display(), "a".repeat(30));
    }

    #[test]
    fn advance_handles_utf8_boundary() {
        let mut tw = TypewriterState::new();
        tw.start("€€€€".to_string());
        tw.advance();
        assert!(tw.display().chars().all(|c| c == '€'));
    }

    #[test]
    fn extract_returns_none_for_no_block() {
        assert_eq!(extract_first_code_block("no code here"), None);
    }

    #[test]
    fn extract_returns_body_of_first_block() {
        let text = "text\n```rust\nfn main() {}\n```\nmore";
        let body = extract_first_code_block(text).unwrap();
        assert_eq!(body, "fn main() {}\n");
    }

    #[test]
    fn incomplete_block_returns_none() {
        let text = "```rust\nfn foo() {}";
        assert_eq!(extract_first_code_block(text), None);
    }
}
