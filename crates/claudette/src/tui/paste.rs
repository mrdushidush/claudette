//! Large-paste buffering — writes oversized pastes to a temp file so the
//! input widget stays responsive. Ported from `claudettes-forge` (originally
//! tacticode `src/tui/paste.rs`).
//!
//! Lifted as part of the import sweep 2026-05-19 (Phase 1 of
//! `docs/sprint_import_2026_05_19.md`). Temp-dir prefix changed from
//! `claudettes-forge` to `claudette`.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const PASTE_THRESHOLD: usize = 500;

/// Handles large pastes by writing to a temp file.
///
/// When active, the input bar shows a compact preview instead of the raw text.
/// The temp file is deleted when `clear()` is called or the struct is dropped.
pub struct PasteFile {
    path: Option<PathBuf>,
    char_count: usize,
    preview: String,
}

impl PasteFile {
    #[must_use]
    pub fn new() -> Self {
        Self {
            path: None,
            char_count: 0,
            preview: String::new(),
        }
    }

    /// Store `text` in a temp file if it exceeds the threshold.
    ///
    /// Returns `true` when the text was stored (caller should not also append
    /// it to the input buffer). Returns `false` when the text is small enough
    /// to be appended directly.
    pub fn try_store(&mut self, text: &str) -> bool {
        if text.len() <= PASTE_THRESHOLD {
            return false;
        }

        let dir = std::env::temp_dir().join("claudette");
        let _ = fs::create_dir_all(&dir);

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let file_path = dir.join(format!("paste-{nanos}.txt"));

        if fs::write(&file_path, text).is_ok() {
            self.char_count = text.len();
            let preview: String = text
                .chars()
                .take(60)
                .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
                .collect();
            self.preview = preview;
            self.path = Some(file_path);
            true
        } else {
            false
        }
    }

    /// Read the full content back from the temp file.
    #[must_use]
    pub fn retrieve(&self) -> Option<String> {
        self.path.as_ref().and_then(|p| fs::read_to_string(p).ok())
    }

    /// Returns `true` if a paste file is currently active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.path.is_some()
    }

    /// One-line display string for the input bar when a paste is active.
    #[must_use]
    pub fn display(&self) -> String {
        format!("{}… [{} chars from paste]", self.preview, self.char_count)
    }

    /// Clear state and delete the temp file.
    pub fn clear(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
        self.char_count = 0;
        self.preview.clear();
    }
}

impl Default for PasteFile {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PasteFile {
    fn drop(&mut self) {
        self.clear();
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_text_not_stored() {
        let mut pf = PasteFile::new();
        let stored = pf.try_store("short");
        assert!(!stored);
        assert!(!pf.is_active());
    }

    #[test]
    fn large_text_stored_and_retrieved() {
        let mut pf = PasteFile::new();
        let big: String = "x".repeat(600);
        let stored = pf.try_store(&big);
        assert!(stored);
        assert!(pf.is_active());
        assert_eq!(pf.retrieve(), Some(big));
    }

    #[test]
    fn clear_removes_active_state() {
        let mut pf = PasteFile::new();
        let big: String = "y".repeat(600);
        pf.try_store(&big);
        pf.clear();
        assert!(!pf.is_active());
        assert_eq!(pf.retrieve(), None);
    }

    #[test]
    fn display_shows_preview_and_count() {
        let mut pf = PasteFile::new();
        let big: String = "a".repeat(600);
        pf.try_store(&big);
        let disp = pf.display();
        assert!(disp.contains("600 chars from paste"));
    }
}
