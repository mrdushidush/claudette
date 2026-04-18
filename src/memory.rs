//! Optional user-supplied "memory file" loader.
//!
//! Reads `~/.claudette/CLAUDETTE.MD` if present and returns its content
//! clipped to a small character budget. The cap exists because the
//! secretary's system prompt is intentionally terse — qwen3.5:9b hallucinates
//! rather than calling tools when given multi-paragraph directives (measured
//! 2026-04-08, see `prompt.rs`). Letting the user attach 800 chars of stable
//! background (name, timezone, projects, preferences) is the sweet spot:
//! enough to be useful, small enough to keep the model grounded.
//!
//! Memory is best-effort UX, never load-bearing — read errors are silenced
//! and the runtime still starts cleanly without a memory file. The REPL
//! `/reload` command re-reads the file without needing a process restart.

use std::path::{Path, PathBuf};

/// Hard cap on memory content, measured in Unicode `char` count (not bytes).
/// 800 chars ≈ 200 tokens — safely below the threshold where verbose system
/// prompts start causing tool-call hallucination on qwen3.5:9b.
pub const MAX_MEMORY_CHARS: usize = 800;

/// Resolve where the memory file lives. Honors the `CLAUDETTE_MEMORY` env
/// var (full path) for tests and power-users; otherwise falls back to
/// `<HOME>/.claudette/CLAUDETTE.MD`.
#[must_use]
pub fn default_memory_path() -> PathBuf {
    if let Ok(custom) = std::env::var("CLAUDETTE_MEMORY") {
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claudette").join("CLAUDETTE.MD")
}

/// Try to load the memory file from the default path. Returns `None` if the
/// file is missing, unreadable, or empty after trimming.
#[must_use]
pub fn try_load_memory() -> Option<String> {
    try_load_memory_at(&default_memory_path())
}

/// Same as `try_load_memory` but reads from a caller-supplied path. Lets
/// tests avoid touching the process-global `CLAUDETTE_MEMORY` env var.
#[must_use]
pub fn try_load_memory_at(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(cap_memory(trimmed))
}

/// Apply the `MAX_MEMORY_CHARS` cap. If the input is over budget, truncate at
/// a Unicode `char` boundary (NEVER a raw byte slice — multibyte glyphs would
/// blow up) and append a visible `[truncated]` marker so both the model and
/// the user (via `/memory`) can see something was clipped.
fn cap_memory(content: &str) -> String {
    let count = content.chars().count();
    if count <= MAX_MEMORY_CHARS {
        return content.to_string();
    }
    let truncated: String = content.chars().take(MAX_MEMORY_CHARS).collect();
    format!("{truncated}\n…[truncated to {MAX_MEMORY_CHARS}/{count} chars]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a unique temp file path under `claudette-test-memory/` and
    /// write `contents` into it. Caller is responsible for cleanup.
    fn temp_memory_file(label: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("claudette-test-memory");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!(
            "{label}-{}-{}.md",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn missing_file_returns_none() {
        let path = std::env::temp_dir().join("claudette-no-such-memory-xyz.md");
        let _ = std::fs::remove_file(&path);
        assert!(try_load_memory_at(&path).is_none());
    }

    #[test]
    fn empty_file_returns_none() {
        let path = temp_memory_file("empty", "   \n  \t \n");
        assert!(try_load_memory_at(&path).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn small_file_loads_verbatim_after_trim() {
        let path = temp_memory_file("small", "  hello world  \n");
        let loaded = try_load_memory_at(&path).expect("expected Some");
        assert_eq!(loaded, "hello world");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn oversize_file_is_truncated_with_marker() {
        let big = "a".repeat(MAX_MEMORY_CHARS + 200);
        let path = temp_memory_file("big", &big);
        let loaded = try_load_memory_at(&path).expect("expected Some");
        assert!(loaded.starts_with(&"a".repeat(MAX_MEMORY_CHARS)));
        assert!(loaded.contains("[truncated"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cap_memory_under_budget_is_identity() {
        let s = "small note";
        assert_eq!(cap_memory(s), s);
    }

    #[test]
    fn cap_memory_truncates_on_char_boundary_for_multibyte() {
        // Each robot emoji is 4 bytes but 1 char. We want to cap by char
        // count, not byte count, so we don't slice through a UTF-8 boundary.
        let multi: String = "🤖".repeat(MAX_MEMORY_CHARS + 50);
        let capped = cap_memory(&multi);
        let robot_count = capped.chars().filter(|c| *c == '🤖').count();
        assert!(
            robot_count <= MAX_MEMORY_CHARS,
            "kept {robot_count} robots, expected ≤{MAX_MEMORY_CHARS}"
        );
        assert!(capped.contains("[truncated"));
    }

    #[test]
    fn cap_memory_exactly_at_budget_passes_through() {
        let s = "x".repeat(MAX_MEMORY_CHARS);
        let capped = cap_memory(&s);
        assert_eq!(capped.chars().count(), MAX_MEMORY_CHARS);
        assert!(!capped.contains("[truncated"));
    }
}
