//! REPL activity indicator.
//!
//! A single-line spinner drawn to **stderr** that tells the user what the
//! current turn is doing — `thinking…` while the model is generating with no
//! visible output yet, or `running <tool>…` while a tool executes — during the
//! "dead air" that a local backend's prompt-processing / JIT model reload
//! creates (often 5–30s). Without it, a tool-only turn prints nothing and the
//! user can't tell a working turn from a hang (today the only signal is the
//! LM Studio request log).
//!
//! The line clears itself the instant real output, an approval prompt, or the
//! end-of-turn status line needs the screen, so it never collides with
//! streamed text. Erasure is done by overwriting with spaces (portable — no
//! ANSI cursor codes), so it behaves on legacy Windows consoles too.
//!
//! It is a process-global that is a **no-op until [`StatusController::enable`]
//! is called** — and only the interactive REPL enables it, and only on a TTY.
//! The TUI, forge, sub-agents, one-shot mode, and tests never enable it, so
//! their output is byte-for-byte unchanged. This mirrors the no-op-until-wired
//! pattern the recall indexer already uses, and survives the mid-session
//! runtime rebuilds that `brain_selector` does for model fallback (the global
//! outlives any single `ConversationRuntime`).

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::theme;

/// Braille spinner frames.
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Redraw cadence — fast enough to feel alive, slow enough to stay cheap.
const TICK: Duration = Duration::from_millis(90);

/// What the current turn is doing — drives what (if anything) the spinner draws.
enum Phase {
    /// Nothing in flight — spinner quiet.
    Idle,
    /// Model is generating but has emitted no visible text yet.
    Thinking(Instant),
    /// A tool is executing.
    Tool(String, Instant),
    /// Model is streaming visible text — spinner stays quiet so it does not
    /// fight the text being written to stdout.
    Streaming,
    /// A `[y/N]` approval prompt owns the screen — spinner quiet.
    Prompt,
}

struct Inner {
    phase: Phase,
    frame: usize,
    /// Display-cell width of the transient line currently on screen, so it can
    /// be erased by overwriting exactly that many spaces.
    drawn: usize,
    /// True while the terminal cursor is hidden for spinning. The `\r` redraw
    /// every tick otherwise strobes the cursor at column 0 — hiding it is the
    /// standard fix. Restored on the next erase (i.e. the moment we go quiet).
    cursor_hidden: bool,
    /// True while we should strip leading newlines from the streamed text run
    /// that just started — many local chat templates emit a leading `\n`, which
    /// would otherwise show as a blank line right where the spinner was.
    swallow_leading_newline: bool,
}

/// Process-global activity indicator. All mutators are cheap no-ops until
/// [`StatusController::enable`] flips `enabled`.
pub struct StatusController {
    enabled: AtomicBool,
    started: AtomicBool,
    inner: Mutex<Inner>,
}

static GLOBAL: OnceLock<StatusController> = OnceLock::new();

/// The process-global activity indicator. No-op until `enable`d.
#[must_use]
pub fn global() -> &'static StatusController {
    GLOBAL.get_or_init(|| StatusController {
        enabled: AtomicBool::new(false),
        started: AtomicBool::new(false),
        inner: Mutex::new(Inner {
            phase: Phase::Idle,
            frame: 0,
            drawn: 0,
            cursor_hidden: false,
            swallow_leading_newline: false,
        }),
    })
}

impl StatusController {
    /// Turn the indicator on and spawn the single render thread. Idempotent —
    /// safe to call more than once. Call only from an interactive, TTY-backed
    /// REPL; everywhere else, leaving it disabled keeps all mutators no-op.
    pub fn enable(&'static self) {
        self.enabled.store(true, Ordering::SeqCst);
        if self.started.swap(true, Ordering::SeqCst) {
            return; // render thread already running
        }
        let _ = thread::Builder::new()
            .name("claudette-status".into())
            .spawn(run_spinner);
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Poisoned-lock recovery: a panic in another holder must not take the
        // indicator (a best-effort cosmetic) down with it.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Model turn begins — show `thinking…`.
    pub fn on_turn_start(&self) {
        self.transition(Phase::Thinking(Instant::now()), true);
    }

    /// Handle one streamed text delta: clear the spinner on the first delta of
    /// a text run (so it doesn't collide with the output), strip leading
    /// newlines until the first visible character of that run, and return the
    /// slice that should actually be written. When the indicator is disabled
    /// (everywhere but the interactive REPL) the delta is returned untouched,
    /// so other surfaces are byte-for-byte unchanged.
    pub fn on_text<'a>(&self, delta: &'a str) -> &'a str {
        if !self.enabled.load(Ordering::Relaxed) {
            return delta;
        }
        let mut g = self.lock();
        if !matches!(g.phase, Phase::Streaming) {
            // First delta of this text run — clear the spinner and arm the
            // leading-newline swallow for the run.
            self.erase(&mut g);
            g.phase = Phase::Streaming;
            g.swallow_leading_newline = true;
        }
        let (out, still_swallowing) = apply_swallow(g.swallow_leading_newline, delta);
        g.swallow_leading_newline = still_swallowing;
        out
    }

    /// A tool is about to run — show `running <name>…`.
    pub fn on_tool_start(&self, name: &str) {
        self.transition(Phase::Tool(name.to_string(), Instant::now()), true);
    }

    /// A tool finished — the model will generate next, so go back to thinking.
    pub fn on_tool_end(&self) {
        // Tool→Thinking are both active lines; relabel on the next tick rather
        // than erasing, to avoid a flicker.
        self.transition(Phase::Thinking(Instant::now()), false);
    }

    /// An approval prompt is about to take the screen — clear and go quiet.
    pub fn on_prompt(&self) {
        self.transition(Phase::Prompt, true);
    }

    /// Turn is over — clear the line so the post-turn status prints cleanly.
    pub fn on_turn_end(&self) {
        self.transition(Phase::Idle, true);
    }

    /// Erase the transient spinner line (if any) and print `line` dimmed to
    /// stderr — for one-off notices (e.g. the cold-start heads-up) that must
    /// not collide with an in-flight spinner. The phase is left unchanged,
    /// so an active spinner simply redraws below the note on its next tick.
    /// No-op when the indicator is disabled, which keeps every non-REPL
    /// surface (one-shot, forge, TUI, tests) byte-for-byte unchanged.
    pub fn print_note(&self, line: &str) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let mut g = self.lock();
        self.erase(&mut g);
        // Still holding the inner lock, so the render thread can't redraw
        // between the erase and this print.
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{}", theme::dim(line));
        let _ = err.flush();
    }

    fn transition(&self, phase: Phase, erase: bool) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let mut g = self.lock();
        if erase {
            self.erase(&mut g);
        }
        g.phase = phase;
    }

    /// Erase the transient line by overwriting its cells with spaces and
    /// returning the cursor to column 0. Caller holds the lock.
    fn erase(&self, g: &mut Inner) {
        if g.drawn == 0 && !g.cursor_hidden {
            return;
        }
        let mut err = std::io::stderr().lock();
        if g.drawn > 0 {
            let _ = write!(err, "\r{}\r", " ".repeat(g.drawn));
            g.drawn = 0;
        }
        if g.cursor_hidden {
            let _ = write!(err, "\x1b[?25h"); // show cursor
            g.cursor_hidden = false;
        }
        let _ = err.flush();
    }

    /// Draw `raw` (uncolored width) as a dimmed transient line, padding with
    /// spaces to cover any longer previous line. Caller holds the lock.
    fn draw(&self, g: &mut Inner, raw: &str) {
        let new = raw.chars().count();
        let pad = g.drawn.saturating_sub(new);
        let mut err = std::io::stderr().lock();
        if !g.cursor_hidden {
            let _ = write!(err, "\x1b[?25l"); // hide cursor while spinning
            g.cursor_hidden = true;
        }
        let _ = write!(err, "\r{}{}", theme::dim(raw), " ".repeat(pad));
        let _ = err.flush();
        g.drawn = new;
    }
}

/// Render the active-phase line. Pure (no I/O) so it can be unit-tested.
fn render_active(label: &str, frame_idx: usize, secs: u64) -> String {
    format!("{} {label} {secs}s", FRAMES[frame_idx % FRAMES.len()])
}

/// Given the run's current swallow flag and a text delta, return the slice to
/// actually write and the next swallow flag. While swallowing, strip leading
/// `\n`/`\r`; once a visible character is seen the flag clears. Pure so the
/// leading-newline logic is testable without enabling the global indicator.
fn apply_swallow(swallow: bool, delta: &str) -> (&str, bool) {
    if !swallow {
        return (delta, false);
    }
    let trimmed = delta.trim_start_matches(['\n', '\r']);
    // Keep swallowing only while we've still seen nothing but newlines.
    (trimmed, trimmed.is_empty())
}

/// Render thread: ticks forever, drawing the current phase's line while the
/// indicator is enabled and the phase is active. Quiet phases (`Idle`,
/// `Streaming`, `Prompt`) draw nothing — their line was already erased at the
/// transition that entered them.
fn run_spinner() {
    loop {
        thread::sleep(TICK);
        let c = global();
        if !c.enabled.load(Ordering::Relaxed) {
            continue;
        }
        let mut g = c.lock();
        let frame = g.frame;
        let line = match &g.phase {
            Phase::Thinking(t) => Some(render_active("thinking…", frame, t.elapsed().as_secs())),
            Phase::Tool(name, t) => Some(render_active(
                &format!("running {name}…"),
                frame,
                t.elapsed().as_secs(),
            )),
            Phase::Idle | Phase::Streaming | Phase::Prompt => None,
        };
        g.frame = g.frame.wrapping_add(1);
        if let Some(l) = line {
            c.draw(&mut g, &l);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_active_formats_frame_label_and_seconds() {
        assert_eq!(render_active("thinking…", 0, 12), "⠋ thinking… 12s");
        assert_eq!(
            render_active("running grep_search…", 2, 3),
            "⠹ running grep_search… 3s"
        );
    }

    #[test]
    fn apply_swallow_strips_leading_newlines_until_first_visible_char() {
        // Leading newlines dropped, flag clears once real text appears.
        assert_eq!(apply_swallow(true, "\n\nFound it"), ("Found it", false));
        // All-newline delta: nothing written, keep swallowing.
        assert_eq!(apply_swallow(true, "\n"), ("", true));
        // No leading newline: passthrough, flag clears.
        assert_eq!(apply_swallow(true, "Found it"), ("Found it", false));
        // Already past the leading edge: never touch later deltas.
        assert_eq!(apply_swallow(false, "\nmid-text"), ("\nmid-text", false));
    }

    #[test]
    fn render_active_wraps_frame_index() {
        // Index 10 wraps back to frame 0 (10 frames total).
        assert_eq!(render_active("thinking…", 10, 0), "⠋ thinking… 0s");
        assert_eq!(render_active("thinking…", 11, 1), "⠙ thinking… 1s");
    }

    #[test]
    fn disabled_controller_mutators_are_noops() {
        // A freshly-initialized global starts disabled; mutators must not panic
        // or block (they early-return before taking the lock or touching I/O).
        let c = global();
        // Don't enable() — exercise the disabled path only, so this test never
        // spawns the render thread or writes to a shared global's stderr.
        c.on_turn_start();
        // Disabled → on_text returns the delta untouched (no swallow, no I/O).
        assert_eq!(c.on_text("\nhello"), "\nhello");
        c.on_tool_start("grep_search");
        c.on_tool_end();
        c.on_prompt();
        c.on_turn_end();
        // Disabled → print_note writes nothing (non-REPL surfaces unchanged).
        c.print_note("cold-start note");
    }
}
