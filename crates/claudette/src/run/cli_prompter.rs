//! Interactive CLI permission prompter + status-line helpers (Wave C3 — split
//! out of run.rs).
//!
//! `CliPrompter` is the REPL/single-shot implementation of the
//! `PermissionPrompter` trait (the `[y/N]` gate, with a colored diff preview
//! and single-key fast path); plus the small pure helpers that format the
//! status line (`humanize_tokens`, `format_ctx_gauge`) and the empty-response
//! retry nudge.

use std::io::{self, Write};

use crate::theme;
use crate::{PermissionPromptDecision, PermissionPrompter, PermissionRequest};

/// Render a token count compactly for the status line: `840`, `1k`, `64k`.
/// Uses a 1024 base so a power-of-two context window prints round — a 65536
/// `num_ctx` shows as `64k`, matching how the window is configured.
fn humanize_tokens(n: usize) -> String {
    if n < 1024 {
        n.to_string()
    } else {
        format!("{}k", (n as f64 / 1024.0).round() as usize)
    }
}

/// Build the post-turn context-window gauge, e.g. `ctx ~30k/64k (47%)`.
/// `used` is the heuristic session-token estimate (`estimate_session_tokens`)
/// — the SAME metric the auto-compaction gate uses; it omits the system prompt
/// and tool schemas, hence the leading `~`. `num_ctx` is the brain's window.
pub(crate) fn format_ctx_gauge(used: usize, num_ctx: u32) -> String {
    let total = num_ctx as usize;
    let pct = used.saturating_mul(100).checked_div(total).unwrap_or(0);
    format!(
        "ctx ~{}/{} ({}%)",
        humanize_tokens(used),
        humanize_tokens(total),
        pct
    )
}

/// Interactive CLI prompter. Prints tool name + a preview of the input,
/// asks `[y/N]`, reads one line from stdin. Used by the REPL and by
/// spawned agents in normal mode (dangerous tools bubble up to the user).
/// The single-shot path passes `None` (no prompter → dangerous tools denied).
pub struct CliPrompter;

impl PermissionPrompter for CliPrompter {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
        // Clear the activity spinner before the approval prompt takes the
        // screen (no-op unless the REPL enabled it).
        crate::status::global().on_prompt();
        let stderr = io::stderr();
        let mut err = stderr.lock();
        let _ = writeln!(err);
        let input_chars = request.input.chars().count();
        let _ = writeln!(
            err,
            "  {} {} wants to run ({} chars):",
            theme::warn(theme::WARN_GLYPH),
            theme::accent(&request.tool_name),
            input_chars
        );
        // Show the full command. The old code truncated at 200 chars, which
        // let an adversary-crafted payload hide past the preview edge while
        // bash ran the complete input. Split on newlines so multi-line
        // commands stay readable. `str::lines()` handles a trailing-newline-
        // less single-line case correctly — yields the one line.
        if request.input.is_empty() {
            let _ = writeln!(err, "    {}", theme::dim("(empty input)"));
        } else if let Some(diff_lines) =
            crate::diff_preview::render(&request.tool_name, &request.input)
        {
            // Edit tools (apply_diff / edit_file / apply_patch): show a colored
            // unified-diff preview instead of the escaped-JSON wall. Full
            // content, nothing truncated.
            for line in diff_lines {
                let _ = writeln!(err, "    {line}");
            }
        } else {
            for line in request.input.lines() {
                let _ = writeln!(err, "    {}", theme::dim(line));
            }
        }
        let _ = write!(err, "  Allow? [y/N · or type a redirect] ");
        let _ = err.flush();

        // Interactive terminal: accept a single keypress so `y` allows / `n`
        // denies without Enter, while any other key opens a free-text redirect.
        // Falls through to the line reader below when stdin isn't a TTY (piped /
        // scripted / spawned agent) or raw mode is unavailable.
        use std::io::IsTerminal as _;
        if io::stdin().is_terminal() {
            if let Some(decision) = read_single_key(&mut err) {
                return decision;
            }
        }

        let stdin = io::stdin();
        let mut buf = String::new();
        match stdin.read_line(&mut buf) {
            Ok(_) => gate_line_decision(&buf),
            Err(_) => PermissionPromptDecision::Deny {
                reason: "could not read user input".to_string(),
            },
        }
    }
}

/// Classify a full line typed at the `[y/N]` permission gate.
///
/// - `y` / `yes` → allow.
/// - empty / `n` / `no` → plain deny.
/// - anything else → deny, but forward the typed text to the model as a
///   *redirect*: the tool is refused, yet the user's instruction is handed
///   back (via the deny reason, which becomes an error `tool_result`) so the
///   model does what was asked instead of just stopping. Pure so the
///   classification is unit-testable without a TTY.
fn gate_line_decision(line: &str) -> PermissionPromptDecision {
    let trimmed = line.trim();
    let lower = trimmed.to_lowercase();
    if lower == "y" || lower == "yes" {
        PermissionPromptDecision::Allow
    } else if trimmed.is_empty() || lower == "n" || lower == "no" {
        PermissionPromptDecision::Deny {
            reason: "user denied permission".to_string(),
        }
    } else {
        PermissionPromptDecision::Deny {
            reason: format!(
                "The user declined to run this tool and gave this instruction \
                 instead — follow it before continuing: {trimmed}"
            ),
        }
    }
}

/// First-keypress confirmation for the `[y/N]` danger gate on an interactive
/// terminal. `y`/`Y` allows and `n`/`N` denies immediately (no Enter needed);
/// Esc / Enter / Ctrl-C / Ctrl-D deny; **any other printable key opens a
/// free-text redirect** — that first character plus the rest of the line
/// (read in cooked mode) is classified by [`gate_line_decision`], so the user
/// can type an instruction for the model instead of a bare allow/deny.
/// Returns `None` if raw mode can't be enabled, so the caller falls back to
/// the line reader (which is redirect-aware too). Mirrors the TUI prompt's key
/// handling in `tui.rs`.
fn read_single_key(err: &mut impl io::Write) -> Option<PermissionPromptDecision> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    enable_raw_mode().ok()?;
    // Read the first decisive keypress.
    let first = loop {
        match event::read() {
            // Windows fires Press + Release — act on Press only.
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => break Some(k),
            Ok(_) => {}           // resize / mouse / key-release — keep waiting
            Err(_) => break None, // read error denies
        }
    };
    // ALWAYS restore cooked mode before any echo / line read below.
    let _ = disable_raw_mode();

    match first.map(|k| (k.code, k.modifiers)) {
        // `y` / `Y` → allow instantly.
        Some((KeyCode::Char('y' | 'Y'), KeyModifiers::NONE | KeyModifiers::SHIFT)) => {
            let _ = writeln!(err, "y");
            Some(PermissionPromptDecision::Allow)
        }
        // Any printable char that isn't `y`/`n` opens a redirect: echo it,
        // read the rest of the line in cooked mode (so normal line editing
        // works), then classify the whole line. The terminal echoes the rest
        // and the trailing newline, so output lands on a fresh row after.
        Some((KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT))
            if c != 'n' && c != 'N' =>
        {
            let _ = write!(err, "{c}");
            let _ = err.flush();
            let mut rest = String::new();
            if io::stdin().read_line(&mut rest).is_err() {
                return Some(PermissionPromptDecision::Deny {
                    reason: "could not read user input".to_string(),
                });
            }
            let mut line = String::with_capacity(rest.len() + 1);
            line.push(c);
            line.push_str(&rest);
            Some(gate_line_decision(&line))
        }
        // `n`/`N`, Esc, Enter, Ctrl-C/Ctrl-D, read error — deny.
        _ => {
            let _ = writeln!(err, "n");
            Some(PermissionPromptDecision::Deny {
                reason: "user denied permission".to_string(),
            })
        }
    }
}

/// The nudge message appended when the model returns an empty response.
/// Tells the model to use `enable_tools` instead of giving up.
pub(crate) const EMPTY_RESPONSE_NUDGE: &str =
    "Your response was empty. If you need a tool that isn't available, \
     call enable_tools(group) to load it first, then call the tool. \
     Otherwise, answer the question directly with text.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PermissionPromptDecision;

    #[test]
    fn gate_line_decision_allows_on_yes() {
        assert_eq!(gate_line_decision("y"), PermissionPromptDecision::Allow);
        assert_eq!(
            gate_line_decision("  Y \n"),
            PermissionPromptDecision::Allow
        );
        assert_eq!(gate_line_decision("yes"), PermissionPromptDecision::Allow);
    }

    #[test]
    fn gate_line_decision_plain_deny_on_empty_or_no() {
        let plain = PermissionPromptDecision::Deny {
            reason: "user denied permission".to_string(),
        };
        assert_eq!(gate_line_decision(""), plain);
        assert_eq!(gate_line_decision("\n"), plain);
        assert_eq!(gate_line_decision("n"), plain);
        assert_eq!(gate_line_decision("NO"), plain);
    }

    #[test]
    fn gate_line_decision_forwards_redirect_text() {
        match gate_line_decision("edit foo.rs instead, leave bar.rs alone\n") {
            PermissionPromptDecision::Deny { reason } => {
                assert!(
                    reason.contains("edit foo.rs instead, leave bar.rs alone"),
                    "redirect must carry the user's instruction: {reason}"
                );
                assert!(
                    reason.contains("follow it"),
                    "redirect must tell the model to act on it: {reason}"
                );
            }
            PermissionPromptDecision::Allow => {
                panic!("expected a deny-with-redirect, got Allow")
            }
        }
    }

    #[test]
    fn humanize_tokens_compacts_with_1024_base() {
        assert_eq!(humanize_tokens(840), "840");
        assert_eq!(humanize_tokens(1024), "1k");
        assert_eq!(humanize_tokens(65536), "64k");
        assert_eq!(humanize_tokens(31000), "30k");
    }

    #[test]
    fn format_ctx_gauge_shows_used_window_and_percent() {
        // 32768 / 65536 = exactly 50%
        assert_eq!(format_ctx_gauge(32768, 65536), "ctx ~32k/64k (50%)");
        // a zero window must not divide by zero
        assert_eq!(format_ctx_gauge(100, 0), "ctx ~100/0 (0%)");
    }
}
