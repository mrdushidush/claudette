//! Colour + emoji theme for the claudette REPL.
//!
//! Centralises every ANSI style and emoji glyph the secretary's terminal UI
//! uses, so swapping the palette or stripping the icons later is a one-file
//! change. Auto-disables colour when stderr is not a TTY (so piping into a
//! file or `tee` produces clean text), and honours `NO_COLOR` / `CLICOLOR`
//! via `colored`'s built-in env handling.
//!
//! All public helpers are intentionally non-generic (`&str -> ColoredString`)
//! so they can be coerced to `fn` pointers and stuffed into arrays for tests
//! and bulk styling loops.

use std::io::IsTerminal;
use std::sync::Once;

use colored::{ColoredString, Colorize};

// === Emoji glyphs ============================================================
//
// Kept as plain `&str` rather than `char` because most of these are multi-code-
// point grapheme clusters and `char` would lose data. The REPL prints them via
// `write!`/`println!`, never indexes into them.

/// Assistant identity / brand glyph used in greeting and the streaming prefix.
pub const ROBOT: &str = "🤖";
/// Note created / note list.
pub const NOTE: &str = "📝";
/// Todo added / todo completed.
pub const TODO: &str = "✅";
/// File-ops glyph.
pub const FILE: &str = "📄";
/// Time tool glyph.
pub const TIME: &str = "⏱";
/// Memory / context glyph.
pub const BRAIN: &str = "🧠";
/// Session save glyph.
pub const SAVE: &str = "💾";
/// Tokens-per-turn / fast-path glyph.
pub const BOLT: &str = "⚡";
/// Generic checkmark for "ok / done".
pub const OK_GLYPH: &str = "✓";
/// Generic warning glyph.
pub const WARN_GLYPH: &str = "⚠";
/// Generic error glyph.
pub const ERR_GLYPH: &str = "❌";
/// Search / inspect glyph.
pub const MAG: &str = "🔍";
/// "New thing" / capability glyph.
pub const SPARKLES: &str = "✨";
/// Settings / config glyph.
pub const GEAR: &str = "⚙";
/// Single-character prompt arrow used in the REPL.
pub const PROMPT_ARROW: &str = "›";

// === Colour init =============================================================

/// Initialise the global colour override based on whether stderr is a TTY.
/// Idempotent — safe to call multiple times. Call once at REPL startup; the
/// single-shot path can skip it (its stats line is plain enough that the
/// auto-detect built into `colored` is fine).
pub fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if !std::io::stderr().is_terminal() {
            colored::control::set_override(false);
        }
    });
}

// === Style helpers ===========================================================

/// Cyan + bold accent — section headers and the REPL prompt arrow.
#[must_use]
pub fn accent(s: &str) -> ColoredString {
    s.cyan().bold()
}

/// Bright blue info — non-error status lines (`[turn iter=…]`).
#[must_use]
pub fn info(s: &str) -> ColoredString {
    s.bright_blue()
}

/// Yellow warning — non-fatal degraded states (e.g. "session save failed").
#[must_use]
pub fn warn(s: &str) -> ColoredString {
    s.yellow()
}

/// Red + bold error — fatal turn errors and command failures.
#[must_use]
pub fn error(s: &str) -> ColoredString {
    s.red().bold()
}

/// Dimmed grey — muted detail attached to status lines.
#[must_use]
pub fn dim(s: &str) -> ColoredString {
    s.dimmed()
}

/// Green — successful actions (`session saved`, `note created`).
#[must_use]
pub fn ok(s: &str) -> ColoredString {
    s.green()
}

/// Magenta + bold — assistant brand text in greetings.
#[must_use]
pub fn brand(s: &str) -> ColoredString {
    s.magenta().bold()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `init` should be safe to call repeatedly. Verifies the `Once` guard.
    #[test]
    fn init_is_idempotent() {
        init();
        init();
        init();
    }

    /// All style helpers must round-trip the inner text intact (whether or not
    /// colour is enabled in the test environment). Uses an explicit fn-pointer
    /// array to confirm the helpers all share the same signature.
    #[test]
    fn all_helpers_round_trip_text() {
        let funcs: [fn(&str) -> ColoredString; 7] =
            [accent, info, warn, error, dim, ok, brand];
        for f in funcs {
            let s = f("hello");
            assert!(
                format!("{s}").contains("hello"),
                "helper dropped the inner text"
            );
        }
    }

    /// Emoji constants must be non-empty so callers can rely on them in
    /// `write!` without producing visual gaps.
    #[test]
    fn emoji_constants_are_non_empty() {
        let glyphs = [
            ROBOT,
            NOTE,
            TODO,
            FILE,
            TIME,
            BRAIN,
            SAVE,
            BOLT,
            OK_GLYPH,
            WARN_GLYPH,
            ERR_GLYPH,
            MAG,
            SPARKLES,
            GEAR,
            PROMPT_ARROW,
        ];
        for g in glyphs {
            assert!(!g.is_empty(), "empty glyph found");
        }
    }
}
