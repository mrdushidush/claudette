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
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError};
use std::time::{Duration, Instant};

use crate::Session;
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::tui_events::{ImageAttachment, TuiEvent, UserInput};
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
        // Detected (nvidia-smi) → CLAUDETTE_VRAM_GB → 8.0; one shell-out
        // at TUI startup, not per-frame.
        let (total_vram_gb, _) = crate::hw::resolve_vram_gb();
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

/// A pending `DangerFullAccess` permission prompt (Sprint G / PR5). The
/// worker thread is parked inside `PermissionPrompter::decide()` until the
/// user answers over `resp_tx` — or until this struct drops (any render-loop
/// exit path), which the worker reads as a deny.
struct PermissionPrompt {
    tool_name: String,
    /// Full tool input — display-side wrap + scroll, never truncated.
    input: String,
    /// e.g. "danger-full-access".
    required_mode: String,
    /// Vertical scroll offset for long inputs (↑/↓).
    scroll: u16,
    /// Rendezvous answer channel: `true` → allow, `false` → deny.
    resp_tx: SyncSender<bool>,
}

/// All mutable TUI state — owned entirely by the render loop thread.
struct App {
    // ── Conversation ──────────────────────────────────────────────────────
    history: Vec<Message>,
    streaming_text: String,
    current_turn_tools: Vec<ToolEntry>,
    input: String,
    /// Images staged for the next submit (populated by Ctrl+V or by
    /// `@path` tokens detected when the user presses Enter). Cleared on
    /// every successful send and on Esc.
    pending_images: Vec<ImageAttachment>,
    /// Transient one-line notice shown in the input row (e.g. "📎 image
    /// attached", "clipboard empty"). Cleared on the next keypress.
    paste_notice: Option<String>,
    /// Large-paste temp-file buffer (>500 chars stashed to disk to keep the
    /// input widget responsive). Empty until a big paste arrives.
    paste_file: paste::PasteFile,
    working: bool,

    // ── Permissions ───────────────────────────────────────────────────────
    /// Pending DangerFullAccess confirmation modal. Outranks every other
    /// input mode while `Some` — the worker thread is blocked on the answer.
    permission_prompt: Option<PermissionPrompt>,

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
            pending_images: Vec::new(),
            paste_notice: None,
            paste_file: paste::PasteFile::new(),
            working: false,
            permission_prompt: None,
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
    #[allow(clippy::too_many_lines)]
    fn handle_tui_event(&mut self, event: TuiEvent) {
        match event {
            TuiEvent::Token(delta) => self.streaming_text.push_str(&delta),

            TuiEvent::PermissionRequest {
                tool_name,
                input,
                required_mode,
                resp_tx,
            } => {
                // A permission question outranks any pane edit mode —
                // force-close it so its input branch can't starve the modal
                // (the worker is blocked until the user answers).
                self.notes.filter_editing = false;
                self.permission_prompt = Some(PermissionPrompt {
                    tool_name,
                    input,
                    required_mode,
                    scroll: 0,
                    resp_tx,
                });
            }

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
            }

            TuiEvent::Saved => {}

            TuiEvent::Info(text) => {
                if !text.trim().is_empty() {
                    self.history.push(Message {
                        role: "System".to_string(),
                        text,
                    });
                }
            }
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
// Image attachment helpers (vision input)
// ─────────────────────────────────────────────────────────────────────────────

use crate::image_attach::{
    attachment_from_file, encode_base64_standard, extract_image_attachments_from_input,
    image_mime_from_path,
};

/// Try to grab an image from the OS clipboard. On success returns one
/// `ImageAttachment` (PNG-encoded). On failure, returns the clipboard
/// text content if any (for the caller to handle as a possible path or
/// fall through to literal text paste).
enum ClipboardPaste {
    Image(ImageAttachment),
    Text(String),
    Empty,
}

fn read_clipboard_paste() -> ClipboardPaste {
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return ClipboardPaste::Empty;
    };
    if let Ok(img) = clipboard.get_image() {
        // arboard hands back raw RGBA. Re-encode to PNG so vision models
        // can ingest it — they don't accept raw pixel buffers.
        let Some(buf) =
            image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
        else {
            return ClipboardPaste::Empty;
        };
        let mut png_bytes: Vec<u8> = Vec::new();
        let dynimg = image::DynamicImage::ImageRgba8(buf);
        if dynimg
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .is_err()
        {
            return ClipboardPaste::Empty;
        }
        return ClipboardPaste::Image(ImageAttachment {
            media_type: "image/png".to_string(),
            data_b64: encode_base64_standard(&png_bytes),
        });
    }
    if let Ok(text) = clipboard.get_text() {
        return ClipboardPaste::Text(text);
    }
    ClipboardPaste::Empty
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

pub fn run_tui(session: Session) -> Result<()> {
    // Restore the terminal even if a panic blows past the `scopeguard::defer!`
    // below. The release profile builds with `panic = "abort"` (see the root
    // Cargo.toml), and `defer!` only fires on normal return or unwind — under
    // `abort` it is SKIPPED. Without this hook a panic mid-session would leave
    // the user's shell in raw mode + alt-screen (a garbled, unusable terminal),
    // which is the most plausible "it broke my shell" failure. A panic hook, by
    // contrast, runs on the panicking thread *before* the process aborts, so the
    // terminal is reset on every exit path. We chain to the previous hook so the
    // panic message still prints. The restore calls are idempotent no-ops once
    // the terminal is back to cooked mode, so leaving the hook installed after a
    // clean exit is harmless.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            std::io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        previous_hook(info);
    }));

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    scopeguard::defer! {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
    }

    let (tui_tx, tui_rx) = std::sync::mpsc::sync_channel::<TuiEvent>(512);
    let (user_tx, user_rx) = std::sync::mpsc::channel::<UserInput>();
    let _worker = tui_worker::spawn_worker(session, user_rx, tui_tx);

    run_loop(&mut terminal, &tui_rx, &user_tx)
}

// ─────────────────────────────────────────────────────────────────────────────
// Event loop
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
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

        // Permission modal — highest-priority input mode; owns its own
        // draw + poll, then loops. The event handler force-closes the
        // pane edit modes on arrival so they can't starve it.
        // The worker thread is parked in `decide()` until we answer via
        // `resp_tx`; every exit path that drops the prompt instead (quit,
        // `?` error) reads as a deny on the worker side.
        if app.permission_prompt.is_some() {
            terminal.draw(|f| render::render(f, &app))?;
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(k) = event::read()? {
                    // Windows fires Press + Release — Press only.
                    if k.kind == KeyEventKind::Press {
                        match (k.code, k.modifiers) {
                            // Quit: deny first so the worker unblocks
                            // promptly, then shut down as usual.
                            (KeyCode::Char('c' | 'd'), KeyModifiers::CONTROL) => {
                                if let Some(p) = app.permission_prompt.take() {
                                    let _ = p.resp_tx.send(false);
                                }
                                let _ = user_tx.send(UserInput::Quit);
                                return Ok(());
                            }
                            (
                                KeyCode::Char('y' | 'Y'),
                                KeyModifiers::NONE | KeyModifiers::SHIFT,
                            ) => {
                                if let Some(p) = app.permission_prompt.take() {
                                    let _ = p.resp_tx.send(true);
                                }
                            }
                            // Default-deny, matching CliPrompter's
                            // anything-but-y semantics on Enter.
                            (
                                KeyCode::Char('n' | 'N'),
                                KeyModifiers::NONE | KeyModifiers::SHIFT,
                            )
                            | (KeyCode::Esc | KeyCode::Enter, _) => {
                                if let Some(p) = app.permission_prompt.take() {
                                    let _ = p.resp_tx.send(false);
                                }
                            }
                            // Long inputs scroll; everything else is
                            // swallowed while the modal is up.
                            (KeyCode::Up, _) => {
                                if let Some(p) = app.permission_prompt.as_mut() {
                                    p.scroll = p.scroll.saturating_sub(1);
                                }
                            }
                            (KeyCode::Down, _) => {
                                if let Some(p) = app.permission_prompt.as_mut() {
                                    p.scroll = p.scroll.saturating_add(1);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            continue;
        }

        terminal.draw(|f| render::render(f, &app))?;

        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let key = match event::read()? {
            Event::Key(k) => k,
            // Bracketed paste — Windows Terminal delivers drag-dropped
            // file paths and Ctrl+V text content this way as a single
            // event, not as a stream of Char keypresses. If the pasted
            // text resolves to an image-file path, attach it; otherwise
            // splice into the input buffer as if the user had typed it.
            Event::Paste(text) if !app.working => {
                app.paste_notice = None;
                let trimmed = text.trim();
                let candidate = trimmed.trim_matches('"');
                let path = std::path::Path::new(candidate);
                if image_mime_from_path(path).is_some() && path.is_file() {
                    match attachment_from_file(path) {
                        Ok(att) => {
                            app.pending_images.push(att);
                            app.paste_notice = Some(format!(
                                "📎 image attached from drop ({} total)",
                                app.pending_images.len()
                            ));
                        }
                        Err(e) => app.paste_notice = Some(format!("paste failed: {e}")),
                    }
                } else if app.paste_file.try_store(&text) {
                    // Large paste — stashed in a temp file; the input bar
                    // shows a preview and the full content is retrieved on
                    // submit. See `tui/paste.rs`.
                    app.paste_notice = Some(app.paste_file.display());
                } else {
                    for c in text.chars() {
                        if c != '\r' && c != '\n' {
                            app.input.push(c);
                        }
                    }
                }
                continue;
            }
            _ => continue,
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
            (KeyCode::Esc, _) if app.active_tab == TAB_NOTES && app.notes.tag_filter.is_some() => {
                app.notes.tag_filter = None;
                app.notes.needs_refresh = true;
                app.notes.selected = 0;
                app.notes.body_scroll = 0;
            }

            // ── Todos tab: Up/Down selection, Space/Enter toggle ──────
            (KeyCode::Up, _) if app.active_tab == TAB_TODOS => {
                app.todos.selected = app.todos.selected.saturating_sub(1);
            }
            (KeyCode::Down, _)
                if app.active_tab == TAB_TODOS
                    && app.todos.selected + 1 < app.todos.items.len() =>
            {
                app.todos.selected += 1;
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

            // Paste from clipboard. Bitmap → PNG-attached image; text →
            // if it parses as an image-file path, attach; otherwise append
            // to the input line as if the user had typed it.
            //
            // **Alt+V, not Ctrl+V** — Windows Terminal (and most modern
            // terminals) intercept Ctrl+V at the terminal level and paste
            // the clipboard's *text* form as a stream of Char events, so a
            // Ctrl+V key event never reaches the TUI. Alt+V passes through
            // unmodified on every terminal we care about.
            (KeyCode::Char('v'), KeyModifiers::ALT) if !app.working => {
                app.paste_notice = None;
                match read_clipboard_paste() {
                    ClipboardPaste::Image(att) => {
                        app.pending_images.push(att);
                        app.paste_notice = Some(format!(
                            "📎 image attached ({} total)",
                            app.pending_images.len()
                        ));
                    }
                    ClipboardPaste::Text(text) => {
                        let trimmed = text.trim().trim_matches('"');
                        let path = std::path::Path::new(trimmed);
                        if image_mime_from_path(path).is_some() && path.is_file() {
                            match attachment_from_file(path) {
                                Ok(att) => {
                                    app.pending_images.push(att);
                                    app.paste_notice = Some(format!(
                                        "📎 image attached from path ({} total)",
                                        app.pending_images.len()
                                    ));
                                }
                                Err(e) => {
                                    app.paste_notice = Some(format!("paste failed: {e}"));
                                }
                            }
                        } else {
                            // Plain text paste — append as if typed. Strip
                            // CR/LF so a multi-line paste doesn't spuriously
                            // submit on the embedded newline.
                            for c in text.chars() {
                                if c != '\r' && c != '\n' {
                                    app.input.push(c);
                                }
                            }
                        }
                    }
                    ClipboardPaste::Empty => {
                        app.paste_notice = Some("clipboard empty or unreadable".to_string());
                    }
                }
            }

            // Drop staged attachments (Esc clears the pending image queue
            // when input is empty so the user can back out of a misclick).
            (KeyCode::Esc, _) if !app.pending_images.is_empty() && app.input.is_empty() => {
                app.pending_images.clear();
                app.paste_notice = Some("attachments cleared".to_string());
            }

            // Esc clears a stashed large paste when input is empty.
            (KeyCode::Esc, _) if app.paste_file.is_active() && app.input.is_empty() => {
                app.paste_file.clear();
                app.paste_notice = Some("paste cleared".to_string());
            }

            // Submit input.
            (KeyCode::Enter, _)
                if !app.working
                    && (!app.input.is_empty()
                        || !app.pending_images.is_empty()
                        || app.paste_file.is_active()) =>
            {
                let typed = std::mem::take(&mut app.input);
                // When a big paste is stashed, the typed text is treated as
                // a prefix/context and the paste body is appended.
                let text = if app.paste_file.is_active() {
                    let body = app.paste_file.retrieve().unwrap_or_default();
                    app.paste_file.clear();
                    if typed.trim().is_empty() {
                        body
                    } else {
                        format!("{typed}\n\n{body}")
                    }
                } else {
                    typed
                };
                app.paste_notice = None;
                app.chat_scroll = 0;
                if let Some(cmd) = text.strip_prefix('/') {
                    let _ = user_tx.send(UserInput::SlashCommand(cmd.to_string()));
                    app.pending_images.clear();
                } else {
                    // Merge clipboard-staged images with any path tokens
                    // typed/drag-dropped into this line.
                    let mut images = std::mem::take(&mut app.pending_images);
                    let extracted = extract_image_attachments_from_input(&text);
                    let staged_count = images.len();
                    let extension_matches = extracted.extension_matches;
                    let attach_failure = extracted.first_failure.clone();
                    images.extend(extracted.attached);

                    // Surface diagnostics in the next-turn history slot —
                    // a silent miss is what got us here in the first place.
                    if extension_matches > 0 && images.len() == staged_count {
                        if let Some(reason) = attach_failure {
                            app.history.push(Message {
                                role: "Error".to_string(),
                                text: format!(
                                    "image-path token detected but couldn't attach: {reason}"
                                ),
                            });
                        }
                    }

                    let display_text = if images.is_empty() {
                        text.clone()
                    } else {
                        format!("{text}  [📎 {}]", images.len())
                    };
                    app.history.push(Message {
                        role: "You".to_string(),
                        text: display_text,
                    });
                    app.active_tab = TAB_CHAT;
                    let _ = user_tx.send(UserInput::Message { text, images });
                }
            }

            // Typing.
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT)
                if !app.working && (!app.input.is_empty() || !c.is_ascii_digit()) =>
            {
                app.paste_notice = None;
                app.input.push(c);
            }
            (KeyCode::Backspace, _) if !app.working => {
                app.paste_notice = None;
                app.input.pop();
            }

            _ => {}
        }
    }
}

mod paste;
mod render;
