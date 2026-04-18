//! Ratatui TUI entry point for Claudette.
//!
//! `run_tui(session)` sets up the terminal, spawns the worker thread, and
//! drives the 50ms render + input loop.
//!
//! Sprint A — skeleton: Chat tab, streaming tokens, tool sidebar.
//! Sprint B — Chat polish: scroll, inline tools, progress bar, blink cursor,
//!   `/clear` + `/compact`.
//! Sprint C — Tools tab: full tool event log, tab switching via `1`-`5`.
//! Sprint D — Notes tab: browse `~/.claudette/notes/`, Up/Down selection,
//!   note body viewer, `f` to filter by tag, 2-second cache TTL.
//!
//! Sprint E adds the Todos tab. Sprint F adds HW.
//! Sprint G adds the `TuiPrompter` for `DangerFullAccess` confirmation modals.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use crate::Session;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};

use crate::tui_events::{TuiEvent, UserInput};
use crate::tui_worker;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const TAB_CHAT: u8 = 0;
const TAB_TOOLS: u8 = 1;
const TAB_NOTES: u8 = 2;
const TAB_TODOS: u8 = 3;
const TAB_HW: u8 = 4;
const TAB_COUNT: u8 = 5;

/// Lines per `PageUp` / `PageDown` press.
const PAGE_LINES: u16 = 8;
/// Cursor blink interval.
const BLINK_MS: u64 = 400;
/// How often to re-scan the notes directory.
const NOTES_CACHE_SECS: u64 = 2;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// A simplified tool entry for the Chat inline display.
enum ToolEntry {
    Start {
        name: String,
    },
    Done {
        name: String,
        ok: bool,
        elapsed_ms: u64,
    },
}

/// A rich tool record for the Tools tab log.
struct ToolRecord {
    name: String,
    input_preview: String,
    result_preview: Option<String>,
    ok: Option<bool>,
    elapsed_ms: Option<u64>,
}

/// Completed conversation turn.
struct Message {
    role: String,
    text: String,
}

/// A parsed note file from `~/.claudette/notes/`.
struct NoteEntry {
    title: String,
    created: String,
    tags: Vec<String>,
    body: String,
}

/// A parsed todo from `~/.claudette/todos.json`.
struct TodoItem {
    id: String,
    text: String,
    done: bool,
    created_at: String,
    completed_at: Option<String>,
}

/// A loaded Ollama model from `/api/ps`.
struct OllamaModel {
    name: String,
    size_vram: u64,
    size_disk: u64,
    gpu_percent: u8,
}

/// State for the HW tab.
struct HwState {
    models: Vec<OllamaModel>,
    ollama_online: bool,
    ollama_url: String,
    total_vram_gb: f64,
    last_refresh: Instant,
    needs_refresh: bool,
    last_error: Option<String>,
}

impl Default for HwState {
    fn default() -> Self {
        let total_vram_gb = std::env::var("CLAUDETTE_VRAM_GB")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(8.0);
        Self {
            models: Vec::new(),
            ollama_online: false,
            ollama_url: hw_ollama_url(),
            total_vram_gb,
            last_refresh: Instant::now(),
            needs_refresh: true,
            last_error: None,
        }
    }
}

/// State for the Todos tab.
struct TodosState {
    items: Vec<TodoItem>,
    selected: usize,
    last_refresh: Instant,
    needs_refresh: bool,
}

impl Default for TodosState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
            last_refresh: Instant::now(),
            needs_refresh: true,
        }
    }
}

/// State for the Notes tab.
struct NotesState {
    entries: Vec<NoteEntry>,
    selected: usize,
    last_refresh: Instant,
    tag_filter: Option<String>,
    /// True while the user is typing a tag filter.
    filter_editing: bool,
    filter_input: String,
    /// Scroll offset for the note body pane.
    body_scroll: u16,
    /// True when the cache needs an immediate refresh.
    needs_refresh: bool,
}

impl Default for NotesState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            last_refresh: Instant::now(),
            tag_filter: None,
            filter_editing: false,
            filter_input: String::new(),
            body_scroll: 0,
            needs_refresh: true,
        }
    }
}

/// All mutable TUI state — owned entirely by the render loop thread.
struct App {
    // ── Conversation ──────────────────────────────────────────────────────
    history: Vec<Message>,
    streaming_text: String,
    current_turn_tools: Vec<ToolEntry>,
    input: String,
    working: bool,

    // ── Tool log ──────────────────────────────────────────────────────────
    all_tool_records: Vec<ToolRecord>,

    // ── Notes ─────────────────────────────────────────────────────────────
    notes: NotesState,

    // ── Todos ─────────────────────────────────────────────────────────────
    todos: TodosState,

    // ── Hardware ──────────────────────────────────────────────────────────
    hw: HwState,

    // ── Tab & scroll ──────────────────────────────────────────────────────
    active_tab: u8,
    chat_scroll: u16,
    tools_scroll: u16,

    // ── Status ────────────────────────────────────────────────────────────
    cursor_phase: bool,
    last_blink: Instant,
    estimated_tokens: usize,
    threshold: usize,
}

impl Default for App {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            streaming_text: String::new(),
            current_turn_tools: Vec::new(),
            input: String::new(),
            working: false,
            all_tool_records: Vec::new(),
            notes: NotesState::default(),
            todos: TodosState::default(),
            hw: HwState::default(),
            active_tab: TAB_CHAT,
            chat_scroll: 0,
            tools_scroll: 0,
            cursor_phase: true,
            last_blink: Instant::now(),
            estimated_tokens: 0,
            threshold: crate::run::compact_threshold(),
        }
    }
}

impl App {
    fn handle_tui_event(&mut self, event: TuiEvent) {
        match event {
            TuiEvent::Token(delta) => self.streaming_text.push_str(&delta),

            TuiEvent::TurnComplete { text, .. } => {
                self.history.push(Message {
                    role: "Claudette".to_string(),
                    text,
                });
                self.streaming_text.clear();
                self.current_turn_tools.clear();
            }

            TuiEvent::Working(w) => {
                self.working = w;
                if w {
                    self.streaming_text.clear();
                    self.current_turn_tools.clear();
                }
            }

            TuiEvent::TurnError(e) => {
                self.history.push(Message {
                    role: "Error".to_string(),
                    text: e,
                });
                self.streaming_text.clear();
                self.current_turn_tools.clear();
                self.working = false;
            }

            TuiEvent::TokensUpdate {
                estimated,
                threshold,
            } => {
                self.estimated_tokens = estimated;
                self.threshold = threshold;
            }

            TuiEvent::ToolCallStart {
                name,
                input_preview,
            } => {
                self.current_turn_tools
                    .push(ToolEntry::Start { name: name.clone() });
                self.all_tool_records.push(ToolRecord {
                    name,
                    input_preview,
                    result_preview: None,
                    ok: None,
                    elapsed_ms: None,
                });
            }

            TuiEvent::ToolCallDone {
                name,
                result_preview,
                ok,
                elapsed_ms,
            } => {
                if let Some(last) = self.current_turn_tools.last_mut() {
                    if matches!(last, ToolEntry::Start { name: n } if n == &name) {
                        *last = ToolEntry::Done {
                            name: name.clone(),
                            ok,
                            elapsed_ms,
                        };
                    }
                }
                if let Some(rec) = self
                    .all_tool_records
                    .iter_mut()
                    .rev()
                    .find(|r| r.name == name && r.result_preview.is_none())
                {
                    rec.result_preview = Some(result_preview);
                    rec.ok = Some(ok);
                    rec.elapsed_ms = Some(elapsed_ms);
                }
            }

            TuiEvent::Compacted { removed } => {
                self.history.push(Message {
                    role: "System".to_string(),
                    text: format!("Auto-compacted {removed} older message(s)."),
                });
            }

            TuiEvent::SessionReset => {
                self.history.clear();
                self.streaming_text.clear();
                self.current_turn_tools.clear();
                self.all_tool_records.clear();
                self.chat_scroll = 0;
                self.estimated_tokens = 0;
                self.history.push(Message {
                    role: "System".to_string(),
                    text: "Session cleared.".to_string(),
                });
            }

            TuiEvent::Saved => {}
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Notes — filesystem helpers
// ─────────────────────────────────────────────────────────────────────────────

fn notes_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claudette").join("notes")
}

fn scan_notes(tag_filter: Option<&str>) -> Vec<NoteEntry> {
    let dir = notes_dir();
    let mut entries = Vec::new();
    let Ok(read_dir) = std::fs::read_dir(&dir) else {
        return entries;
    };
    for item in read_dir.flatten() {
        let path = item.path();
        if path.extension().is_some_and(|e| e == "md") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                let note = parse_note(&content);
                if let Some(filter) = tag_filter {
                    if !note.tags.iter().any(|t| t.eq_ignore_ascii_case(filter)) {
                        continue;
                    }
                }
                entries.push(note);
            }
        }
    }
    // Newest first (created field is ISO timestamp).
    entries.sort_by(|a, b| b.created.cmp(&a.created));
    entries
}

fn parse_note(content: &str) -> NoteEntry {
    let mut title = String::new();
    let mut created = String::new();
    let mut tags = Vec::new();
    let mut body = String::new();
    let mut past_header = false;

    for line in content.lines() {
        if !past_header {
            if let Some(h) = line.strip_prefix("# ") {
                title = h.to_string();
            } else if let Some(rest) = line.strip_prefix("Created:") {
                created = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("Tags:") {
                tags = rest
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect();
            } else if line.trim().is_empty() && !title.is_empty() {
                past_header = true;
            }
        } else {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
        }
    }

    NoteEntry {
        title,
        created,
        tags,
        body,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Todos — filesystem helpers
// ─────────────────────────────────────────────────────────────────────────────

fn todos_path() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claudette").join("todos.json")
}

fn load_todos() -> Vec<TodoItem> {
    let Ok(content) = std::fs::read_to_string(todos_path()) else {
        return Vec::new();
    };
    let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&content) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| {
            Some(TodoItem {
                id: v.get("id")?.as_str()?.to_string(),
                text: v.get("text")?.as_str()?.to_string(),
                done: v.get("done")?.as_bool()?,
                created_at: v
                    .get("created_at")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string(),
                completed_at: v
                    .get("completed_at")
                    .and_then(|c| c.as_str())
                    .map(String::from),
            })
        })
        .collect()
}

fn save_todos(items: &[TodoItem]) {
    let arr: Vec<serde_json::Value> = items
        .iter()
        .map(|t| {
            let mut obj = serde_json::json!({
                "id": t.id,
                "text": t.text,
                "done": t.done,
                "created_at": t.created_at,
            });
            if let Some(ref completed) = t.completed_at {
                obj["completed_at"] = serde_json::Value::String(completed.clone());
            }
            obj
        })
        .collect();
    if let Ok(json) = serde_json::to_string_pretty(&arr) {
        let _ = std::fs::write(todos_path(), json);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Hardware — Ollama polling
// ─────────────────────────────────────────────────────────────────────────────

/// How often to poll Ollama `/api/ps`.
const HW_REFRESH_SECS: u64 = 10;

fn hw_ollama_url() -> String {
    match std::env::var("OLLAMA_HOST").ok().filter(|h| !h.is_empty()) {
        Some(h) => {
            let h = h.trim_end_matches('/');
            if h.starts_with("http://") || h.starts_with("https://") {
                h.to_string()
            } else {
                format!("http://{h}")
            }
        }
        None => "http://localhost:11434".to_string(),
    }
}

fn poll_ollama(url: &str) -> Result<Vec<OllamaModel>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(format!("{url}/api/ps"))
        .send()
        .map_err(|e| e.to_string())?;

    let body: serde_json::Value = resp.json().map_err(|e| e.to_string())?;

    let models = body
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let name = m.get("name")?.as_str()?.to_string();
                    let size_vram = m
                        .get("size_vram")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    let size_disk = m
                        .get("size_disk")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    let total = size_vram + size_disk;
                    let gpu_percent = if total > 0 {
                        ((size_vram as f64 / total as f64) * 100.0) as u8
                    } else {
                        100
                    };
                    Some(OllamaModel {
                        name,
                        size_vram,
                        size_disk,
                        gpu_percent,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    } else if bytes > 0 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        "0 B".to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

pub fn run_tui(session: Session) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    scopeguard::defer! {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    }

    let (tui_tx, tui_rx) = std::sync::mpsc::sync_channel::<TuiEvent>(512);
    let (user_tx, user_rx) = std::sync::mpsc::channel::<UserInput>();
    let _worker = tui_worker::spawn_worker(session, user_rx, tui_tx);

    run_loop(&mut terminal, &tui_rx, &user_tx)
}

// ─────────────────────────────────────────────────────────────────────────────
// Event loop
// ─────────────────────────────────────────────────────────────────────────────

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    tui_rx: &Receiver<TuiEvent>,
    user_tx: &Sender<UserInput>,
) -> Result<()> {
    let mut app = App::default();

    loop {
        // Drain worker events.
        loop {
            match tui_rx.try_recv() {
                Ok(ev) => app.handle_tui_event(ev),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }

        // Tick blink.
        let now = Instant::now();
        if now.duration_since(app.last_blink) >= Duration::from_millis(BLINK_MS) {
            app.cursor_phase = !app.cursor_phase;
            app.last_blink = now;
        }

        // Refresh notes cache when the Notes tab is active and stale.
        if app.active_tab == TAB_NOTES
            && (app.notes.needs_refresh
                || now.duration_since(app.notes.last_refresh)
                    >= Duration::from_secs(NOTES_CACHE_SECS))
        {
            app.notes.needs_refresh = false;
            app.notes.entries = scan_notes(app.notes.tag_filter.as_deref());
            app.notes.last_refresh = now;
            if app.notes.selected >= app.notes.entries.len() {
                app.notes.selected = app.notes.entries.len().saturating_sub(1);
            }
        }

        // Refresh todos cache when the Todos tab is active and stale.
        if app.active_tab == TAB_TODOS
            && (app.todos.needs_refresh
                || now.duration_since(app.todos.last_refresh)
                    >= Duration::from_secs(NOTES_CACHE_SECS))
        {
            app.todos.needs_refresh = false;
            app.todos.items = load_todos();
            app.todos.last_refresh = now;
            if app.todos.selected >= app.todos.items.len() {
                app.todos.selected = app.todos.items.len().saturating_sub(1);
            }
        }

        // Refresh HW tab (poll Ollama) every 10s when visible.
        if app.active_tab == TAB_HW
            && (app.hw.needs_refresh
                || now.duration_since(app.hw.last_refresh) >= Duration::from_secs(HW_REFRESH_SECS))
        {
            app.hw.needs_refresh = false;
            app.hw.last_refresh = now;
            match poll_ollama(&app.hw.ollama_url) {
                Ok(models) => {
                    app.hw.models = models;
                    app.hw.ollama_online = true;
                    app.hw.last_error = None;
                }
                Err(e) => {
                    app.hw.models.clear();
                    app.hw.ollama_online = false;
                    app.hw.last_error = Some(e);
                }
            }
        }

        terminal.draw(|f| render(f, &app))?;

        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        // Windows fires both Press and Release events — only handle Press.
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // ── Filter editing mode (Notes tab) consumes keys first ───────
        if app.notes.filter_editing {
            match (key.code, key.modifiers) {
                (KeyCode::Char('c' | 'd'), KeyModifiers::CONTROL) => {
                    let _ = user_tx.send(UserInput::Quit);
                    return Ok(());
                }
                (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                    app.notes.filter_input.push(c);
                }
                (KeyCode::Backspace, _) => {
                    app.notes.filter_input.pop();
                }
                (KeyCode::Enter, _) => {
                    let tag = std::mem::take(&mut app.notes.filter_input);
                    app.notes.tag_filter = if tag.is_empty() { None } else { Some(tag) };
                    app.notes.filter_editing = false;
                    // Force immediate rescan.
                    app.notes.needs_refresh = true;
                    app.notes.selected = 0;
                    app.notes.body_scroll = 0;
                }
                (KeyCode::Esc, _) => {
                    app.notes.filter_editing = false;
                    app.notes.filter_input.clear();
                    app.notes.tag_filter = None;
                    app.notes.needs_refresh = true;
                    app.notes.selected = 0;
                    app.notes.body_scroll = 0;
                }
                _ => {}
            }
            continue;
        }

        // ── Normal key handling ───────────────────────────────────────
        match (key.code, key.modifiers) {
            // Quit.
            (KeyCode::Char('c' | 'd'), KeyModifiers::CONTROL) => {
                let _ = user_tx.send(UserInput::Quit);
                return Ok(());
            }

            // Tab switching (works even while Claudette is thinking).
            (KeyCode::Char('1'), _) if app.input.is_empty() => app.active_tab = TAB_CHAT,
            (KeyCode::Char('2'), _) if app.input.is_empty() => app.active_tab = TAB_TOOLS,
            (KeyCode::Char('3'), _) if app.input.is_empty() => app.active_tab = TAB_NOTES,
            (KeyCode::Char('4'), _) if app.input.is_empty() => app.active_tab = TAB_TODOS,
            (KeyCode::Char('5'), _) if app.input.is_empty() => app.active_tab = TAB_HW,
            (KeyCode::Tab, KeyModifiers::NONE) => {
                app.active_tab = (app.active_tab + 1) % TAB_COUNT;
            }
            (KeyCode::BackTab, _) => {
                app.active_tab = app.active_tab.checked_sub(1).unwrap_or(TAB_COUNT - 1);
            }

            // ── Notes tab: Up/Down selection, f=filter ────────────────
            (KeyCode::Up, _) if app.active_tab == TAB_NOTES => {
                app.notes.selected = app.notes.selected.saturating_sub(1);
                app.notes.body_scroll = 0;
            }
            (KeyCode::Down, _) if app.active_tab == TAB_NOTES => {
                if app.notes.selected + 1 < app.notes.entries.len() {
                    app.notes.selected += 1;
                }
                app.notes.body_scroll = 0;
            }
            (KeyCode::Char('f'), _) if app.active_tab == TAB_NOTES && app.input.is_empty() => {
                app.notes.filter_editing = true;
                app.notes.filter_input.clear();
            }
            (KeyCode::Esc, _) if app.active_tab == TAB_NOTES => {
                if app.notes.tag_filter.is_some() {
                    app.notes.tag_filter = None;
                    app.notes.needs_refresh = true;
                    app.notes.selected = 0;
                    app.notes.body_scroll = 0;
                }
            }

            // ── Todos tab: Up/Down selection, Space/Enter toggle ──────
            (KeyCode::Up, _) if app.active_tab == TAB_TODOS => {
                app.todos.selected = app.todos.selected.saturating_sub(1);
            }
            (KeyCode::Down, _) if app.active_tab == TAB_TODOS => {
                if app.todos.selected + 1 < app.todos.items.len() {
                    app.todos.selected += 1;
                }
            }
            (KeyCode::Char(' '), _) if app.active_tab == TAB_TODOS && app.input.is_empty() => {
                if let Some(item) = app.todos.items.get_mut(app.todos.selected) {
                    item.done = !item.done;
                    item.completed_at = if item.done {
                        Some(chrono::Local::now().to_rfc3339())
                    } else {
                        None
                    };
                    save_todos(&app.todos.items);
                }
            }
            (KeyCode::Enter, _)
                if app.active_tab == TAB_TODOS && app.input.is_empty() && !app.working =>
            {
                if let Some(item) = app.todos.items.get_mut(app.todos.selected) {
                    item.done = !item.done;
                    item.completed_at = if item.done {
                        Some(chrono::Local::now().to_rfc3339())
                    } else {
                        None
                    };
                    save_todos(&app.todos.items);
                }
            }

            // Per-tab scroll.
            (KeyCode::PageUp, _) => match app.active_tab {
                TAB_CHAT => app.chat_scroll = app.chat_scroll.saturating_add(PAGE_LINES),
                TAB_TOOLS => app.tools_scroll = app.tools_scroll.saturating_add(PAGE_LINES),
                TAB_NOTES => {
                    app.notes.body_scroll = app.notes.body_scroll.saturating_add(PAGE_LINES);
                }
                _ => {}
            },
            (KeyCode::PageDown, _) => match app.active_tab {
                TAB_CHAT => app.chat_scroll = app.chat_scroll.saturating_sub(PAGE_LINES),
                TAB_TOOLS => app.tools_scroll = app.tools_scroll.saturating_sub(PAGE_LINES),
                TAB_NOTES => {
                    app.notes.body_scroll = app.notes.body_scroll.saturating_sub(PAGE_LINES);
                }
                _ => {}
            },
            (KeyCode::End, _) => match app.active_tab {
                TAB_CHAT => app.chat_scroll = 0,
                TAB_TOOLS => app.tools_scroll = 0,
                TAB_NOTES => app.notes.body_scroll = 0,
                _ => {}
            },

            // Submit input.
            (KeyCode::Enter, _) if !app.working => {
                if !app.input.is_empty() {
                    let text = std::mem::take(&mut app.input);
                    app.chat_scroll = 0;
                    if let Some(cmd) = text.strip_prefix('/') {
                        let _ = user_tx.send(UserInput::SlashCommand(cmd.to_string()));
                    } else {
                        app.history.push(Message {
                            role: "You".to_string(),
                            text: text.clone(),
                        });
                        app.active_tab = TAB_CHAT;
                        let _ = user_tx.send(UserInput::Message(text));
                    }
                }
            }

            // Typing.
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) if !app.working => {
                if !app.input.is_empty() || !c.is_ascii_digit() {
                    app.input.push(c);
                }
            }
            (KeyCode::Backspace, _) if !app.working => {
                app.input.pop();
            }

            _ => {}
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Top-level render
// ─────────────────────────────────────────────────────────────────────────────

fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title / tab bar
            Constraint::Min(3),    // active tab content
            Constraint::Length(1), // status bar (semantic + metrics)
            Constraint::Length(2), // input (top border + 1 row)
        ])
        .split(area);

    render_title(f, outer[0], app);

    match app.active_tab {
        TAB_CHAT => render_chat_tab(f, app, outer[1]),
        TAB_TOOLS => render_tools_tab(f, app, outer[1]),
        TAB_NOTES => render_notes_tab(f, app, outer[1]),
        TAB_TODOS => render_todos_tab(f, app, outer[1]),
        TAB_HW => render_hw_tab(f, app, outer[1]),
        n => render_placeholder(f, n, outer[1]),
    }

    render_status(f, app, outer[2]);
    render_input(f, app, outer[3]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Title / tab bar
// ─────────────────────────────────────────────────────────────────────────────

fn render_title(f: &mut Frame, area: Rect, app: &App) {
    let tab_names = ["Chat", "Tools", "Notes", "Todos", "HW"];
    // Active tab: inverse yellow block (black-on-yellow bold) — high-
    // contrast highlight that reads clearly on the darkgray bar.
    let active_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let inactive_style = Style::default().fg(Color::White).bg(Color::DarkGray);
    // Brand badge: inverse red block — visual anchor top-left.
    let brand_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Red)
        .add_modifier(Modifier::BOLD);

    let mut spans = vec![
        Span::styled(" CLAUDET ", brand_style),
        Span::styled(" ", Style::default().bg(Color::DarkGray)),
    ];
    for (i, name) in tab_names.iter().enumerate() {
        let style = if i as u8 == app.active_tab {
            active_style
        } else {
            inactive_style
        };
        spans.push(Span::styled(format!(" [{}]{name} ", i + 1), style));
    }

    if app.active_tab == TAB_CHAT && app.chat_scroll > 0 {
        spans.push(Span::styled(
            "  ↓End to return",
            Style::default().fg(Color::Yellow).bg(Color::DarkGray),
        ));
    }

    let title = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::DarkGray));
    f.render_widget(title, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Chat tab
// ─────────────────────────────────────────────────────────────────────────────

fn render_chat_tab(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(area);
    render_messages(f, app, cols[0]);
    render_tool_sidebar(f, app, cols[1]);
}

fn render_messages(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    for msg in &app.history {
        let (label_color, body_color) = match msg.role.as_str() {
            "You" => (Color::Cyan, Color::White),
            "Error" => (Color::Red, Color::LightRed),
            "System" => (Color::Yellow, Color::DarkGray),
            _ => (Color::Green, Color::White),
        };
        lines.push(Line::from(Span::styled(
            format!("{}:", msg.role),
            Style::default()
                .fg(label_color)
                .add_modifier(Modifier::BOLD),
        )));
        for line in msg.text.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(body_color),
            )));
        }
        lines.push(Line::from(Span::raw("")));
    }

    if !app.streaming_text.is_empty() || app.working {
        lines.push(Line::from(Span::styled(
            "Claudette:",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )));
        for entry in &app.current_turn_tools {
            lines.push(match entry {
                ToolEntry::Start { name } => Line::from(Span::styled(
                    format!("  · {name}…"),
                    Style::default().fg(Color::DarkGray),
                )),
                ToolEntry::Done {
                    name,
                    ok,
                    elapsed_ms,
                } => {
                    let icon = if *ok { "✓" } else { "✗" };
                    Line::from(Span::styled(
                        format!("  · {icon} {name} ({elapsed_ms}ms)"),
                        Style::default().fg(Color::DarkGray),
                    ))
                }
            });
        }
        for line in app.streaming_text.lines() {
            lines.push(Line::from(Span::raw(format!("  {line}"))));
        }
        if app.cursor_phase || !app.streaming_text.is_empty() {
            lines.push(Line::from(Span::styled(
                "  ▌",
                Style::default().fg(Color::Yellow),
            )));
        } else {
            lines.push(Line::from(Span::raw("   ")));
        }
    }

    // Border takes 1 row top + 1 row bottom; wrap width is inner width.
    let inner_width = area.width.saturating_sub(2);
    let visible = area.height.saturating_sub(2);
    // Count rows AFTER wrapping. Using `lines.len()` here would undercount
    // when a logical line wraps across multiple rows (Wrap { trim: false }),
    // causing the tail of a long response to be clipped until a later
    // message grew the logical-line count enough to compensate.
    let total = wrapped_row_count(&lines, inner_width);
    let max_scroll = total.saturating_sub(visible);
    let clamped = app.chat_scroll.min(max_scroll);
    let scroll_pos = max_scroll.saturating_sub(clamped);

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Chat "))
        .wrap(Wrap { trim: false })
        .scroll((scroll_pos, 0));
    f.render_widget(para, area);
}

/// Estimate the number of terminal rows a `Vec<Line>` will occupy when
/// rendered with `Wrap { trim: false }` at the given inner width. Each
/// logical line contributes `ceil(visual_width / inner_width).max(1)` rows.
/// Uses `char_indices().count()` as the visual-width proxy — accurate for
/// ASCII and most of our output; CJK / combining marks may still drift by
/// a row or two, which is acceptable for scroll clamping.
fn wrapped_row_count(lines: &[Line<'_>], inner_width: u16) -> u16 {
    let w = inner_width.max(1) as usize;
    let mut total: u32 = 0;
    for line in lines {
        let visual: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        let rows = visual.div_ceil(w).max(1);
        total = total.saturating_add(rows as u32);
    }
    total.min(u32::from(u16::MAX)) as u16
}

fn render_tool_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let max_entries = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = app
        .all_tool_records
        .iter()
        .rev()
        .take(max_entries)
        .map(|rec| {
            if let (Some(ok), Some(elapsed_ms)) = (rec.ok, rec.elapsed_ms) {
                let (icon, color) = if ok {
                    ("✓", Color::Green)
                } else {
                    ("✗", Color::Red)
                };
                Line::from(Span::styled(
                    format!("{icon} {} ({elapsed_ms}ms)", rec.name),
                    Style::default().fg(color),
                ))
            } else {
                Line::from(Span::styled(
                    format!("⟳ {}", rec.name),
                    Style::default().fg(Color::Yellow),
                ))
            }
        })
        .collect();

    let para = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Tools "));
    f.render_widget(para, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tools tab
// ─────────────────────────────────────────────────────────────────────────────

fn render_tools_tab(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    if app.all_tool_records.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No tool calls yet.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for rec in &app.all_tool_records {
            let header = if let (Some(ok), Some(ms)) = (rec.ok, rec.elapsed_ms) {
                let (icon, color) = if ok {
                    ("✓", Color::Green)
                } else {
                    ("✗", Color::Red)
                };
                Line::from(vec![
                    Span::styled(
                        format!("  {icon} "),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        rec.name.clone(),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  ({ms}ms)"), Style::default().fg(Color::DarkGray)),
                ])
            } else {
                Line::from(vec![
                    Span::styled(
                        "  ⟳ ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        rec.name.clone(),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  (running…)", Style::default().fg(Color::DarkGray)),
                ])
            };
            lines.push(header);
            lines.push(Line::from(vec![
                Span::styled("    › ", Style::default().fg(Color::DarkGray)),
                Span::styled(rec.input_preview.clone(), Style::default().fg(Color::Gray)),
            ]));
            if let Some(ref result) = rec.result_preview {
                let color = if rec.ok.unwrap_or(true) {
                    Color::White
                } else {
                    Color::LightRed
                };
                lines.push(Line::from(vec![
                    Span::styled("    ← ", Style::default().fg(Color::DarkGray)),
                    Span::styled(result.clone(), Style::default().fg(color)),
                ]));
            }
            lines.push(Line::from(Span::raw("")));
        }
    }

    let title = format!(" Tool Event Log ({} calls) ", app.all_tool_records.len());
    let total = lines.len() as u16;
    let visible = area.height.saturating_sub(2);
    let max_top = total.saturating_sub(visible);
    let scroll_pos = if app.tools_scroll == 0 {
        max_top
    } else {
        max_top.saturating_sub(app.tools_scroll.min(max_top))
    };

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title.as_str()))
        .wrap(Wrap { trim: false })
        .scroll((scroll_pos, 0));
    f.render_widget(para, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Notes tab
// ─────────────────────────────────────────────────────────────────────────────

fn render_notes_tab(f: &mut Frame, app: &App, area: Rect) {
    // 35% note list | 65% note body.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    render_notes_list(f, app, cols[0]);
    render_note_body(f, app, cols[1]);
}

fn render_notes_list(f: &mut Frame, app: &App, area: Rect) {
    let notes = &app.notes;

    let items: Vec<ListItem> = notes
        .entries
        .iter()
        .enumerate()
        .map(|(i, note)| {
            // Short date from the Created timestamp: "04-14".
            let date = if note.created.len() >= 10 {
                &note.created[5..10]
            } else {
                "??-??"
            };
            let tag_str = if note.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", note.tags.join(", "))
            };
            let text = format!("{date} {}{tag_str}", note.title);
            let style = if i == notes.selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            ListItem::new(text).style(style)
        })
        .collect();

    let mut title = format!(" Notes ({}) ", notes.entries.len());
    if let Some(ref tag) = notes.tag_filter {
        title = format!(" Notes ({}) — tag:{tag} ", notes.entries.len());
    }
    let hint = if notes.entries.is_empty() {
        " f=filter ↑↓=select "
    } else {
        ""
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_bottom(hint),
        )
        .highlight_symbol("> ")
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();
    if !notes.entries.is_empty() {
        state.select(Some(notes.selected));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn render_note_body(f: &mut Frame, app: &App, area: Rect) {
    let notes = &app.notes;

    let (title, lines) = if let Some(note) = notes.entries.get(notes.selected) {
        let mut lines: Vec<Line> = Vec::new();
        // Header.
        lines.push(Line::from(Span::styled(
            format!("# {}", note.title),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )));
        if !note.created.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("Created: {}", note.created),
                Style::default().fg(Color::DarkGray),
            )));
        }
        if !note.tags.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("Tags: {}", note.tags.join(", ")),
                Style::default().fg(Color::Yellow),
            )));
        }
        lines.push(Line::from(Span::raw("")));
        // Body.
        for line in note.body.lines() {
            lines.push(Line::from(Span::raw(line.to_string())));
        }
        (format!(" {} ", note.title), lines)
    } else {
        let lines = vec![Line::from(Span::styled(
            "  No notes found. Create notes with Claudette: \"create a note about ...\"",
            Style::default().fg(Color::DarkGray),
        ))];
        (" Note ".to_string(), lines)
    };

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((app.notes.body_scroll, 0));
    f.render_widget(para, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Todos tab
// ─────────────────────────────────────────────────────────────────────────────

fn render_todos_tab(f: &mut Frame, app: &App, area: Rect) {
    let todos = &app.todos;
    let done_count = todos.items.iter().filter(|t| t.done).count();
    let total = todos.items.len();
    let title = format!(" Todos ({total} — {done_count} done) ");

    let items: Vec<ListItem> = todos
        .items
        .iter()
        .map(|todo| {
            let check = if todo.done { "[x]" } else { "[ ]" };
            let date = if todo.created_at.len() >= 10 {
                &todo.created_at[..10]
            } else {
                ""
            };
            let style = if todo.done {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(format!(" {check} {:<50} {date}", todo.text)).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_symbol("> ")
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();
    if !todos.items.is_empty() {
        state.select(Some(todos.selected));
    }
    f.render_stateful_widget(list, area, &mut state);
}

// ─────────────────────────────────────────────────────────────────────────────
// HW tab
// ─────────────────────────────────────────────────────────────────────────────

fn render_hw_tab(f: &mut Frame, app: &App, area: Rect) {
    let hw = &app.hw;
    let mut lines: Vec<Line> = Vec::new();

    // ── Ollama status ─────────────────────────────────────────────────────
    lines.push(Line::from(Span::raw("")));
    let (dot, dot_color, status_text) = if hw.ollama_online {
        ("●", Color::Green, "Online")
    } else {
        ("●", Color::Red, "Offline")
    };
    lines.push(Line::from(vec![
        Span::styled(
            "  Ollama  ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{dot} {status_text}"),
            Style::default().fg(dot_color),
        ),
        Span::styled(
            format!("                       {}", hw.ollama_url),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    lines.push(Line::from(Span::raw("")));

    // ── Error (if any) ────────────────────────────────────────────────────
    if let Some(ref err) = hw.last_error {
        lines.push(Line::from(Span::styled(
            format!("  Error: {err}"),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(Span::raw("")));
    }

    // ── Loaded models ─────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        "  Loaded models:",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    if hw.models.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (none)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for m in &hw.models {
            // GPU offload: higher=better (100% = all on GPU, 0% = all CPU).
            let gpu_color = if m.gpu_percent == 100 {
                Color::Green
            } else if m.gpu_percent >= 70 {
                Color::Yellow
            } else {
                Color::Red
            };
            const MINI_BAR: usize = 8;
            let mini_filled = (m.gpu_percent as usize * MINI_BAR / 100).min(MINI_BAR);
            let mini_empty = MINI_BAR - mini_filled;
            lines.push(Line::from(vec![
                Span::styled(
                    format!("    {:<24}", m.name),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} VRAM", format_bytes(m.size_vram)),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled("  │  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} disk", format_bytes(m.size_disk)),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled("  │  ", Style::default().fg(Color::DarkGray)),
                Span::styled("█".repeat(mini_filled), Style::default().fg(gpu_color)),
                Span::styled("░".repeat(mini_empty), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!(" {}% GPU", m.gpu_percent),
                    Style::default().fg(gpu_color).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }
    lines.push(Line::from(Span::raw("")));

    // ── VRAM usage bar ────────────────────────────────────────────────────
    let total_vram_bytes = (hw.total_vram_gb * 1_073_741_824.0) as u64;
    let used_vram: u64 = hw.models.iter().map(|m| m.size_vram).sum();
    let vram_ratio = if total_vram_bytes > 0 {
        (used_vram as f64 / total_vram_bytes as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let bar_color = if vram_ratio < 0.7 {
        Color::Green
    } else if vram_ratio < 0.9 {
        Color::Yellow
    } else {
        Color::Red
    };
    const VBAR: usize = 30;
    let filled = (vram_ratio * VBAR as f64).round() as usize;
    let empty = VBAR - filled;

    lines.push(Line::from(vec![
        Span::styled(
            "  VRAM  ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("█".repeat(filled), Style::default().fg(bar_color)),
        Span::styled("░".repeat(empty), Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("  {} / {:.1} GB", format_bytes(used_vram), hw.total_vram_gb,),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            format!("  ({:.0}%)", vram_ratio * 100.0),
            Style::default().fg(bar_color).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(Span::raw("")));

    // ── Refresh info ──────────────────────────────────────────────────────
    let elapsed = Instant::now().duration_since(hw.last_refresh).as_secs();
    let next_in = HW_REFRESH_SECS.saturating_sub(elapsed);
    lines.push(Line::from(Span::styled(
        format!("  Auto-refresh: every {HW_REFRESH_SECS}s  │  Next in {next_in}s"),
        Style::default().fg(Color::DarkGray),
    )));

    let para =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Hardware "));
    f.render_widget(para, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Placeholder (unreachable now but keeps the exhaustive match happy)
// ─────────────────────────────────────────────────────────────────────────────

fn render_placeholder(f: &mut Frame, _tab: u8, area: Rect) {
    let para = Paragraph::new("\n  Coming soon.")
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(para, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Status bar
// ─────────────────────────────────────────────────────────────────────────────

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let threshold = app.threshold.max(1);
    let ratio = (app.estimated_tokens as f64 / threshold as f64).clamp(0.0, 1.0);
    let gauge_color = if ratio < 0.6 {
        Color::Green
    } else if ratio < 0.85 {
        Color::Yellow
    } else {
        Color::Red
    };

    const BAR: usize = 10;
    let filled = (ratio * BAR as f64).round() as usize;
    let empty = BAR - filled;

    // Semantic status word — yellow for running, green for responding, dim
    // gray for idle.
    let (status_word, status_color) = status_word_for(app);

    let scroll_hint = match app.active_tab {
        TAB_CHAT if app.chat_scroll > 0 => "↑PgUp ↓PgDn",
        TAB_TOOLS if app.tools_scroll > 0 => "↑PgUp ↓PgDn",
        TAB_NOTES if app.notes.body_scroll > 0 => "↑PgUp ↓PgDn",
        _ => "",
    };

    let tab_hint = match app.active_tab {
        TAB_NOTES if app.notes.filter_editing => "typing filter…",
        TAB_NOTES => "f=filter ↑↓=select",
        TAB_TODOS => "↑↓=select Space=toggle",
        TAB_HW => "auto-refresh 10s",
        _ => "Tab switch",
    };

    let bg = Style::default().bg(Color::DarkGray);
    // Separator: light-gray glyph on darkgray — visible without grabbing focus.
    let sep = Span::styled(" │ ", Style::default().fg(Color::Gray).bg(Color::DarkGray));
    // Hints: white on darkgray so they stay readable at terminal contrast.
    let hint = Style::default().fg(Color::White).bg(Color::DarkGray);

    let mut spans: Vec<Span> = vec![
        Span::styled(" ", bg),
        // Status word — bold, semantic color against darkgray.
        Span::styled(
            status_word,
            Style::default()
                .fg(status_color)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        sep.clone(),
        // Token gauge — filled block in gauge color, empty in dim gray.
        Span::styled(
            "█".repeat(filled),
            Style::default().fg(gauge_color).bg(Color::DarkGray),
        ),
        Span::styled(
            "░".repeat(empty),
            Style::default().fg(Color::DarkGray).bg(Color::DarkGray),
        ),
        // Token value stays white so the numbers are easy to read; the
        // gauge already carries the color signal.
        Span::styled(
            format!(
                " {:.1}K/{:.0}K",
                app.estimated_tokens as f64 / 1000.0,
                threshold as f64 / 1000.0,
            ),
            Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        sep.clone(),
        // Message counter — bright cyan, bold.
        Span::styled(
            format!("{} msgs", app.history.len()),
            Style::default()
                .fg(Color::LightCyan)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        sep.clone(),
        Span::styled(tab_hint.to_string(), hint),
    ];

    if !scroll_hint.is_empty() {
        spans.push(sep.clone());
        spans.push(Span::styled(
            scroll_hint.to_string(),
            Style::default()
                .fg(Color::LightYellow)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ));
    }

    spans.push(sep);
    spans.push(Span::styled("Ctrl+C quit", hint));

    let status = Paragraph::new(Line::from(spans)).style(bg);
    f.render_widget(status, area);
}

/// One-word status descriptor for the status bar — reflects what the worker
/// is doing right now. Matches the phase logic in `render_input`.
fn status_word_for(app: &App) -> (&'static str, Color) {
    if !app.working {
        // Idle: bright white on darkgray — the state you see most, so
        // it needs to be clearly legible.
        return ("ready", Color::White);
    }
    if !app.streaming_text.is_empty() {
        return ("responding", Color::LightGreen);
    }
    match app.current_turn_tools.last() {
        Some(ToolEntry::Start { .. }) => ("running", Color::LightYellow),
        Some(ToolEntry::Done { .. }) => ("processing", Color::LightCyan),
        None => ("thinking", Color::LightYellow),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Input box
// ─────────────────────────────────────────────────────────────────────────────

fn render_input(f: &mut Frame, app: &App, area: Rect) {
    let (content, style) = if app.notes.filter_editing && app.active_tab == TAB_NOTES {
        (
            format!(" Filter by tag: {}_", app.notes.filter_input),
            Style::default().fg(Color::Yellow),
        )
    } else if app.working {
        // Richer state indicator based on where we are in the turn.
        let (msg, color) = if !app.streaming_text.is_empty() {
            ("Claudette is responding", Color::Green)
        } else {
            match app.current_turn_tools.last() {
                Some(ToolEntry::Start { name }) => {
                    let spinner = if app.cursor_phase { "⟳" } else { "·" };
                    return render_input_line(
                        f,
                        area,
                        format!(" {spinner} running {name}…"),
                        Style::default().fg(Color::Yellow),
                    );
                }
                Some(ToolEntry::Done { .. }) => ("processing results", Color::Cyan),
                None => ("Claudette is thinking", Color::DarkGray),
            }
        };
        let spinner = if app.cursor_phase { "⟳" } else { "·" };
        (format!(" {spinner} {msg}…"), Style::default().fg(color))
    } else {
        (
            format!(" > {}_", app.input),
            Style::default().fg(Color::White),
        )
    };
    render_input_line(f, area, content, style);
}

fn render_input_line(f: &mut Frame, area: Rect, content: String, style: Style) {
    let widget = Paragraph::new(content)
        .style(style)
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(widget, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_row_count_short_lines_one_row_each() {
        let lines = vec![Line::from("hi"), Line::from("world"), Line::from("")];
        assert_eq!(wrapped_row_count(&lines, 80), 3);
    }

    #[test]
    fn wrapped_row_count_long_line_wraps_to_multiple_rows() {
        // 100-char line at width 40 → ceil(100/40) = 3 rows.
        let long = "x".repeat(100);
        let lines = vec![Line::from(long)];
        assert_eq!(wrapped_row_count(&lines, 40), 3);
    }

    #[test]
    fn wrapped_row_count_empty_line_still_counts_as_one_row() {
        let lines = vec![Line::from("")];
        assert_eq!(wrapped_row_count(&lines, 80), 1);
    }

    #[test]
    fn wrapped_row_count_handles_zero_width_without_panic() {
        let lines = vec![Line::from("abc")];
        // Width 0 clamps to 1 — 3-char line at width 1 → 3 rows.
        assert_eq!(wrapped_row_count(&lines, 0), 3);
    }

    #[test]
    fn wrapped_row_count_sums_multi_span_width() {
        // Line with two spans of 30 chars each = 60 chars visual.
        // At width 40 → ceil(60/40) = 2 rows.
        let line = Line::from(vec![Span::raw("a".repeat(30)), Span::raw("b".repeat(30))]);
        assert_eq!(wrapped_row_count(&[line], 40), 2);
    }
}
