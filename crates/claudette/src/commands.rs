//! Slash-command dispatcher for the claudette REPL.
//!
//! Slash commands are how the user controls the REPL itself (sessions,
//! memory, status, exit) without sending text to the model. They are matched
//! in [`run::run_secretary_repl`] BEFORE the line is dispatched to the LLM:
//! any line starting with `/` is parsed via [`parse_slash_command`], and a
//! non-`None` return value is handled inside the REPL loop without ever
//! reaching the model.
//!
//! Adding a new command is a 5-step touch:
//! 1. Add a variant to [`SlashCommand`].
//! 2. Add the keyword (and any aliases) to [`parse_slash_command`].
//! 3. Add a handler arm to [`dispatch_slash_command`].
//! 4. Add the one-liner to [`print_help`] so `/help` lists it.
//! 5. Add a parser test in `mod tests`.
//!
//! All REPL output goes to stderr — never stdout — so piping the assistant's
//! actual replies into a file still works cleanly. From the TUI, callers pass
//! a `Vec<u8>` buffer instead and ship it as a `TuiEvent::Info` message.

use std::io::Write;

use crate::{
    compact_session, ApiClient, CompactionConfig, ConversationRuntime, Session, ToolExecutor,
};

use crate::api::current_num_ctx;
use crate::memory::{default_memory_path, try_load_memory, MAX_MEMORY_CHARS};
use crate::model_config::{self, ModelConfig, Preset};
use crate::run::{compact_threshold, current_model, default_session_path, sessions_dir};
use crate::theme;
use crate::tool_groups::{ToolGroup, ToolRegistry};
use crate::tools::secretary_tools_json;

// === Public types ============================================================

/// One slash command parsed from a REPL line. Carries any string arg directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Help,
    Clear,
    Compact,
    Sessions,
    /// Remove a saved session from `~/.claudette/sessions/`. Slash form is
    /// `/sessions delete <name>` (mirrors git/docker subcommand pattern).
    SessionsDelete(String),
    /// Rename a saved session. Slash form is `/sessions rename <old> <new>`.
    /// Carries both names already sanitised by the parser.
    SessionsRename {
        old: String,
        new: String,
    },
    Save(String),
    Load(String),
    Status,
    Cost,
    Tools,
    Model,
    Memory,
    Reload,
    Capabilities,
    Exit,
    Validate(String),
    Agents,
    /// Sprint 14: swap the whole preset bundle (brain + fallback) in one
    /// command. `fast` / `auto` / `smart`.
    PresetSwitch(Preset),
    /// Sprint 14: pin the brain model. `auto` re-enables the current
    /// preset's fallback; any other value pins the brain and disables
    /// auto-fallback. Lives for the process (no persistence).
    Brain(String),
    /// Sprint 14: pin the coder model. Out of the fallback scope — just
    /// a convenience wrapper over `CLAUDETTE_CODER_MODEL`.
    Coder(String),
    /// Sprint 14: dump the current model config (brain, fallback, coder,
    /// last fallback timestamp from `fallback.jsonl`).
    Models,
    /// Cross-session recall: search the embedded memory for past messages
    /// matching the query. Bypasses the brain — runs the same lookup as the
    /// `recall` tool would, but prints results straight to the REPL.
    Recall(String),
    /// Phase 2 brownfield shortcut: clone a target repo and make it the
    /// active mission in one shot. Thin wrapper over the `mission_start`
    /// tool — exposes it from the REPL without going through the brain.
    Brownfield(String),
    /// 0.4.2 forge-mode v0a: run the trailing prompt against the active
    /// brownfield mission with file/search/git/advanced/github tools
    /// pre-enabled, ending at `mission_submit` (auto-PR). Errors if no
    /// mission is active. Mirrors the `--forge "<prompt>"` CLI flag from
    /// inside the REPL.
    Forge(String),
    /// Recognised as starting with `/` but unusable: unknown name, missing
    /// required argument, etc. Carries a human-readable error.
    Invalid(String),
}

/// What the REPL should do after a slash command finishes running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashOutcome {
    /// Command handled, REPL continues to the next line.
    Continue,
    /// User asked to leave the REPL.
    Exit,
}

/// Per-REPL accumulators that the dispatcher reads for `/status` and `/cost`.
/// Lives in the REPL loop, not in the runtime, because the runtime's own
/// `UsageTracker` resets when we rebuild on `/clear` / `/load` / `/reload` —
/// but we want lifetime-of-the-process stats too.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReplState {
    pub cumulative_input_tokens: u64,
    pub cumulative_output_tokens: u64,
    pub turn_count: u32,
}

impl ReplState {
    /// Add one turn's token usage to the running totals.
    pub fn record_turn(&mut self, input: u32, output: u32) {
        self.cumulative_input_tokens = self
            .cumulative_input_tokens
            .saturating_add(u64::from(input));
        self.cumulative_output_tokens = self
            .cumulative_output_tokens
            .saturating_add(u64::from(output));
        self.turn_count = self.turn_count.saturating_add(1);
    }
}

// === Parser ==================================================================

/// Parse a REPL line into a `SlashCommand`. Returns `None` if the line is not
/// a slash command (so the caller can fall through to `runtime.run_turn`).
/// Returns `Some(Invalid(...))` if it IS a slash command but unusable.
#[must_use]
pub fn parse_slash_command(line: &str) -> Option<SlashCommand> {
    let trimmed = line.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let body = &trimmed[1..];
    if body.is_empty() {
        return Some(SlashCommand::Invalid(
            "empty command — try /help".to_string(),
        ));
    }

    let mut parts = body.splitn(2, char::is_whitespace);
    let cmd = parts.next()?.to_lowercase();
    let arg = parts.next().map(|s| s.trim().to_string());

    let parsed = match cmd.as_str() {
        "help" | "h" | "?" => SlashCommand::Help,
        "clear" | "cl" => SlashCommand::Clear,
        "compact" => SlashCommand::Compact,
        "sessions" | "ls" => parse_sessions_subcommand(arg.as_deref()),
        "save" => match arg.filter(|s| !s.is_empty()) {
            Some(name) => SlashCommand::Save(name),
            None => SlashCommand::Invalid("/save requires a name. Try: /save my-work".to_string()),
        },
        "load" => match arg.filter(|s| !s.is_empty()) {
            Some(name) => SlashCommand::Load(name),
            None => SlashCommand::Invalid("/load requires a name. Try: /load my-work".to_string()),
        },
        "status" | "st" => SlashCommand::Status,
        "cost" => SlashCommand::Cost,
        "tools" => SlashCommand::Tools,
        "model" => SlashCommand::Model,
        "memory" | "mem" => SlashCommand::Memory,
        "reload" => SlashCommand::Reload,
        "capabilities" | "cap" => SlashCommand::Capabilities,
        "validate" | "val" => match arg.filter(|s| !s.is_empty()) {
            Some(path) => SlashCommand::Validate(path),
            None => SlashCommand::Invalid(
                "/validate requires a path. Try: /validate ~/.claudette/files/userClass.py"
                    .to_string(),
            ),
        },
        "agents" => SlashCommand::Agents,
        "preset" => match arg.filter(|s| !s.is_empty()) {
            Some(p) => match p.parse::<Preset>() {
                Ok(preset) => SlashCommand::PresetSwitch(preset),
                Err(e) => SlashCommand::Invalid(e),
            },
            None => SlashCommand::Invalid("/preset requires one of: fast, auto, smart".to_string()),
        },
        "brain" => match arg.filter(|s| !s.is_empty()) {
            Some(m) => SlashCommand::Brain(m),
            None => SlashCommand::Invalid("/brain requires a model name or 'auto'".to_string()),
        },
        "coder" => match arg.filter(|s| !s.is_empty()) {
            Some(m) => SlashCommand::Coder(m),
            None => SlashCommand::Invalid("/coder requires a model name".to_string()),
        },
        "models" => SlashCommand::Models,
        "recall" => match arg.filter(|s| !s.is_empty()) {
            Some(query) => SlashCommand::Recall(query),
            None => SlashCommand::Invalid(
                "/recall requires a query. Try: /recall meeting with brian".to_string(),
            ),
        },
        "brownfield" => match arg.filter(|s| !s.is_empty()) {
            Some(target) => SlashCommand::Brownfield(target),
            None => SlashCommand::Invalid(
                "/brownfield requires a target. Try: /brownfield owner/repo".to_string(),
            ),
        },
        "forge" => match arg.filter(|s| !s.is_empty()) {
            Some(prompt) => SlashCommand::Forge(prompt),
            None => SlashCommand::Invalid(
                "/forge requires a prompt. Try: /forge fix the parser bug".to_string(),
            ),
        },
        "exit" | "quit" | "q" | "x" => SlashCommand::Exit,
        other => SlashCommand::Invalid(format!("unknown command: /{other} — try /help")),
    };
    Some(parsed)
}

/// Sub-parser for the `/sessions` family. Today this covers the bare
/// "list" form and `/sessions delete <name>`. Centralised so the main
/// parser arm stays a one-liner.
fn parse_sessions_subcommand(arg: Option<&str>) -> SlashCommand {
    let arg = arg.unwrap_or("").trim();
    if arg.is_empty() {
        return SlashCommand::Sessions;
    }
    let mut parts = arg.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or("").to_lowercase();
    let rest = parts.next().map_or("", str::trim);
    match verb.as_str() {
        "delete" | "rm" | "remove" => {
            if rest.is_empty() {
                SlashCommand::Invalid(
                    "/sessions delete requires a name. Try: /sessions delete my-work".to_string(),
                )
            } else {
                SlashCommand::SessionsDelete(rest.to_string())
            }
        }
        "rename" | "mv" => {
            let mut split = rest.splitn(2, char::is_whitespace);
            let old = split.next().unwrap_or("").trim();
            let new = split.next().map_or("", str::trim);
            if old.is_empty() || new.is_empty() {
                SlashCommand::Invalid(
                    "/sessions rename requires two names. Try: /sessions rename old new"
                        .to_string(),
                )
            } else {
                SlashCommand::SessionsRename {
                    old: old.to_string(),
                    new: new.to_string(),
                }
            }
        }
        other => SlashCommand::Invalid(format!(
            "unknown /sessions subcommand: {other} — try /sessions, /sessions delete <name>, \
             or /sessions rename <old> <new>"
        )),
    }
}

// === Dispatcher ==============================================================

/// Run a parsed slash command. Generic over the concrete runtime so both the
/// REPL (`ConversationRuntime<OllamaApiClient, SecretaryToolExecutor>`) and
/// the TUI (`ConversationRuntime<OllamaApiClient, TuiToolExecutor>`) can use
/// the same dispatcher. `out` is where human-readable output is written —
/// stderr for the REPL, a buffer shipped via `TuiEvent::Info` for the TUI.
/// `rebuild` is the callback that produces a fresh runtime from a session;
/// it's used by commands that swap the conversation context (`/clear`,
/// `/load`, `/reload`, `/compact`, `/preset`, `/brain`).
pub fn dispatch_slash_command<C, T, W, R>(
    cmd: SlashCommand,
    runtime: &mut ConversationRuntime<C, T>,
    state: &ReplState,
    out: &mut W,
    rebuild: &R,
) -> SlashOutcome
where
    C: ApiClient,
    T: ToolExecutor,
    W: Write,
    R: Fn(Session) -> ConversationRuntime<C, T>,
{
    let outcome = match cmd {
        SlashCommand::Help => {
            print_help(out);
            SlashOutcome::Continue
        }
        SlashCommand::Clear => {
            *runtime = rebuild(Session::default());
            let _ = writeln!(
                out,
                "{} {}",
                theme::ok(theme::OK_GLYPH),
                theme::ok("session cleared (saved files on disk untouched)")
            );
            SlashOutcome::Continue
        }
        SlashCommand::Compact => {
            handle_compact(out, runtime, rebuild);
            SlashOutcome::Continue
        }
        SlashCommand::Sessions => {
            handle_sessions(out);
            SlashOutcome::Continue
        }
        SlashCommand::SessionsDelete(name) => {
            handle_sessions_delete(out, &name);
            SlashOutcome::Continue
        }
        SlashCommand::SessionsRename { old, new } => {
            handle_sessions_rename(out, &old, &new);
            SlashOutcome::Continue
        }
        SlashCommand::Save(name) => {
            handle_save(out, runtime, &name);
            SlashOutcome::Continue
        }
        SlashCommand::Load(name) => {
            handle_load(out, runtime, &name, rebuild);
            SlashOutcome::Continue
        }
        SlashCommand::Status => {
            handle_status(out, runtime, state);
            SlashOutcome::Continue
        }
        SlashCommand::Cost => {
            handle_cost(out, runtime, state);
            SlashOutcome::Continue
        }
        SlashCommand::Tools => {
            handle_tools(out);
            SlashOutcome::Continue
        }
        SlashCommand::Model => {
            handle_model(out);
            SlashOutcome::Continue
        }
        SlashCommand::Memory => {
            handle_memory(out);
            SlashOutcome::Continue
        }
        SlashCommand::Reload => {
            handle_reload(out, runtime, rebuild);
            SlashOutcome::Continue
        }
        SlashCommand::Capabilities => {
            handle_capabilities(out);
            SlashOutcome::Continue
        }
        SlashCommand::Validate(path) => {
            handle_validate(out, &path);
            SlashOutcome::Continue
        }
        SlashCommand::Agents => {
            handle_agents(out);
            SlashOutcome::Continue
        }
        SlashCommand::PresetSwitch(preset) => {
            handle_preset(out, runtime, preset, rebuild);
            SlashOutcome::Continue
        }
        SlashCommand::Brain(model) => {
            handle_brain(out, runtime, &model, rebuild);
            SlashOutcome::Continue
        }
        SlashCommand::Coder(model) => {
            handle_coder(out, &model);
            SlashOutcome::Continue
        }
        SlashCommand::Models => {
            handle_models(out);
            SlashOutcome::Continue
        }
        SlashCommand::Recall(query) => {
            handle_recall(out, &query);
            SlashOutcome::Continue
        }
        SlashCommand::Brownfield(target) => {
            handle_brownfield(out, &target);
            SlashOutcome::Continue
        }
        SlashCommand::Forge(prompt) => {
            handle_forge(out, &prompt);
            SlashOutcome::Continue
        }
        SlashCommand::Exit => SlashOutcome::Exit,
        SlashCommand::Invalid(msg) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&msg)
            );
            SlashOutcome::Continue
        }
    };

    let _ = out.flush();
    outcome
}

// === Handlers ================================================================

fn print_help(out: &mut impl Write) {
    // Grouped by purpose so the user can scan visually instead of skimming
    // a 23-line flat list. The first section header doubles as the title.
    let sections: &[(&str, &[(&str, &str)])] = &[
        (
            "session",
            &[
                (
                    "/clear (cl)",
                    "Wipe in-memory session — saved files untouched",
                ),
                ("/compact", "Force a context compaction now"),
                (
                    "/sessions (ls)",
                    "List saved sessions in ~/.claudette/sessions/",
                ),
                ("/save <name>", "Snapshot the current session under a name"),
                ("/load <name>", "Replace current session with a saved one"),
                (
                    "/sessions delete <name>",
                    "Remove a saved session (alias: rm, remove)",
                ),
                (
                    "/sessions rename <old> <new>",
                    "Rename a saved session (alias: mv)",
                ),
                ("/status (st)", "Turns, tokens, model, context window"),
                ("/cost", "Cumulative token usage for this REPL"),
            ],
        ),
        (
            "tools & memory",
            &[
                ("/tools", "List the secretary's tools"),
                ("/memory (mem)", "Show CLAUDETTE.MD memory in use"),
                ("/reload", "Re-read CLAUDETTE.MD without losing history"),
                (
                    "/validate (val) <path>",
                    "Run Codet code validator on a file",
                ),
                ("/agents", "List available agent types"),
                ("/recall <query>", "Search cross-session memory"),
            ],
        ),
        (
            "models",
            &[
                ("/model", "Show the active brain model"),
                (
                    "/preset <fast|auto|smart>",
                    "Switch brain preset (swap 4b/9b/fallback)",
                ),
                (
                    "/brain <model|auto>",
                    "Pin brain model (or 'auto' to restore preset fallback)",
                ),
                ("/coder <model>", "Pin coder model"),
                ("/models", "Show current model config"),
            ],
        ),
        (
            "brownfield & forge",
            &[
                (
                    "/brownfield <target>",
                    "Clone a repo and make it the active mission",
                ),
                (
                    "/forge <prompt>",
                    "Run prompt in forge-mode against the active mission (auto-PR)",
                ),
            ],
        ),
        (
            "meta",
            &[
                ("/help (h, ?)", "Show this list"),
                ("/capabilities (cap)", "Full configuration dump"),
                ("/exit (quit, q, x)", "Leave the REPL"),
            ],
        ),
    ];

    let _ = writeln!(
        out,
        "{} {}",
        theme::SPARKLES,
        theme::accent("claudette slash commands")
    );
    for (heading, entries) in sections {
        let _ = writeln!(out);
        let _ = writeln!(out, "  {}", theme::accent(heading));
        for (cmd, desc) in *entries {
            let _ = writeln!(out, "    {}  {}", theme::accent(cmd), theme::dim(desc));
        }
    }
}

fn handle_compact<C, T, R>(
    out: &mut impl Write,
    runtime: &mut ConversationRuntime<C, T>,
    rebuild: &R,
) where
    C: ApiClient,
    T: ToolExecutor,
    R: Fn(Session) -> ConversationRuntime<C, T>,
{
    // Force compaction by setting `max_estimated_tokens = 0` so the
    // should-compact gate is satisfied as long as the session has more than
    // `preserve_recent_messages` (=4) entries.
    let result = compact_session(
        runtime.session(),
        CompactionConfig {
            preserve_recent_messages: 4,
            max_estimated_tokens: 0,
        },
    );
    if result.removed_message_count == 0 {
        let _ = writeln!(
            out,
            "{} {}",
            theme::dim("○"),
            theme::dim("nothing to compact (session has 4 or fewer messages)")
        );
        return;
    }
    let removed = result.removed_message_count;
    *runtime = rebuild(result.compacted_session);
    let _ = writeln!(
        out,
        "{} {}",
        theme::SAVE,
        theme::ok(&format!("compacted {removed} message(s) into a summary"))
    );
}

fn handle_sessions(out: &mut impl Write) {
    let dir = sessions_dir();
    let _ = writeln!(
        out,
        "{} {} {}",
        theme::FILE,
        theme::accent("sessions"),
        theme::dim(&dir.display().to_string())
    );
    let entries = list_session_entries(&dir);
    if entries.is_empty() {
        let _ = writeln!(out, "  {}", theme::dim("(none yet)"));
        let _ = writeln!(
            out,
            "  {}",
            theme::dim("save your current conversation with: /save <name>")
        );
        return;
    }
    for entry in entries {
        let _ = writeln!(
            out,
            "  {} {}  {}",
            theme::ok(theme::OK_GLYPH),
            theme::accent(&entry.name),
            theme::dim(&entry.metadata_str())
        );
    }
    let _ = writeln!(
        out,
        "\n  {}",
        theme::dim("delete with: /sessions delete <name>")
    );
}

fn handle_sessions_rename(out: &mut impl Write, old: &str, new: &str) {
    let safe_old = match sanitize_session_name(old) {
        Ok(n) => n,
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("old name: {e}"))
            );
            return;
        }
    };
    let safe_new = match sanitize_session_name(new) {
        Ok(n) => n,
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("new name: {e}"))
            );
            return;
        }
    };
    if safe_old == safe_new {
        let _ = writeln!(
            out,
            "{} {}",
            theme::dim("○"),
            theme::dim("old and new names are the same — nothing to do")
        );
        return;
    }
    let from = sessions_dir().join(format!("{safe_old}.json"));
    let to = sessions_dir().join(format!("{safe_new}.json"));
    if !from.exists() {
        let _ = writeln!(
            out,
            "{} {}",
            theme::error(theme::ERR_GLYPH),
            theme::error(&format!("no session at {}", from.display()))
        );
        return;
    }
    if to.exists() {
        // Refuse to clobber — the user can /sessions delete the target
        // first if they really mean to overwrite. Cheaper than a
        // confirmation prompt in a CLI we want to stay non-interactive.
        let _ = writeln!(
            out,
            "{} {}",
            theme::error(theme::ERR_GLYPH),
            theme::error(&format!(
                "refusing to overwrite existing session at {}",
                to.display()
            ))
        );
        return;
    }
    match std::fs::rename(&from, &to) {
        Ok(()) => {
            let _ = writeln!(
                out,
                "{} {} {}",
                theme::ok(theme::OK_GLYPH),
                theme::ok(&format!("renamed '{safe_old}' → '{safe_new}'")),
                theme::dim(&to.display().to_string())
            );
        }
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("rename failed: {e}"))
            );
        }
    }
}

fn handle_sessions_delete(out: &mut impl Write, name: &str) {
    let safe_name = match sanitize_session_name(name) {
        Ok(n) => n,
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&e)
            );
            return;
        }
    };
    let path = sessions_dir().join(format!("{safe_name}.json"));
    if !path.exists() {
        let _ = writeln!(
            out,
            "{} {}",
            theme::error(theme::ERR_GLYPH),
            theme::error(&format!("no session at {}", path.display()))
        );
        return;
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {
            let _ = writeln!(
                out,
                "{} {} {}",
                theme::ok(theme::OK_GLYPH),
                theme::ok(&format!("deleted session '{safe_name}'")),
                theme::dim(&path.display().to_string())
            );
        }
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("delete failed: {e}"))
            );
        }
    }
}

/// One row in the `/sessions` listing — name plus the file metadata we
/// surface to help the user pick the right session to load or delete.
struct SessionEntry {
    name: String,
    size_bytes: u64,
    modified: Option<std::time::SystemTime>,
}

impl SessionEntry {
    fn metadata_str(&self) -> String {
        let size = format_bytes_short(self.size_bytes);
        let when = self
            .modified
            .and_then(format_relative_age)
            .unwrap_or_else(|| "?".to_string());
        format!("({size}, {when})")
    }
}

fn list_session_entries(dir: &std::path::Path) -> Vec<SessionEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<SessionEntry> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str())? != "json" {
                return None;
            }
            let name = p.file_stem().and_then(|s| s.to_str())?.to_string();
            let meta = e.metadata().ok();
            Some(SessionEntry {
                name,
                size_bytes: meta.as_ref().map_or(0, std::fs::Metadata::len),
                modified: meta.as_ref().and_then(|m| m.modified().ok()),
            })
        })
        .collect();
    // Newest first — by mtime if present, falling back to name asc.
    out.sort_by(|a, b| match (a.modified, b.modified) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.name.cmp(&b.name),
    });
    out
}

/// Human-friendly byte size: 1234 → "1.2 KB", 42_000_000 → "42.0 MB".
fn format_bytes_short(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n < KB {
        format!("{n} B")
    } else if n < MB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else if n < GB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else {
        format!("{:.1} GB", n as f64 / GB as f64)
    }
}

/// "5 seconds ago", "3 minutes ago", "2 hours ago", "4 days ago".
/// Returns None on clock-skew (modified-time in the future or unmeasurable).
fn format_relative_age(when: std::time::SystemTime) -> Option<String> {
    let elapsed = std::time::SystemTime::now().duration_since(when).ok()?;
    let secs = elapsed.as_secs();
    let s = if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 60 * 60 {
        format!("{}m ago", secs / 60)
    } else if secs < 60 * 60 * 24 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    };
    Some(s)
}

fn handle_save<C: ApiClient, T: ToolExecutor>(
    out: &mut impl Write,
    runtime: &ConversationRuntime<C, T>,
    name: &str,
) {
    let safe_name = match sanitize_session_name(name) {
        Ok(n) => n,
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&e)
            );
            return;
        }
    };
    let path = sessions_dir().join(format!("{safe_name}.json"));
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("create_dir_all failed: {e}"))
            );
            return;
        }
    }
    match runtime.session().save_to_path(&path) {
        Ok(()) => {
            let _ = writeln!(
                out,
                "{} {} {}",
                theme::SAVE,
                theme::ok("session saved →"),
                theme::dim(&path.display().to_string())
            );
        }
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("save failed: {e}"))
            );
        }
    }
}

fn handle_load<C, T, R>(
    out: &mut impl Write,
    runtime: &mut ConversationRuntime<C, T>,
    name: &str,
    rebuild: &R,
) where
    C: ApiClient,
    T: ToolExecutor,
    R: Fn(Session) -> ConversationRuntime<C, T>,
{
    let safe_name = match sanitize_session_name(name) {
        Ok(n) => n,
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&e)
            );
            return;
        }
    };
    let path = sessions_dir().join(format!("{safe_name}.json"));
    if !path.exists() {
        let _ = writeln!(
            out,
            "{} {}",
            theme::error(theme::ERR_GLYPH),
            theme::error(&format!("no session at {}", path.display()))
        );
        return;
    }
    match Session::load_from_path(&path) {
        Ok(session) => {
            let count = session.messages.len();
            *runtime = rebuild(session);
            let _ = writeln!(
                out,
                "{} {} {}",
                theme::ok(theme::OK_GLYPH),
                theme::ok(&format!("loaded {safe_name}")),
                theme::dim(&format!("({count} messages)"))
            );
        }
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("load failed: {e}"))
            );
        }
    }
}

fn handle_status<C: ApiClient, T: ToolExecutor>(
    out: &mut impl Write,
    runtime: &ConversationRuntime<C, T>,
    state: &ReplState,
) {
    let session = runtime.session();
    let memory_marker = if try_load_memory().is_some() {
        theme::ok("loaded")
    } else {
        theme::dim("none")
    };

    let _ = writeln!(out, "{} {}", theme::GEAR, theme::accent("status"));
    let _ = writeln!(
        out,
        "  {} model: {}",
        theme::dim("•"),
        theme::ok(&current_model())
    );
    let _ = writeln!(
        out,
        "  {} preset: {}",
        theme::dim("•"),
        theme::ok(&model_config::active().preset.to_string())
    );
    let _ = writeln!(
        out,
        "  {} session messages: {}",
        theme::dim("•"),
        session.messages.len()
    );
    let _ = writeln!(
        out,
        "  {} REPL turns: {}",
        theme::dim("•"),
        state.turn_count
    );
    let _ = writeln!(
        out,
        "  {} cumulative tokens: in={} out={}",
        theme::dim("•"),
        state.cumulative_input_tokens,
        state.cumulative_output_tokens
    );
    let _ = writeln!(out, "  {} num_ctx: {}", theme::dim("•"), current_num_ctx());
    let _ = writeln!(
        out,
        "  {} compaction threshold: {} cumulative input tokens",
        theme::dim("•"),
        compact_threshold()
    );
    let _ = writeln!(out, "  {} memory: {}", theme::dim("•"), memory_marker);
    // Recall status — only show when not in the silent-healthy state so
    // the line doesn't add noise for the typical case where recall is
    // working. Surfaces both the explicit kill-switch and the sticky-
    // disable triggered by a failing embed model.
    let recall_marker = recall_status_marker();
    if let Some(text) = recall_marker {
        let _ = writeln!(out, "  {} recall: {}", theme::dim("•"), text);
    }
    // Brownfield mission marker — shows up only when a mission is active,
    // so users can tell at a glance which working tree their git/bash/file
    // tools are routed to.
    if let Some(mission) = crate::missions::active_mission() {
        let _ = writeln!(
            out,
            "  {} mission: {} {}",
            theme::dim("•"),
            theme::ok(&mission.slug),
            theme::dim(&mission.path.display().to_string())
        );
    }
    let _ = writeln!(
        out,
        "  {} session file: {}",
        theme::dim("•"),
        theme::dim(&default_session_path().display().to_string())
    );
}

/// Build a one-liner describing the recall subsystem's current state, or
/// `None` if recall is fully functional (so the `/status` line stays quiet
/// for the typical case). Surfaces both the env-var kill-switch and the
/// sticky-disable flag set after a failed embed probe.
fn recall_status_marker() -> Option<colored::ColoredString> {
    if crate::run::recall_disabled() {
        return Some(theme::dim("disabled via CLAUDETTE_RECALL_DISABLE"));
    }
    if !crate::run::recall_index_allowed() {
        return Some(theme::warn(
            "disabled this session — embed probe failed at startup",
        ));
    }
    None
}

fn handle_cost<C: ApiClient, T: ToolExecutor>(
    out: &mut impl Write,
    runtime: &ConversationRuntime<C, T>,
    state: &ReplState,
) {
    let usage = runtime.usage().cumulative_usage();
    let avg_in = if state.turn_count > 0 {
        state.cumulative_input_tokens / u64::from(state.turn_count)
    } else {
        0
    };
    let avg_out = if state.turn_count > 0 {
        state.cumulative_output_tokens / u64::from(state.turn_count)
    } else {
        0
    };

    let _ = writeln!(out, "{} {}", theme::BOLT, theme::accent("token usage"));
    let _ = writeln!(
        out,
        "  {} REPL turns: {}",
        theme::dim("•"),
        state.turn_count
    );
    let _ = writeln!(
        out,
        "  {} REPL cumulative — in: {}  out: {}",
        theme::dim("•"),
        state.cumulative_input_tokens,
        state.cumulative_output_tokens
    );
    let _ = writeln!(
        out,
        "  {} REPL average / turn — in: {}  out: {}",
        theme::dim("•"),
        avg_in,
        avg_out
    );
    let _ = writeln!(
        out,
        "  {} runtime cumulative — in: {}  out: {}",
        theme::dim("•"),
        usage.input_tokens,
        usage.output_tokens
    );
    let _ = writeln!(
        out,
        "  {} {}",
        theme::dim("•"),
        theme::dim("(Ollama is free; numbers are for tuning, not billing)")
    );
}

fn handle_tools(out: &mut impl Write) {
    // Sprint 8: show tools grouped into core + optional groups. We can't
    // reach into the live runtime's registry from a slash command (the
    // borrow checker would fight us), so we build a fresh ToolRegistry for
    // display purposes. It uses the same construction logic so the groups
    // it shows are exactly what the live registry has.
    let registry = ToolRegistry::new();

    let _ = writeln!(
        out,
        "{} {} {}",
        theme::SPARKLES,
        theme::accent("secretary tools"),
        theme::dim(&format!(
            "(core {} + {} optional groups)",
            registry.core_tool_names().len(),
            ToolGroup::all().len()
        ))
    );

    let _ = writeln!(out, "  {} core (always loaded)", theme::BOLT);
    describe_tool_group(out, &registry.core_tool_names());

    for group in ToolGroup::all() {
        let names = registry.group_tool_names(group);
        let _ = writeln!(
            out,
            "\n  {} {} {}",
            theme::BOLT,
            theme::accent(group.name()),
            theme::dim(&format!(
                "— {} tool(s), enable with enable_tools({{group: {:?}}})",
                names.len(),
                group.name()
            ))
        );
        describe_tool_group(out, &names);
    }

    let _ = writeln!(
        out,
        "\n  {}",
        theme::dim(&format!(
            "core schema: {} chars — enabling a group grows this temporarily",
            registry.current_schema_chars(),
        ))
    );
}

/// Print a short "• name: description" line for each tool in `names`,
/// looking up the description in the full `secretary_tools_json` registry.
fn describe_tool_group(out: &mut impl Write, names: &[String]) {
    let full = secretary_tools_json();
    let arr: &[serde_json::Value] = full.as_array().map_or(&[], Vec::as_slice);
    for name in names {
        // enable_tools is synthesized by ToolRegistry and doesn't live in
        // secretary_tools_json — give it a hard-coded description.
        let desc_owned;
        let desc: &str = if name == "enable_tools" {
            "Load an optional tool group (git, ide, search, advanced)."
        } else {
            desc_owned = arr
                .iter()
                .find(|t| t.pointer("/function/name").and_then(|v| v.as_str()) == Some(name))
                .and_then(|t| {
                    t.pointer("/function/description")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .unwrap_or_else(|| "(no description)".to_string());
            &desc_owned
        };
        let short = first_sentence(desc, 80);
        let _ = writeln!(
            out,
            "    {} {}: {}",
            theme::ok(theme::OK_GLYPH),
            theme::accent(name),
            theme::dim(&short)
        );
    }
}

fn handle_model(out: &mut impl Write) {
    let _ = writeln!(out, "{} {}", theme::ROBOT, theme::accent("model"));
    let _ = writeln!(
        out,
        "  {} active: {}",
        theme::dim("•"),
        theme::ok(&current_model())
    );
    let _ = writeln!(
        out,
        "  {} {}",
        theme::dim("•"),
        theme::dim("override with: CLAUDETTE_MODEL=<name>")
    );
}

fn handle_memory(out: &mut impl Write) {
    let path = default_memory_path();
    let _ = writeln!(
        out,
        "{} {} {}",
        theme::BRAIN,
        theme::accent("memory"),
        theme::dim(&path.display().to_string())
    );
    match try_load_memory() {
        Some(content) => {
            let len = content.chars().count();
            let _ = writeln!(
                out,
                "  {} {} chars (cap {MAX_MEMORY_CHARS})",
                theme::dim("•"),
                len
            );
            let _ = writeln!(out);
            for line in content.lines() {
                let _ = writeln!(out, "    {line}");
            }
        }
        None => {
            let _ = writeln!(
                out,
                "  {}",
                theme::dim("(no memory file — create one to give the secretary background)")
            );
        }
    }
}

fn handle_reload<C, T, R>(
    out: &mut impl Write,
    runtime: &mut ConversationRuntime<C, T>,
    rebuild: &R,
) where
    C: ApiClient,
    T: ToolExecutor,
    R: Fn(Session) -> ConversationRuntime<C, T>,
{
    let session = runtime.session().clone();
    *runtime = rebuild(session);
    if try_load_memory().is_some() {
        let _ = writeln!(
            out,
            "{} {}",
            theme::BRAIN,
            theme::ok("memory reloaded into the system prompt")
        );
    } else {
        let _ = writeln!(
            out,
            "{} {}",
            theme::BRAIN,
            theme::dim("no memory file found — continuing without")
        );
    }
}

fn handle_agents(out: &mut impl Write) {
    let _ = writeln!(
        out,
        "{} {}",
        theme::ROBOT,
        theme::accent("available agents")
    );
    let agents: &[(&str, &str)] = &[
        (
            "researcher",
            "web search, file reading, code search (max 10 iter)",
        ),
        ("gitops", "git workflows, bash, file reading (max 8 iter)"),
        (
            "reviewer",
            "code review: bugs, security, quality (max 5 iter, read-only)",
        ),
        (
            "codet",
            "code validation sidecar (automatic on write_file, supports py/rs/js/ts)",
        ),
    ];
    for (name, desc) in agents {
        let _ = writeln!(out, "  {}  {}", theme::accent(name), theme::dim(desc));
    }
    let _ = writeln!(
        out,
        "\n  {}",
        theme::dim("Trigger via the spawn_agent tool or ask Claudette to delegate.")
    );
}

fn handle_preset<C, T, R>(
    out: &mut impl Write,
    runtime: &mut ConversationRuntime<C, T>,
    preset: Preset,
    rebuild: &R,
) where
    C: ApiClient,
    T: ToolExecutor,
    R: Fn(Session) -> ConversationRuntime<C, T>,
{
    // Start from a fresh preset-defaults config, then reapply the TOML +
    // env overlays so we don't silently lose per-role customisations the
    // user had in `~/.claudette/models.toml` or env vars.
    let new_cfg = ModelConfig::resolve(preset);
    model_config::set_active(new_cfg);
    rebuild_after_model_swap(runtime, rebuild);
    print_models(out, &model_config::active(), Some(preset));
}

fn handle_brain<C, T, R>(
    out: &mut impl Write,
    runtime: &mut ConversationRuntime<C, T>,
    model: &str,
    rebuild: &R,
) where
    C: ApiClient,
    T: ToolExecutor,
    R: Fn(Session) -> ConversationRuntime<C, T>,
{
    // Special case: `/brain auto` is the inverse of a pin — restores the
    // current preset's fallback policy. Any other value pins the brain
    // and clears the fallback so the next turn doesn't silently swap.
    if model.eq_ignore_ascii_case("auto") {
        let preset = model_config::active().preset;
        let new_cfg = ModelConfig::resolve(preset);
        model_config::set_active(new_cfg);
        rebuild_after_model_swap(runtime, rebuild);
        let _ = writeln!(
            out,
            "{} {}",
            theme::ROBOT,
            theme::ok(&format!(
                "restored preset {}: {} (fallback: {})",
                model_config::active().preset,
                model_config::active().brain.model,
                model_config::active()
                    .fallback_brain
                    .as_ref()
                    .map_or("none", |f| f.model.as_str()),
            ))
        );
        return;
    }

    let cfg = model_config::update_active(|c| {
        c.brain.model = model.to_string();
        c.fallback_brain = None;
    });
    rebuild_after_model_swap(runtime, rebuild);
    let _ = writeln!(
        out,
        "{} {}",
        theme::ROBOT,
        theme::ok(&format!(
            "brain pinned → {} (fallback disabled)",
            cfg.brain.model
        ))
    );
}

fn handle_coder(out: &mut impl Write, model: &str) {
    let cfg = model_config::update_active(|c| c.coder.model = model.to_string());
    // No runtime rebuild — the coder is a sidecar invoked per-call by
    // Codet via `coder_model()`, which re-reads `model_config::active()`
    // on every use.
    let _ = writeln!(
        out,
        "{} {}",
        theme::ROBOT,
        theme::ok(&format!("coder pinned → {}", cfg.coder.model))
    );
}

fn handle_models(out: &mut impl Write) {
    print_models(out, &model_config::active(), None);
}

fn handle_recall(out: &mut impl Write, query: &str) {
    let _ = writeln!(
        out,
        "{} {} {}",
        theme::BRAIN,
        theme::accent("recall"),
        theme::dim(query)
    );
    match crate::recall::global_query(query, 5) {
        Ok(hits) if hits.is_empty() => {
            let _ = writeln!(out, "  {}", theme::dim("(no matches)"));
        }
        Ok(hits) => {
            for hit in hits {
                let role = match hit.role {
                    crate::recall::Role::User => "user",
                    crate::recall::Role::Assistant => "asst",
                };
                let preview = first_sentence(&hit.snippet, 200);
                let _ = writeln!(
                    out,
                    "  {} {} {} {}",
                    theme::ok(theme::OK_GLYPH),
                    theme::dim(&format!("{:.3}", hit.score)),
                    theme::accent(&format!("[{role}] {}", short_ts(&hit.ts))),
                    preview
                );
            }
        }
        Err(e) => {
            let _ = writeln!(
                out,
                "  {} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&e)
            );
        }
    }
}

/// Trim an RFC3339 timestamp to its calendar-date prefix for display.
/// `2026-05-08T14:33:21+00:00` → `2026-05-08`. Falls back to the full
/// string if there's no `T`.
fn short_ts(ts: &str) -> String {
    ts.split_once('T')
        .map_or_else(|| ts.to_string(), |(date, _)| date.to_string())
}

/// Drive a forge-mode turn from inside the REPL. Reuses the public
/// `run_forge_mission` entrypoint so the slash command and the `--forge`
/// CLI flag share the active-mission gate, runtime construction, and
/// streaming behaviour. The forge runtime is built fresh per call (its own
/// session + tool registry); it doesn't share state with the long-lived
/// REPL runtime, so the REPL session stays clean if the brain wanders.
fn handle_forge(out: &mut impl Write, prompt: &str) {
    let _ = writeln!(
        out,
        "{} {} {}",
        theme::ROBOT,
        theme::accent("forge"),
        theme::dim(prompt)
    );
    let opts = crate::SessionOptions {
        resume: false,
        autosave: false,
    };
    match crate::run_forge_mission(prompt, opts) {
        Ok(summary) => {
            let _ = writeln!(
                out,
                "  {} {}",
                theme::BOLT,
                theme::ok(&format!(
                    "forge iter={} in={} out={}",
                    summary.iterations, summary.usage.input_tokens, summary.usage.output_tokens
                ))
            );
        }
        Err(e) => {
            let _ = writeln!(
                out,
                "  {} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("{e:#}"))
            );
        }
    }
}

fn handle_brownfield(out: &mut impl Write, target: &str) {
    let _ = writeln!(
        out,
        "{} {} {}",
        theme::ROBOT,
        theme::accent("brownfield"),
        theme::dim(target)
    );
    let payload = serde_json::json!({ "target": target }).to_string();
    match crate::tools::dispatch_tool("mission_start", &payload) {
        Ok(json) => match serde_json::from_str::<serde_json::Value>(&json) {
            Ok(v) => {
                let slug = v.get("slug").and_then(|x| x.as_str()).unwrap_or("?");
                let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("?");
                let _ = writeln!(
                    out,
                    "  {} {} {}",
                    theme::ok(theme::OK_GLYPH),
                    theme::ok(&format!("mission active: {slug}")),
                    theme::dim(path)
                );
            }
            // mission_start always returns valid JSON on success, so this
            // branch is defensive against future shape drift; print the
            // raw payload so the user sees something useful either way.
            Err(_) => {
                let _ = writeln!(out, "  {} {}", theme::ok(theme::OK_GLYPH), json);
            }
        },
        Err(e) => {
            let _ = writeln!(
                out,
                "  {} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&e)
            );
        }
    }
}

fn print_models(out: &mut impl Write, cfg: &ModelConfig, just_switched: Option<Preset>) {
    let _ = writeln!(out, "{} {}", theme::ROBOT, theme::accent("models"));
    if let Some(p) = just_switched {
        let _ = writeln!(
            out,
            "  {} {}",
            theme::ok(theme::OK_GLYPH),
            theme::ok(&format!("preset switched to {p}"))
        );
    } else {
        let _ = writeln!(
            out,
            "  {} preset: {}",
            theme::dim("•"),
            theme::ok(&cfg.preset.to_string())
        );
    }
    let _ = writeln!(
        out,
        "  {} brain: {} {}",
        theme::dim("•"),
        theme::ok(&cfg.brain.model),
        theme::dim(&format!(
            "(num_ctx={}, num_predict={})",
            cfg.brain.num_ctx, cfg.brain.num_predict
        ))
    );
    match &cfg.fallback_brain {
        Some(fb) => {
            let _ = writeln!(
                out,
                "  {} fallback: {} {}",
                theme::dim("•"),
                theme::ok(&fb.model),
                theme::dim("(used when primary is stuck; reverts after success)")
            );
        }
        None => {
            let _ = writeln!(
                out,
                "  {} fallback: {}",
                theme::dim("•"),
                theme::dim("none (no auto-escalation)")
            );
        }
    }
    let _ = writeln!(
        out,
        "  {} coder: {} {}",
        theme::dim("•"),
        theme::ok(&cfg.coder.model),
        theme::dim(&format!(
            "(num_ctx={}, num_predict={})",
            cfg.coder.num_ctx, cfg.coder.num_predict
        ))
    );
    if let Some(ts) = last_fallback_event() {
        let _ = writeln!(
            out,
            "  {} last fallback: {}",
            theme::dim("•"),
            theme::dim(&ts)
        );
    }
}

/// Read the newest timestamp from `~/.claudette/fallback.jsonl`, if any.
/// Silent on missing/empty file — fallback logging is best-effort.
fn last_fallback_event() -> Option<String> {
    let path = crate::model_config::default_toml_path()
        .parent()?
        .join("fallback.jsonl");
    let content = std::fs::read_to_string(&path).ok()?;
    let last = content.lines().rfind(|l| !l.trim().is_empty())?;
    // Pull the "ts" field without a full JSON parser if possible; fall back
    // to serde_json for anything weirder.
    serde_json::from_str::<serde_json::Value>(last)
        .ok()
        .and_then(|v| v.get("ts").and_then(|t| t.as_str()).map(String::from))
}

/// Rebuild the runtime in place so the next turn uses the updated brain
/// model. Preserves the full message history. Matches the pattern
/// `/clear` / `/reload` / `/load` already use.
fn rebuild_after_model_swap<C, T, R>(runtime: &mut ConversationRuntime<C, T>, rebuild: &R)
where
    C: ApiClient,
    T: ToolExecutor,
    R: Fn(Session) -> ConversationRuntime<C, T>,
{
    let session = runtime.session().clone();
    *runtime = rebuild(session);
}

fn handle_validate(out: &mut impl Write, path_str: &str) {
    let path = std::path::Path::new(path_str);
    // Expand tilde for convenience.
    let resolved = if path_str.starts_with("~/") || path_str.starts_with("~\\") {
        crate::tools::expand_tilde(path_str)
    } else {
        path.to_path_buf()
    };
    if !resolved.exists() {
        let _ = writeln!(
            out,
            "{} {}",
            theme::error(theme::ERR_GLYPH),
            theme::error(&format!("file not found: {}", resolved.display()))
        );
        return;
    }

    let _ = writeln!(
        out,
        "{} {} {}",
        theme::GEAR,
        theme::accent("validating"),
        theme::dim(&resolved.display().to_string())
    );

    match crate::codet::validate_code_file(&resolved, &[]) {
        None => {
            let _ = writeln!(
                out,
                "  {}",
                theme::dim("(not a known code file type — nothing to validate)")
            );
        }
        Some(result) => {
            let _ = writeln!(
                out,
                "  {} syntax: {}",
                theme::dim("•"),
                if result.syntax_ok {
                    theme::ok("ok")
                } else {
                    theme::error("failed")
                }
            );
            if result.tests_found {
                let _ = writeln!(
                    out,
                    "  {} tests: {} passed, {} failed, {} errors",
                    theme::dim("•"),
                    result.tests_passed,
                    result.tests_failed,
                    result.tests_errors
                );
            } else {
                let _ = writeln!(
                    out,
                    "  {} tests: {}",
                    theme::dim("•"),
                    theme::dim("none found")
                );
            }
            if result.fixes_applied > 0 {
                let _ = writeln!(
                    out,
                    "  {} fixes applied: {} — {}",
                    theme::dim("•"),
                    result.fixes_applied,
                    result.fix_summary
                );
            }
            match &result.status {
                crate::codet::CodetStatus::AllPassed => {
                    let _ = writeln!(
                        out,
                        "  {} {}",
                        theme::ok(theme::OK_GLYPH),
                        theme::ok("all checks passed")
                    );
                }
                crate::codet::CodetStatus::FixedAll => {
                    let _ = writeln!(
                        out,
                        "  {} {}",
                        theme::ok(theme::OK_GLYPH),
                        theme::ok("all checks passed (after Codet fixes)")
                    );
                }
                crate::codet::CodetStatus::CouldNotFix { last_error } => {
                    let short: String = last_error.lines().take(3).collect::<Vec<_>>().join(" | ");
                    let _ = writeln!(
                        out,
                        "  {} {}",
                        theme::error(theme::ERR_GLYPH),
                        theme::error(&format!("could not fix: {short}"))
                    );
                }
                crate::codet::CodetStatus::Skipped => {
                    let _ = writeln!(out, "  {}", theme::dim("(validation skipped)"));
                }
            }
        }
    }
}

fn handle_capabilities(out: &mut impl Write) {
    let _ = writeln!(out, "{} {}", theme::GEAR, theme::accent("capabilities"));
    let _ = writeln!(
        out,
        "  {} model: {}",
        theme::dim("•"),
        theme::ok(&current_model())
    );
    let _ = writeln!(out, "  {} num_ctx: {}", theme::dim("•"), current_num_ctx());
    let _ = writeln!(
        out,
        "  {} compact threshold: {}",
        theme::dim("•"),
        compact_threshold()
    );
    let _ = writeln!(
        out,
        "  {} memory file: {}",
        theme::dim("•"),
        theme::dim(&default_memory_path().display().to_string())
    );
    let _ = writeln!(
        out,
        "  {} session file: {}",
        theme::dim("•"),
        theme::dim(&default_session_path().display().to_string())
    );
    let registry = ToolRegistry::new();
    let core_count = registry.core_tool_names().len();
    let optional_count: usize = ToolGroup::all()
        .iter()
        .map(|g| registry.group_tool_names(*g).len())
        .sum();
    let _ = writeln!(
        out,
        "  {} tools: {} core + {} in {} groups (on-demand)",
        theme::dim("•"),
        core_count,
        optional_count,
        ToolGroup::all().len(),
    );
    let _ = writeln!(
        out,
        "  {} version: {}",
        theme::dim("•"),
        env!("CARGO_PKG_VERSION")
    );
}

// === Helpers =================================================================

/// Validate a session name for `/save` and `/load`. Restricts to ASCII
/// alphanumerics, `-`, `_` so we never construct a path that escapes the
/// `sessions/` directory or hits Windows reserved names like `CON`/`PRN`.
fn sanitize_session_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("session name is empty".to_string());
    }
    if n.len() > 64 {
        return Err(format!("session name too long ({} > 64 chars)", n.len()));
    }
    if n.starts_with('.') {
        return Err("session name cannot start with '.'".to_string());
    }
    for c in n.chars() {
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!(
                "session name contains illegal character {c:?} (allowed: alphanumerics, '-', '_')"
            ));
        }
    }
    Ok(n.to_string())
}

/// Take the first sentence of `s` — that is, everything up to the first
/// "period followed by whitespace" (`. `, `.\n`, `.\t`). Falls back to the
/// whole string if there's no sentence-end. Then char-truncates to
/// `max_chars` if still too long. Char-based truncation, never byte
/// slicing, so multibyte glyphs don't blow up the formatter.
///
/// Splitting on `". "` (instead of bare `.`) is the fix for a `/tools` bug
/// where descriptions like `(~/.claudette/files/)` were being cut at the
/// path's first dot, producing output like `Write text content to a file
/// in the secretary's scratch directory (~/`. Real prose ends sentences
/// with a period followed by a space, so this gives us the right answer
/// for both natural text and inline path strings.
fn first_sentence(s: &str, max_chars: usize) -> String {
    let head = sentence_head(s).unwrap_or(s);
    if head.chars().count() <= max_chars {
        return head.to_string();
    }
    head.chars().take(max_chars).collect()
}

/// Find the first sentence-end in `s` and return everything before it.
/// A sentence-end is `.`, `!`, or `?` followed by ASCII whitespace.
/// Returns `None` if there's no sentence boundary in the input.
fn sentence_head(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        let here = bytes[i];
        if matches!(here, b'.' | b'!' | b'?') {
            let next = bytes[i + 1];
            if next.is_ascii_whitespace() {
                return Some(&s[..i]);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_returns_none_for_non_slash_lines() {
        assert!(parse_slash_command("hello").is_none());
        assert!(parse_slash_command("").is_none());
        assert!(parse_slash_command("  ").is_none());
        assert!(parse_slash_command("what time is it?").is_none());
    }

    #[test]
    fn parse_help_aliases() {
        assert_eq!(parse_slash_command("/help"), Some(SlashCommand::Help));
        assert_eq!(parse_slash_command("/h"), Some(SlashCommand::Help));
        assert_eq!(parse_slash_command("/?"), Some(SlashCommand::Help));
    }

    #[test]
    fn parse_clear_aliases() {
        assert_eq!(parse_slash_command("/clear"), Some(SlashCommand::Clear));
        assert_eq!(parse_slash_command("/cl"), Some(SlashCommand::Clear));
    }

    #[test]
    fn parse_simple_commands() {
        assert_eq!(parse_slash_command("/compact"), Some(SlashCommand::Compact));
        assert_eq!(
            parse_slash_command("/sessions"),
            Some(SlashCommand::Sessions)
        );
        assert_eq!(parse_slash_command("/ls"), Some(SlashCommand::Sessions));
        assert_eq!(parse_slash_command("/status"), Some(SlashCommand::Status));
        assert_eq!(parse_slash_command("/st"), Some(SlashCommand::Status));
        assert_eq!(parse_slash_command("/cost"), Some(SlashCommand::Cost));
        assert_eq!(parse_slash_command("/tools"), Some(SlashCommand::Tools));
        assert_eq!(parse_slash_command("/model"), Some(SlashCommand::Model));
        assert_eq!(parse_slash_command("/memory"), Some(SlashCommand::Memory));
        assert_eq!(parse_slash_command("/mem"), Some(SlashCommand::Memory));
        assert_eq!(parse_slash_command("/reload"), Some(SlashCommand::Reload));
        assert_eq!(
            parse_slash_command("/capabilities"),
            Some(SlashCommand::Capabilities)
        );
        assert_eq!(
            parse_slash_command("/cap"),
            Some(SlashCommand::Capabilities)
        );
    }

    #[test]
    fn parse_exit_aliases() {
        for alias in ["/exit", "/quit", "/q", "/x"] {
            assert_eq!(parse_slash_command(alias), Some(SlashCommand::Exit));
        }
    }

    #[test]
    fn parse_save_with_name() {
        assert_eq!(
            parse_slash_command("/save my-work"),
            Some(SlashCommand::Save("my-work".to_string()))
        );
        assert_eq!(
            parse_slash_command("/save   spaces  "),
            Some(SlashCommand::Save("spaces".to_string()))
        );
    }

    #[test]
    fn parse_save_without_name_is_invalid() {
        let parsed = parse_slash_command("/save");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
        let parsed = parse_slash_command("/save   ");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_load_with_name() {
        assert_eq!(
            parse_slash_command("/load deep-research"),
            Some(SlashCommand::Load("deep-research".to_string()))
        );
    }

    #[test]
    fn parse_load_without_name_is_invalid() {
        let parsed = parse_slash_command("/load");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_validate_with_path() {
        assert_eq!(
            parse_slash_command("/validate ~/foo.py"),
            Some(SlashCommand::Validate("~/foo.py".to_string()))
        );
        assert_eq!(
            parse_slash_command("/val test.py"),
            Some(SlashCommand::Validate("test.py".to_string()))
        );
    }

    #[test]
    fn parse_validate_without_path_is_invalid() {
        let parsed = parse_slash_command("/validate");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_agents() {
        assert_eq!(parse_slash_command("/agents"), Some(SlashCommand::Agents));
        assert_eq!(parse_slash_command("/AGENTS"), Some(SlashCommand::Agents));
    }

    #[test]
    fn parse_preset_variants() {
        assert_eq!(
            parse_slash_command("/preset fast"),
            Some(SlashCommand::PresetSwitch(Preset::Fast))
        );
        assert_eq!(
            parse_slash_command("/preset auto"),
            Some(SlashCommand::PresetSwitch(Preset::Auto))
        );
        assert_eq!(
            parse_slash_command("/preset SMART"),
            Some(SlashCommand::PresetSwitch(Preset::Smart))
        );
    }

    #[test]
    fn parse_preset_without_arg_is_invalid() {
        let parsed = parse_slash_command("/preset");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_preset_with_unknown_arg_is_invalid() {
        let parsed = parse_slash_command("/preset balanced");
        match parsed {
            Some(SlashCommand::Invalid(msg)) => assert!(msg.contains("balanced")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn parse_brain_with_model_name() {
        assert_eq!(
            parse_slash_command("/brain qwen3.5:9b"),
            Some(SlashCommand::Brain("qwen3.5:9b".to_string()))
        );
        assert_eq!(
            parse_slash_command("/brain auto"),
            Some(SlashCommand::Brain("auto".to_string()))
        );
    }

    #[test]
    fn parse_brain_without_arg_is_invalid() {
        let parsed = parse_slash_command("/brain");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_coder_with_model_name() {
        assert_eq!(
            parse_slash_command("/coder qwen3-coder:30b"),
            Some(SlashCommand::Coder("qwen3-coder:30b".to_string()))
        );
    }

    #[test]
    fn parse_coder_without_arg_is_invalid() {
        let parsed = parse_slash_command("/coder");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_models_alias() {
        assert_eq!(parse_slash_command("/models"), Some(SlashCommand::Models));
    }

    #[test]
    fn parse_recall_with_query() {
        assert_eq!(
            parse_slash_command("/recall meeting with brian"),
            Some(SlashCommand::Recall("meeting with brian".to_string()))
        );
        assert_eq!(
            parse_slash_command("/recall   trimmed query  "),
            Some(SlashCommand::Recall("trimmed query".to_string()))
        );
    }

    #[test]
    fn parse_recall_without_query_is_invalid() {
        let parsed = parse_slash_command("/recall");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
        let parsed = parse_slash_command("/recall   ");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_brownfield_with_target() {
        assert_eq!(
            parse_slash_command("/brownfield octocat/Hello-World"),
            Some(SlashCommand::Brownfield("octocat/Hello-World".to_string()))
        );
        // Full URL forms pass through untouched — mission_start does the
        // canonicalisation.
        assert_eq!(
            parse_slash_command("/brownfield https://github.com/octocat/Hello-World.git"),
            Some(SlashCommand::Brownfield(
                "https://github.com/octocat/Hello-World.git".to_string()
            ))
        );
        // ssh form preserved verbatim, including the colon.
        assert_eq!(
            parse_slash_command("/brownfield git@github.com:octocat/Hello-World.git"),
            Some(SlashCommand::Brownfield(
                "git@github.com:octocat/Hello-World.git".to_string()
            ))
        );
        // Surrounding whitespace trimmed by the parser, like /save / /recall.
        assert_eq!(
            parse_slash_command("/brownfield   octocat/Hello-World  "),
            Some(SlashCommand::Brownfield("octocat/Hello-World".to_string()))
        );
    }

    #[test]
    fn parse_forge_with_prompt() {
        assert_eq!(
            parse_slash_command("/forge fix the parser bug"),
            Some(SlashCommand::Forge("fix the parser bug".to_string()))
        );
        assert_eq!(
            parse_slash_command("/forge   add a flag --foo  "),
            Some(SlashCommand::Forge("add a flag --foo".to_string()))
        );
    }

    #[test]
    fn parse_forge_without_prompt_is_invalid() {
        let parsed = parse_slash_command("/forge");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
        let parsed = parse_slash_command("/forge   ");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
        if let Some(SlashCommand::Invalid(msg)) = parse_slash_command("/forge") {
            assert!(msg.contains("/forge"), "got: {msg}");
        }
    }

    #[test]
    fn parse_brownfield_without_target_is_invalid() {
        let parsed = parse_slash_command("/brownfield");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
        let parsed = parse_slash_command("/brownfield   ");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
        // Spot-check the error string mentions the command, so the user
        // knows what they typed wrong rather than getting a generic hint.
        if let Some(SlashCommand::Invalid(msg)) = parse_slash_command("/brownfield") {
            assert!(msg.contains("/brownfield"), "got: {msg}");
        }
    }

    #[test]
    fn short_ts_trims_to_date_prefix() {
        assert_eq!(short_ts("2026-05-08T14:33:21+00:00"), "2026-05-08");
        assert_eq!(short_ts("2026-05-08"), "2026-05-08");
    }

    #[test]
    fn parse_unknown_command_is_invalid() {
        let parsed = parse_slash_command("/whatever");
        match parsed {
            Some(SlashCommand::Invalid(msg)) => assert!(msg.contains("/whatever")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn parse_sessions_bare_is_list() {
        assert_eq!(
            parse_slash_command("/sessions"),
            Some(SlashCommand::Sessions)
        );
        assert_eq!(parse_slash_command("/ls"), Some(SlashCommand::Sessions));
        // Trailing whitespace also routes to the list form.
        assert_eq!(
            parse_slash_command("/sessions   "),
            Some(SlashCommand::Sessions)
        );
    }

    #[test]
    fn parse_sessions_delete_with_name() {
        assert_eq!(
            parse_slash_command("/sessions delete my-work"),
            Some(SlashCommand::SessionsDelete("my-work".to_string()))
        );
        // `rm` and `remove` are accepted aliases — match the unix idiom and
        // the user's mental model from `git rm` / `docker rm`.
        assert_eq!(
            parse_slash_command("/sessions rm scratch"),
            Some(SlashCommand::SessionsDelete("scratch".to_string()))
        );
        assert_eq!(
            parse_slash_command("/sessions remove pinned"),
            Some(SlashCommand::SessionsDelete("pinned".to_string()))
        );
    }

    #[test]
    fn parse_sessions_delete_without_name_is_invalid() {
        let parsed = parse_slash_command("/sessions delete");
        match parsed {
            Some(SlashCommand::Invalid(msg)) => {
                assert!(msg.contains("requires a name"), "got: {msg}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn parse_sessions_unknown_subcommand_is_invalid() {
        let parsed = parse_slash_command("/sessions fly");
        match parsed {
            Some(SlashCommand::Invalid(msg)) => {
                assert!(msg.contains("fly"), "got: {msg}");
                assert!(msg.contains("subcommand"), "got: {msg}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn parse_sessions_rename_with_both_names() {
        assert_eq!(
            parse_slash_command("/sessions rename old new"),
            Some(SlashCommand::SessionsRename {
                old: "old".to_string(),
                new: "new".to_string(),
            })
        );
        // `mv` alias from the unix idiom.
        assert_eq!(
            parse_slash_command("/sessions mv scratch saved"),
            Some(SlashCommand::SessionsRename {
                old: "scratch".to_string(),
                new: "saved".to_string(),
            })
        );
        // Extra whitespace inside the args is trimmed.
        assert_eq!(
            parse_slash_command("/sessions rename   alpha    beta"),
            Some(SlashCommand::SessionsRename {
                old: "alpha".to_string(),
                new: "beta".to_string(),
            })
        );
    }

    #[test]
    fn parse_sessions_rename_with_missing_arg_is_invalid() {
        for input in [
            "/sessions rename",
            "/sessions rename solo",
            "/sessions mv just-one",
        ] {
            let parsed = parse_slash_command(input);
            match parsed {
                Some(SlashCommand::Invalid(msg)) => {
                    assert!(
                        msg.contains("two names"),
                        "expected 'two names' hint for {input}, got: {msg}"
                    );
                }
                other => panic!("for {input}, expected Invalid, got {other:?}"),
            }
        }
    }

    #[test]
    fn handle_sessions_rename_rejects_when_source_missing() {
        let mut buf: Vec<u8> = Vec::new();
        handle_sessions_rename(&mut buf, "missing-source-zzz", "missing-dest-zzz");
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("no session at"), "got: {out}");
    }

    #[test]
    fn handle_sessions_rename_noop_when_old_eq_new() {
        let mut buf: Vec<u8> = Vec::new();
        handle_sessions_rename(&mut buf, "same", "same");
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("nothing to do"),
            "expected the same-name short-circuit, got: {out}"
        );
    }

    #[test]
    fn format_bytes_short_covers_each_unit() {
        assert_eq!(format_bytes_short(0), "0 B");
        assert_eq!(format_bytes_short(512), "512 B");
        assert_eq!(format_bytes_short(1024), "1.0 KB");
        assert_eq!(format_bytes_short(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes_short(1024_u64.pow(3)), "1.0 GB");
    }

    #[test]
    fn format_relative_age_buckets_match_user_expectations() {
        // Build a SystemTime in the past for each unit boundary and check
        // the bucket label. None means clock skew (future timestamps); we
        // don't synthesise those here.
        let now = std::time::SystemTime::now();
        let cases = [
            (5_u64, "s ago"),
            (90, "m ago"),     // 1 minute boundary
            (3700, "h ago"),   // 1 hour boundary
            (90_000, "d ago"), // 1 day boundary
        ];
        for (secs, suffix) in cases {
            let when = now - std::time::Duration::from_secs(secs);
            let s = format_relative_age(when).unwrap_or_else(|| panic!("got None for {secs}s"));
            assert!(s.ends_with(suffix), "for {secs}s got: {s}");
        }
        // Future timestamps (clock skew) return None.
        assert!(format_relative_age(now + std::time::Duration::from_secs(60)).is_none());
    }

    #[test]
    fn handle_sessions_delete_rejects_unknown_session() {
        // No file at that name → error message naming the path.
        let mut buf: Vec<u8> = Vec::new();
        handle_sessions_delete(&mut buf, "definitely-does-not-exist-zzz");
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("no session at"), "got: {out}");
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(parse_slash_command("/HELP"), Some(SlashCommand::Help));
        assert_eq!(parse_slash_command("/Status"), Some(SlashCommand::Status));
    }

    #[test]
    fn parse_empty_slash_is_invalid() {
        let parsed = parse_slash_command("/");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn sanitize_accepts_normal_names() {
        assert_eq!(
            sanitize_session_name("my-work_2026"),
            Ok("my-work_2026".to_string())
        );
        assert_eq!(
            sanitize_session_name("  trimmed  "),
            Ok("trimmed".to_string())
        );
    }

    #[test]
    fn sanitize_rejects_path_chars() {
        for bad in [
            "../escape",
            "with/slash",
            "with\\backslash",
            "with:colon",
            "with space",
            "with.dot",
        ] {
            assert!(sanitize_session_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn sanitize_rejects_empty_and_dotted() {
        assert!(sanitize_session_name("").is_err());
        assert!(sanitize_session_name("   ").is_err());
        assert!(sanitize_session_name(".hidden").is_err());
    }

    #[test]
    fn sanitize_rejects_oversized_names() {
        let long = "a".repeat(65);
        assert!(sanitize_session_name(&long).is_err());
    }

    #[test]
    fn first_sentence_takes_up_to_period_space() {
        assert_eq!(first_sentence("hello. world.", 100), "hello");
    }

    #[test]
    fn first_sentence_does_not_split_on_period_inside_path() {
        // Regression for the `/tools` bug: descriptions like "scratch
        // directory (~/.claudette/files/)" were being cut at the dot
        // inside the path. The split must require a sentence boundary
        // (period followed by whitespace), not just any dot.
        let desc = "Write to scratch (~/.claudette/files/) safely. \
                    Sandboxed always.";
        let head = first_sentence(desc, 200);
        assert_eq!(head, "Write to scratch (~/.claudette/files/) safely");
    }

    #[test]
    fn first_sentence_handles_question_and_exclamation() {
        assert_eq!(first_sentence("hi! there", 100), "hi");
        assert_eq!(first_sentence("really? maybe", 100), "really");
    }

    #[test]
    fn first_sentence_falls_back_to_whole_string_when_no_boundary() {
        let s = "no sentence boundary at all";
        assert_eq!(first_sentence(s, 100), s);
    }

    #[test]
    fn first_sentence_falls_back_to_max_chars_for_runaway_text() {
        let s = "no sentence boundary at all";
        assert_eq!(first_sentence(s, 7), "no sent");
    }

    #[test]
    fn first_sentence_handles_multibyte_safely() {
        // 4 robots, no sentence boundary; cap at 3 = "🤖🤖🤖" (3 chars).
        assert_eq!(first_sentence("🤖🤖🤖🤖", 3), "🤖🤖🤖");
    }

    #[test]
    fn repl_state_records_turns() {
        let mut s = ReplState::default();
        s.record_turn(100, 50);
        s.record_turn(200, 80);
        assert_eq!(s.cumulative_input_tokens, 300);
        assert_eq!(s.cumulative_output_tokens, 130);
        assert_eq!(s.turn_count, 2);
    }
}
