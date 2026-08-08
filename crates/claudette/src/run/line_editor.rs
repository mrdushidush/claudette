//! Line editing for the REPL prompt.
//!
//! Before this, `run/repl.rs` read input with a bare `stdin().read_line`,
//! so pressing Up in the first thirty seconds of using Claudette printed
//! `^[[A` — the single loudest "this is unpolished" signal in the product,
//! and invisible in the docs.
//!
//! Built on `crossterm`, which is already an unconditional dependency (the
//! TUI uses it), rather than pulling in `rustyline`/`reedline`: a new
//! dependency here would have to clear `deny.toml`'s license allow-list and
//! duplicate-version bans for a feature this size.
//!
//! Design: [`LineState`] is a pure state machine over ([`EditAction`],
//! buffer, cursor) with no terminal in sight, so every editing rule is unit
//! testable. The terminal loop in [`LineEditor::read_line`] does nothing but
//! translate key events into actions and redraw.
//!
//! **Non-TTY input falls back to plain `read_line`.** This is load-bearing,
//! not a nicety: the eval battery runs `claudette "<prompt>" < /dev/null`,
//! CI pipes input, and `--research` runs unattended. Raw mode on a
//! non-terminal would break all three.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, queue, style, terminal};

/// Max history entries kept in memory and on disk. Generous enough to cover
/// a long session, small enough that the file stays trivial to read and
/// rewrite wholesale.
const MAX_HISTORY: usize = 500;

/// Every slash command the REPL accepts, for Tab completion. Aliases are
/// included because completing `/cl` to `/clear` should work as readily as
/// completing `/cle`.
const SLASH_COMMANDS: &[&str] = &[
    "/brain",
    "/brownfield",
    "/capabilities",
    "/clear",
    "/compact",
    "/cost",
    "/diff",
    "/exit",
    "/forge",
    "/help",
    "/load",
    "/memory",
    "/mission_exit",
    "/model",
    "/models",
    "/preset",
    "/recall",
    "/reload",
    "/save",
    "/sessions",
    "/status",
    "/tools",
    "/undo",
];

/// One editing operation. Deliberately terminal-agnostic so the state
/// machine can be driven from tests without a tty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditAction {
    Insert(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    /// Delete the word before the cursor (Ctrl+W).
    DeleteWordBack,
    /// Discard everything before the cursor (Ctrl+U).
    KillToStart,
    /// Discard everything from the cursor on (Ctrl+K).
    KillToEnd,
    HistoryPrev,
    HistoryNext,
    /// Tab: complete a leading slash command.
    Complete,
    Submit,
    /// Ctrl+C: abandon this line, keep the REPL running.
    Cancel,
    /// Ctrl+D on an empty line: end of input.
    Eof,
    Noop,
}

/// What [`LineState::apply`] wants the caller to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditOutcome {
    /// Keep editing; redraw.
    Continue,
    /// The user pressed Enter — take the buffer.
    Submit,
    /// The user pressed Ctrl+C — drop the buffer, prompt again.
    Cancel,
    /// The user pressed Ctrl+D on an empty line — the REPL should exit.
    Eof,
}

/// Buffer + cursor + history cursor. `cursor` is a **character** index, not
/// a byte offset, so multi-byte input (Hebrew, emoji, CJK) can't split a
/// char or panic on a byte boundary.
#[derive(Debug, Default)]
pub(crate) struct LineState {
    pub(crate) buf: String,
    pub(crate) cursor: usize,
    /// Position in history while browsing: `None` = editing a fresh line.
    history_idx: Option<usize>,
    /// The in-progress line stashed when history browsing started, so
    /// Down past the newest entry restores what you were typing.
    stash: String,
}

impl LineState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn char_count(&self) -> usize {
        self.buf.chars().count()
    }

    /// Byte offset of character index `idx`. `idx == char_count` yields the
    /// buffer length, so this is safe to use as a splice point.
    fn byte_of(&self, idx: usize) -> usize {
        self.buf
            .char_indices()
            .nth(idx)
            .map_or(self.buf.len(), |(b, _)| b)
    }

    /// Apply one action. Returns what the caller should do next.
    pub(crate) fn apply(&mut self, action: EditAction, history: &[String]) -> EditOutcome {
        match action {
            EditAction::Insert(c) => {
                let at = self.byte_of(self.cursor);
                self.buf.insert(at, c);
                self.cursor += 1;
            }
            EditAction::Backspace => {
                if self.cursor > 0 {
                    let from = self.byte_of(self.cursor - 1);
                    let to = self.byte_of(self.cursor);
                    self.buf.replace_range(from..to, "");
                    self.cursor -= 1;
                }
            }
            EditAction::Delete => {
                if self.cursor < self.char_count() {
                    let from = self.byte_of(self.cursor);
                    let to = self.byte_of(self.cursor + 1);
                    self.buf.replace_range(from..to, "");
                }
            }
            EditAction::Left => self.cursor = self.cursor.saturating_sub(1),
            EditAction::Right => self.cursor = (self.cursor + 1).min(self.char_count()),
            EditAction::Home => self.cursor = 0,
            EditAction::End => self.cursor = self.char_count(),
            EditAction::DeleteWordBack => {
                // Skip trailing spaces, then the word itself — the readline
                // behaviour muscle memory expects.
                let chars: Vec<char> = self.buf.chars().collect();
                let mut start = self.cursor;
                while start > 0 && chars[start - 1].is_whitespace() {
                    start -= 1;
                }
                while start > 0 && !chars[start - 1].is_whitespace() {
                    start -= 1;
                }
                let from = self.byte_of(start);
                let to = self.byte_of(self.cursor);
                self.buf.replace_range(from..to, "");
                self.cursor = start;
            }
            EditAction::KillToStart => {
                let to = self.byte_of(self.cursor);
                self.buf.replace_range(0..to, "");
                self.cursor = 0;
            }
            EditAction::KillToEnd => {
                let from = self.byte_of(self.cursor);
                self.buf.truncate(from);
                self.cursor = self.char_count();
            }
            EditAction::HistoryPrev => self.browse_history(history, true),
            EditAction::HistoryNext => self.browse_history(history, false),
            EditAction::Complete => self.complete(),
            EditAction::Submit => return EditOutcome::Submit,
            EditAction::Cancel => return EditOutcome::Cancel,
            EditAction::Eof => {
                // Ctrl+D mid-line is a no-op, matching every shell: it only
                // means end-of-input on an empty line.
                if self.buf.is_empty() {
                    return EditOutcome::Eof;
                }
            }
            EditAction::Noop => {}
        }
        EditOutcome::Continue
    }

    /// Walk history. `back` moves toward older entries. `history` is
    /// oldest-first, so index 0 is the oldest.
    fn browse_history(&mut self, history: &[String], back: bool) {
        if history.is_empty() {
            return;
        }
        let next = match (self.history_idx, back) {
            // Entering history from a fresh line: stash it, land on newest.
            (None, true) => {
                self.stash = std::mem::take(&mut self.buf);
                Some(history.len() - 1)
            }
            (None, false) => return, // Already at the newest; nothing below.
            (Some(0), true) => Some(0), // Clamp at the oldest.
            (Some(i), true) => Some(i - 1),
            (Some(i), false) => {
                if i + 1 >= history.len() {
                    // Past the newest: restore the line we were typing.
                    self.buf = std::mem::take(&mut self.stash);
                    self.cursor = self.char_count();
                    self.history_idx = None;
                    return;
                }
                Some(i + 1)
            }
        };
        if let Some(i) = next {
            if let Some(entry) = history.get(i) {
                self.buf.clone_from(entry);
                self.cursor = self.char_count();
                self.history_idx = Some(i);
            }
        }
    }

    /// Tab-complete a leading slash command. Only fires when the buffer is
    /// a single `/`-prefixed token and the cursor is at its end — completing
    /// mid-argument would be guesswork.
    fn complete(&mut self) {
        if !self.buf.starts_with('/')
            || self.buf.contains(char::is_whitespace)
            || self.cursor != self.char_count()
        {
            return;
        }
        let matches: Vec<&&str> = SLASH_COMMANDS
            .iter()
            .filter(|c| c.starts_with(&self.buf))
            .collect();
        let completed = match matches.as_slice() {
            [only] => (**only).to_string(),
            // Several candidates: fill in the longest shared prefix, which
            // is what makes repeated Tab feel like progress rather than a
            // dead key.
            many if many.len() > 1 => {
                let first = *many[0];
                let mut end = first.len();
                for m in many {
                    end = end.min(
                        first
                            .char_indices()
                            .zip(m.chars())
                            .take_while(|((_, a), b)| a == b)
                            .last()
                            .map_or(0, |((i, a), _)| i + a.len_utf8()),
                    );
                }
                first[..end].to_string()
            }
            _ => return,
        };
        if completed.len() > self.buf.len() {
            self.buf = completed;
            self.cursor = self.char_count();
        }
    }
}

/// Map a crossterm key event to an [`EditAction`].
pub(crate) fn action_for(key: KeyEvent) -> EditAction {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match (key.code, ctrl) {
        (KeyCode::Enter, _) => EditAction::Submit,
        (KeyCode::Backspace, _) => EditAction::Backspace,
        (KeyCode::Delete, _) => EditAction::Delete,
        (KeyCode::Left, false) => EditAction::Left,
        (KeyCode::Right, false) => EditAction::Right,
        (KeyCode::Up, _) => EditAction::HistoryPrev,
        (KeyCode::Down, _) => EditAction::HistoryNext,
        (KeyCode::Home, _) => EditAction::Home,
        (KeyCode::End, _) => EditAction::End,
        (KeyCode::Tab, _) => EditAction::Complete,
        (KeyCode::Char('a'), true) => EditAction::Home,
        (KeyCode::Char('e'), true) => EditAction::End,
        (KeyCode::Char('b'), true) => EditAction::Left,
        (KeyCode::Char('f'), true) => EditAction::Right,
        (KeyCode::Char('w'), true) => EditAction::DeleteWordBack,
        (KeyCode::Char('u'), true) => EditAction::KillToStart,
        (KeyCode::Char('k'), true) => EditAction::KillToEnd,
        (KeyCode::Char('p'), true) => EditAction::HistoryPrev,
        (KeyCode::Char('n'), true) => EditAction::HistoryNext,
        (KeyCode::Char('c'), true) => EditAction::Cancel,
        (KeyCode::Char('d'), true) => EditAction::Eof,
        // Ctrl+<other> must not type a literal character.
        (KeyCode::Char(_), true) => EditAction::Noop,
        (KeyCode::Char(c), false) => EditAction::Insert(c),
        _ => EditAction::Noop,
    }
}

/// Sentinels that let *piped* input carry a multi-line prompt as one turn.
///
/// The piped branch of [`LineEditor::read_line`] is one `read_line` per turn,
/// which is right for a human at a keyboard and wrong for any program feeding
/// Claudette a prompt containing newlines: line two becomes turn two, a blank
/// line inside the prompt is skipped by the REPL loop, and a line reading
/// `exit` ends the session. There was no other way in — `Event::Paste` strips
/// newlines and only fires under raw mode, one-shot carries newlines on argv
/// but has no permission prompter, and no slash command reads a file into a
/// turn.
///
/// So: a piped line that is exactly [`MULTILINE_OPEN`] opens a block, and every
/// line up to one that is exactly [`MULTILINE_CLOSE`] is joined with `\n` and
/// returned as a single [`ReadOutcome::Line`]. Interactive input never sees
/// this — raw mode reads key events, not lines.
///
/// Hyphenated deliberately: `main.rs`'s doc-drift guard scans `src/` for
/// `CLAUDETTE_` followed by capitals, and an underscored sentinel reads to it
/// as an undocumented env var. It is not one, and should not look like one.
const MULTILINE_OPEN: &str = "<<<CLAUDETTE-PROMPT";
const MULTILINE_CLOSE: &str = "CLAUDETTE-PROMPT>>>";

/// What one `read_line` call produced.
#[derive(Debug)]
pub(crate) enum ReadOutcome {
    Line(String),
    /// Ctrl+C: prompt again without running a turn.
    Interrupted,
    /// Ctrl+D on an empty line, or EOF on piped input.
    Eof,
}

/// One turn's worth of piped input: a plain line, or a sentinel-delimited
/// multi-line block joined with `\n`.
///
/// Line endings are normalised to `\n`. The block is prompt *content*, often a
/// code body the model is asked to reproduce, and a CRLF that survives into it
/// corrupts the answer without ever looking wrong.
///
/// An unterminated block is an error, never a partial prompt. A truncated pipe
/// has to be loud here, because half a prompt still produces a fluent answer.
fn read_piped_turn(input: &mut impl io::BufRead) -> io::Result<ReadOutcome> {
    let mut first = String::new();
    if input.read_line(&mut first)? == 0 {
        return Ok(ReadOutcome::Eof);
    }
    if strip_eol(&first) != MULTILINE_OPEN {
        return Ok(ReadOutcome::Line(first));
    }

    let mut block = String::new();
    let mut empty = true;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "multi-line prompt opened with `{MULTILINE_OPEN}` reached end \
                     of input before its closing `{MULTILINE_CLOSE}`"
                ),
            ));
        }
        let body = strip_eol(&line);
        if body == MULTILINE_CLOSE {
            return Ok(ReadOutcome::Line(block));
        }
        // Tracked separately from `block.is_empty()` so a prompt that opens
        // with a blank line keeps it.
        if !empty {
            block.push('\n');
        }
        empty = false;
        block.push_str(body);
    }
}

/// Strip one trailing line terminator, CRLF or LF, and nothing else.
fn strip_eol(line: &str) -> &str {
    line.strip_suffix('\n')
        .map_or(line, |l| l.strip_suffix('\r').unwrap_or(l))
}

/// Owns the history list and its backing file.
pub(crate) struct LineEditor {
    history: Vec<String>,
    path: Option<PathBuf>,
    /// False when stdin or stderr is not a terminal — the whole editor
    /// degrades to `read_line` in that case.
    interactive: bool,
}

impl LineEditor {
    pub(crate) fn new() -> Self {
        let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
        let path = crate::env_config::home_dir()
            .join(".claudette")
            .join("repl_history");
        let history = load_history(&path);
        Self {
            history,
            path: Some(path),
            interactive,
        }
    }

    /// Record a submitted line. Consecutive duplicates are collapsed so
    /// holding Up doesn't walk through the same command repeatedly.
    pub(crate) fn push_history(&mut self, line: &str) {
        let line = line.trim();
        // A multi-line block (piped, sentinel-delimited) is deliberately not
        // recalled: Up restores into a single-line buffer, and the history file
        // is one entry per line, so storing it would corrupt both.
        if line.is_empty()
            || line.contains('\n')
            || self.history.last().map(String::as_str) == Some(line)
        {
            return;
        }
        self.history.push(line.to_string());
        if self.history.len() > MAX_HISTORY {
            let excess = self.history.len() - MAX_HISTORY;
            self.history.drain(0..excess);
        }
        if let Some(p) = &self.path {
            save_history(p, &self.history);
        }
    }

    /// Read one line, rendering `prompt` (whose *visible* width is
    /// `prompt_width` — the string itself carries ANSI colour codes that
    /// occupy no columns).
    pub(crate) fn read_line(&mut self, prompt: &str, prompt_width: u16) -> io::Result<ReadOutcome> {
        if !self.interactive {
            let stdin = io::stdin();
            return read_piped_turn(&mut stdin.lock());
        }
        self.read_line_raw(prompt, prompt_width)
    }

    fn read_line_raw(&mut self, prompt: &str, prompt_width: u16) -> io::Result<ReadOutcome> {
        let mut err = io::stderr();
        write!(err, "{prompt}")?;
        err.flush()?;

        terminal::enable_raw_mode()?;
        // Raw mode must come off on every path, including the `?` early
        // returns below — otherwise a transient terminal error leaves the
        // user's shell in raw mode with no echo.
        let _restore = scopeguard::guard((), |()| {
            let _ = terminal::disable_raw_mode();
        });

        let mut state = LineState::new();
        let mut prev_rows: u16 = 0;

        loop {
            let key = match event::read()? {
                // `Release`/`Repeat` arrive on Windows; acting on both press
                // and release would double every keystroke.
                Event::Key(k) if k.kind == KeyEventKind::Press => k,
                Event::Paste(text) => {
                    for c in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                        state.apply(EditAction::Insert(c), &self.history);
                    }
                    prev_rows = redraw(&mut err, prompt, prompt_width, &state, prev_rows)?;
                    continue;
                }
                Event::Resize(_, _) => {
                    prev_rows = redraw(&mut err, prompt, prompt_width, &state, prev_rows)?;
                    continue;
                }
                _ => continue,
            };

            match state.apply(action_for(key), &self.history) {
                EditOutcome::Continue => {
                    prev_rows = redraw(&mut err, prompt, prompt_width, &state, prev_rows)?;
                }
                EditOutcome::Submit => {
                    writeln!(err)?;
                    err.flush()?;
                    return Ok(ReadOutcome::Line(state.buf));
                }
                EditOutcome::Cancel => {
                    writeln!(err)?;
                    err.flush()?;
                    return Ok(ReadOutcome::Interrupted);
                }
                EditOutcome::Eof => {
                    writeln!(err)?;
                    err.flush()?;
                    return Ok(ReadOutcome::Eof);
                }
            }
        }
    }
}

/// Repaint the prompt line(s) and place the cursor. Returns how many rows
/// the line now occupies, which the next redraw needs in order to climb back
/// to the start of a wrapped line before clearing.
fn redraw(
    err: &mut io::Stderr,
    prompt: &str,
    prompt_width: u16,
    state: &LineState,
    prev_rows: u16,
) -> io::Result<u16> {
    let cols = terminal::size().map_or(80, |(c, _)| c).max(1);
    let total = prompt_width as usize + state.buf.chars().count();
    let rows = u16::try_from(total / cols as usize).unwrap_or(0);

    if prev_rows > 0 {
        queue!(err, cursor::MoveUp(prev_rows))?;
    }
    queue!(
        err,
        cursor::MoveToColumn(0),
        terminal::Clear(terminal::ClearType::FromCursorDown),
        style::Print(prompt),
        style::Print(&state.buf),
    )?;

    // Park the cursor where the caret belongs. Done as an absolute move from
    // the start of the input area so wrapped lines land correctly.
    let caret = prompt_width as usize + state.cursor;
    let caret_row = u16::try_from(caret / cols as usize).unwrap_or(0);
    let caret_col = u16::try_from(caret % cols as usize).unwrap_or(0);
    if rows > caret_row {
        queue!(err, cursor::MoveUp(rows - caret_row))?;
    }
    queue!(err, cursor::MoveToColumn(caret_col))?;
    err.flush()?;
    Ok(caret_row)
}

fn load_history(path: &std::path::Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToString::to_string)
        .collect();
    if out.len() > MAX_HISTORY {
        let excess = out.len() - MAX_HISTORY;
        out.drain(0..excess);
    }
    out
}

/// Best-effort persist. A history file we can't write is not worth
/// interrupting the session over — and it may legitimately be read-only on
/// a locked-down box.
fn save_history(path: &std::path::Path, history: &[String]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, history.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    fn state_with(buf: &str, cursor: usize) -> LineState {
        LineState {
            buf: buf.to_string(),
            cursor,
            ..LineState::default()
        }
    }

    fn apply_all(s: &mut LineState, actions: &[EditAction], history: &[String]) {
        for a in actions {
            s.apply(a.clone(), history);
        }
    }

    #[test]
    fn typing_inserts_at_the_cursor() {
        let mut s = LineState::new();
        apply_all(
            &mut s,
            &[
                EditAction::Insert('a'),
                EditAction::Insert('c'),
                EditAction::Left,
                EditAction::Insert('b'),
            ],
            &[],
        );
        assert_eq!(s.buf, "abc");
        assert_eq!(s.cursor, 2);
    }

    #[test]
    fn editing_is_char_based_not_byte_based() {
        // A byte-indexed cursor would split these and panic. Hebrew is a
        // first-class case here: the system prompt answers in it.
        let mut s = state_with("שלום", 4);
        s.apply(EditAction::Backspace, &[]);
        assert_eq!(s.buf, "שלו");
        s.apply(EditAction::Home, &[]);
        s.apply(EditAction::Delete, &[]);
        assert_eq!(s.buf, "לו");

        let mut e = state_with("a🎉b", 3);
        e.apply(EditAction::Backspace, &[]);
        assert_eq!(e.buf, "a🎉");
    }

    #[test]
    fn backspace_and_delete_at_the_edges_are_noops() {
        let mut s = state_with("ab", 0);
        assert_eq!(s.apply(EditAction::Backspace, &[]), EditOutcome::Continue);
        assert_eq!(s.buf, "ab");
        let mut e = state_with("ab", 2);
        e.apply(EditAction::Delete, &[]);
        assert_eq!(e.buf, "ab");
    }

    #[test]
    fn ctrl_w_deletes_the_previous_word_including_trailing_space() {
        let mut s = state_with("git commit now ", 15);
        s.apply(EditAction::DeleteWordBack, &[]);
        assert_eq!(s.buf, "git commit ");
        s.apply(EditAction::DeleteWordBack, &[]);
        assert_eq!(s.buf, "git ");
    }

    #[test]
    fn kill_to_start_and_end_respect_the_cursor() {
        let mut s = state_with("hello world", 6);
        s.apply(EditAction::KillToStart, &[]);
        assert_eq!(s.buf, "world");
        assert_eq!(s.cursor, 0);

        let mut e = state_with("hello world", 5);
        e.apply(EditAction::KillToEnd, &[]);
        assert_eq!(e.buf, "hello");
        assert_eq!(e.cursor, 5);
    }

    #[test]
    fn up_walks_back_through_history_and_down_returns_the_draft() {
        // The behaviour the raw read_line couldn't do at all: this is the
        // regression guard for "press Up, get ^[[A".
        let h = hist(&["first", "second"]);
        let mut s = LineState::new();
        apply_all(
            &mut s,
            &[EditAction::Insert('d'), EditAction::Insert('r')],
            &h,
        );

        s.apply(EditAction::HistoryPrev, &h);
        assert_eq!(s.buf, "second", "Up lands on the newest entry");
        s.apply(EditAction::HistoryPrev, &h);
        assert_eq!(s.buf, "first");
        s.apply(EditAction::HistoryPrev, &h);
        assert_eq!(s.buf, "first", "clamps at the oldest");

        s.apply(EditAction::HistoryNext, &h);
        assert_eq!(s.buf, "second");
        s.apply(EditAction::HistoryNext, &h);
        assert_eq!(s.buf, "dr", "past the newest, the draft comes back");
        assert_eq!(s.cursor, 2);
    }

    #[test]
    fn history_navigation_on_empty_history_is_a_noop() {
        let mut s = state_with("typing", 6);
        s.apply(EditAction::HistoryPrev, &[]);
        assert_eq!(s.buf, "typing");
    }

    #[test]
    fn tab_completes_a_unique_slash_command() {
        let mut s = state_with("/brow", 5);
        s.apply(EditAction::Complete, &[]);
        assert_eq!(s.buf, "/brownfield");
        assert_eq!(s.cursor, 11);
    }

    #[test]
    fn tab_fills_the_shared_prefix_when_several_commands_match() {
        // /model and /models both match — completing to the shared prefix
        // is progress; completing to either would be a wrong guess.
        let mut s = state_with("/mod", 4);
        s.apply(EditAction::Complete, &[]);
        assert_eq!(s.buf, "/model");
    }

    #[test]
    fn tab_does_not_complete_plain_prose_or_mid_argument() {
        let mut s = state_with("what is", 7);
        s.apply(EditAction::Complete, &[]);
        assert_eq!(s.buf, "what is", "prose must not gain a slash command");

        let mut e = state_with("/save my-", 9);
        e.apply(EditAction::Complete, &[]);
        assert_eq!(e.buf, "/save my-", "arguments are not command names");
    }

    #[test]
    fn ctrl_d_only_ends_input_on_an_empty_line() {
        let mut s = state_with("half typed", 10);
        assert_eq!(s.apply(EditAction::Eof, &[]), EditOutcome::Continue);
        assert_eq!(s.buf, "half typed", "Ctrl+D mid-line must not discard it");

        let mut e = LineState::new();
        assert_eq!(e.apply(EditAction::Eof, &[]), EditOutcome::Eof);
    }

    #[test]
    fn ctrl_c_cancels_and_enter_submits() {
        let mut s = state_with("oops", 4);
        assert_eq!(s.apply(EditAction::Cancel, &[]), EditOutcome::Cancel);
        let mut e = state_with("go", 2);
        assert_eq!(e.apply(EditAction::Submit, &[]), EditOutcome::Submit);
    }

    #[test]
    fn control_chords_never_type_a_literal_character() {
        // Ctrl+Z / Ctrl+L etc. must not end up in the buffer as text.
        for c in ['z', 'l', 'q', 'r'] {
            let action = action_for(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
            assert_eq!(action, EditAction::Noop, "ctrl+{c} should be inert");
        }
        assert_eq!(
            action_for(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            EditAction::Insert('a'),
        );
    }

    #[test]
    fn key_bindings_map_to_the_expected_actions() {
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(action_for(ctrl('a')), EditAction::Home);
        assert_eq!(action_for(ctrl('e')), EditAction::End);
        assert_eq!(action_for(ctrl('w')), EditAction::DeleteWordBack);
        assert_eq!(action_for(ctrl('u')), EditAction::KillToStart);
        assert_eq!(action_for(ctrl('k')), EditAction::KillToEnd);
        assert_eq!(action_for(ctrl('c')), EditAction::Cancel);
        assert_eq!(action_for(ctrl('d')), EditAction::Eof);
        assert_eq!(
            action_for(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            EditAction::HistoryPrev,
        );
    }

    #[test]
    fn history_file_round_trips_and_is_capped() {
        let dir = std::env::temp_dir().join(format!("claudette-hist-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("repl_history");

        let many: Vec<String> = (0..MAX_HISTORY + 50).map(|i| format!("cmd {i}")).collect();
        save_history(&path, &many);
        let loaded = load_history(&path);
        assert_eq!(loaded.len(), MAX_HISTORY, "oldest entries are dropped");
        assert_eq!(
            loaded.last().map(String::as_str),
            Some(format!("cmd {}", MAX_HISTORY + 49).as_str()),
            "the newest entry survives",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_history_file_loads_empty_rather_than_failing() {
        let path = std::env::temp_dir().join("claudette-nonexistent-history-file");
        let _ = std::fs::remove_file(&path);
        assert!(load_history(&path).is_empty());
    }

    // --- piped input: the multi-line prompt path ---

    fn piped(input: &str) -> io::Result<ReadOutcome> {
        read_piped_turn(&mut input.as_bytes())
    }

    fn line_of(outcome: ReadOutcome) -> String {
        match outcome {
            ReadOutcome::Line(l) => l,
            ReadOutcome::Interrupted => panic!("expected a line, got Interrupted"),
            ReadOutcome::Eof => panic!("expected a line, got Eof"),
        }
    }

    #[test]
    fn a_plain_piped_line_is_unchanged_by_the_sentinel_path() {
        // Including its trailing newline: the REPL trims, and the old
        // behaviour is what every existing piped caller was built against.
        assert_eq!(line_of(piped("write a haiku\n").unwrap()), "write a haiku\n");
    }

    #[test]
    fn empty_input_is_eof() {
        assert!(matches!(piped("").unwrap(), ReadOutcome::Eof));
    }

    #[test]
    fn a_sentinel_block_becomes_one_line_joined_with_newlines() {
        let got = line_of(
            piped(&format!(
                "{MULTILINE_OPEN}\nfirst\nsecond\nthird\n{MULTILINE_CLOSE}\n"
            ))
            .unwrap(),
        );
        assert_eq!(got, "first\nsecond\nthird");
    }

    #[test]
    fn crlf_in_a_block_is_normalised_to_lf() {
        let got = line_of(
            piped(&format!(
                "{MULTILINE_OPEN}\r\nfirst\r\nsecond\r\n{MULTILINE_CLOSE}\r\n"
            ))
            .unwrap(),
        );
        assert_eq!(got, "first\nsecond", "no CR survives into prompt content");
    }

    #[test]
    fn blank_lines_inside_a_block_are_content_including_a_leading_one() {
        let got = line_of(
            piped(&format!(
                "{MULTILINE_OPEN}\n\nfirst\n\nsecond\n{MULTILINE_CLOSE}\n"
            ))
            .unwrap(),
        );
        // The REPL loop skips a blank *line*; inside a block it is a blank
        // line of the prompt, which is the whole point.
        assert_eq!(got, "\nfirst\n\nsecond");
    }

    #[test]
    fn exit_inside_a_block_is_content_not_a_command() {
        let got =
            line_of(piped(&format!("{MULTILINE_OPEN}\nexit\nquit\n{MULTILINE_CLOSE}\n")).unwrap());
        assert_eq!(got, "exit\nquit");
    }

    #[test]
    fn the_reader_resumes_after_the_closing_sentinel() {
        let input = format!("{MULTILINE_OPEN}\nblock\n{MULTILINE_CLOSE}\nnext turn\n");
        let mut bytes = input.as_bytes();
        assert_eq!(line_of(read_piped_turn(&mut bytes).unwrap()), "block");
        assert_eq!(
            line_of(read_piped_turn(&mut bytes).unwrap()),
            "next turn\n",
            "the turn after a block reads normally",
        );
    }

    #[test]
    fn an_unterminated_block_errors_rather_than_delivering_half_a_prompt() {
        let err = piped(&format!("{MULTILINE_OPEN}\nfirst\nsecond\n")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        assert!(
            err.to_string().contains(MULTILINE_CLOSE),
            "the error names the terminator that was missing: {err}",
        );
    }

    #[test]
    fn the_opener_must_match_exactly() {
        // Trailing text, leading space, or a near-miss is an ordinary prompt.
        for near in [
            format!("{MULTILINE_OPEN} \n"),
            format!(" {MULTILINE_OPEN}\n"),
            format!("{MULTILINE_OPEN}x\n"),
        ] {
            let got = line_of(piped(&near).unwrap());
            assert_eq!(got, near, "near-miss opener stays a plain line");
        }
    }

    #[test]
    fn history_refuses_a_multi_line_entry() {
        let mut ed = LineEditor {
            history: Vec::new(),
            path: None,
            interactive: false,
        };
        ed.push_history("first\nsecond");
        assert!(ed.history.is_empty(), "a block would corrupt the history file");
        ed.push_history("plain");
        assert_eq!(ed.history, hist(&["plain"]));
    }
}
