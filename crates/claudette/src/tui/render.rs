//! Render-side of the Ratatui TUI.
//!
//! Extracted from `tui.rs` on 2026-05-15 — the parent file was approaching
//! 1900 lines and the audit flagged the size as a maintainability problem.
//! Render code has no state of its own (every `render_*` is `(f, app,
//! area)` → unit), so moving it here keeps `tui.rs` focused on the event
//! loop and shared state.
//!
//! All `render_*` helpers stay private; only [`render`] is exposed back to
//! the parent so the event loop can call it from `terminal.draw(...)`.

use std::time::Instant;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use super::{
    format_bytes, App, ToolEntry, HW_REFRESH_SECS, TAB_CHAT, TAB_HW, TAB_NOTES, TAB_TODOS,
    TAB_TOOLS,
};

// ─────────────────────────────────────────────────────────────────────────────
// Top-level render
// ─────────────────────────────────────────────────────────────────────────────

pub(super) fn render(f: &mut Frame, app: &App) {
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

    // Space Invaders easter egg — overlay covers the whole area when active.
    if let Some(game) = app.space_game.as_ref() {
        game.draw(f, area);
    }
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
            format!("  {} / {:.1} GB", format_bytes(used_vram), hw.total_vram_gb),
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
    } else if let Some(notice) = &app.paste_notice {
        // Paste feedback wins over the regular prompt for one keypress.
        // Cleared on the next character/backspace so it doesn't linger.
        (format!(" {notice}"), Style::default().fg(Color::Cyan))
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
        let attach = if app.pending_images.is_empty() {
            String::new()
        } else {
            format!(" [📎 {}]", app.pending_images.len())
        };
        (
            format!(" > {}_{attach}", app.input),
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
