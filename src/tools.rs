//! Tool definitions for the secretary, in Ollama's native tool-call schema.
//!
//! Each tool is declared as a JSON object compatible with Ollama's
//! `/api/chat` `tools` parameter. `dispatch_tool` is the sync entry point
//! `SecretaryToolExecutor` calls to actually run them.
//!
//! Storage layout (created on first write):
//!
//! ```text
//! ~/.claudette/
//! ├── notes/
//! │   └── 2026-04-08T11-30-15-call-mom-tomorrow.md
//! ├── files/
//! │   └── (sandboxed scratch dir for write_file)
//! └── todos.json
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::test_runner::run_command_with_timeout;

// Per-group sub-modules. Each exports `schemas()` and `dispatch()`; see the
// group-module contract at the top of `registry.rs`.
mod facts;
mod file_ops;
mod git;
mod github;
mod ide;
mod markets;
mod notes;
mod registry;
mod search;
mod telegram;
mod web_search;

// ────────────────────────────────────────────────────────────────────────────
// Tool registry — advertised to the model on every request
// ────────────────────────────────────────────────────────────────────────────

#[must_use]
pub fn secretary_tools_json() -> Value {
    let mut tools: Vec<Value> = json!([
        // ── Core ────────────────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "get_current_time",
                "description": "Returns the current date, time, weekday, and timezone.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        },
        // Notes group (note_create, note_list, note_read, note_delete)
        // lives in src/tools/notes.rs and is appended to this array below.
        {
            "type": "function",
            "function": {
                "name": "todo_add",
                "description": "Add a task to the todo list.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "Task description" }
                    },
                    "required": ["text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_list",
                "description": "List todos with their status and IDs. By default lists all; pass pending_only to hide completed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pending_only": { "type": "boolean", "description": "If true, hide completed todos (default false)" }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_complete",
                "description": "Mark a todo as done by its ID.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Todo ID from todo_list" }
                    },
                    "required": ["id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_uncomplete",
                "description": "Un-mark a completed todo (set done back to false) by its ID.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Todo ID from todo_list" }
                    },
                    "required": ["id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_delete",
                "description": "Delete a todo by its ID. This is irreversible.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Todo ID from todo_list" }
                    },
                    "required": ["id"]
                }
            }
        },
        // File ops group (read_file, write_file, list_dir) lives in
        // src/tools/file_ops.rs and is appended to this array below.
        {
            "type": "function",
            "function": {
                "name": "get_capabilities",
                "description": "Show the secretary's config, available tools, and limits. Use for 'what can you do' questions.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        },
        // Web-search group (web_search — Brave API) lives in
        // src/tools/web_search.rs and is appended to this array below.
        // Search group (web_fetch, glob_search, grep_search) lives in
        // src/tools/search.rs and is appended to this array below.
        // IDE group (open_in_editor, reveal_in_explorer, open_url) lives
        // in src/tools/ide.rs and is appended to this array below.
        // Git group (git_status, git_diff, git_log, git_add, git_commit,
        // git_branch, git_checkout, git_push) lives in src/tools/git.rs
        // and is appended to this array below.
        // ── Shell + edit ─────────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "bash",
                "description": "Run a shell command. Requires user confirmation. Use for system tasks the other tools can't handle.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to execute" }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace text in an existing file under the user's home. Requires confirmation. For creating new files use write_file or generate_code.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path":     { "type": "string", "description": "File path (absolute or ~/)" },
                        "old_text": { "type": "string", "description": "Exact text to find and replace" },
                        "new_text": { "type": "string", "description": "Replacement text" }
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }
        },
        // ── Code generation ─────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "generate_code",
                "description": "Generate code using the specialized coder model and write it to a file. USE THIS instead of write_file for any code. Supports Python, Rust, JavaScript, TypeScript, HTML, CSS. Auto-validates syntax and tests. The file is written to disk; reply with a SHORT confirmation (path + 1 sentence). DO NOT paste the generated code in your reply — it bloats the conversation and the user can already open the file. BROWNFIELD: when the user mentions an existing file the new code must match (e.g. 'add tests for X.py', 'extend X.py', 'refactor X.py'), ALWAYS list those file paths in `reference_files` so the coder can read the real API instead of inventing one.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "description":     { "type": "string", "description": "What code to write — include language, functions, tests needed" },
                        "filename":        { "type": "string", "description": "Filename (e.g. 'calc.py', 'lib.rs', 'app.ts'). Extension sets the language." },
                        "reference_files": { "type": "array", "items": { "type": "string" }, "description": "Existing file paths the coder MUST read before writing (real class/method names, signatures, exceptions). Pass each path as the user typed it — '~/.claudette/files/X.py', './X.py', or 'X.py'. Up to 4 files; oversize files are auto-truncated." }
                    },
                    "required": ["description", "filename"]
                }
            }
        },
        // ── Agent delegation ────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "spawn_agent",
                "description": "Delegate a task to a specialized agent. 'researcher' for web/file/code research, 'gitops' for git workflows, 'reviewer' for code review.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "agent_type": { "type": "string", "enum": ["researcher", "gitops", "reviewer"], "description": "Agent type" },
                        "task":       { "type": "string", "description": "Task description for the agent" },
                        "auto":       { "type": "boolean", "description": "Skip confirmation prompts for dangerous tools (default false)" }
                    },
                    "required": ["agent_type", "task"]
                }
            }
        },
        // Facts group (wikipedia_search, wikipedia_summary, weather_current,
        // weather_forecast) lives in src/tools/facts.rs and is appended below.
        // Registry group (crate_info, crate_search, npm_info, npm_search)
        // lives in src/tools/registry.rs and is appended to this array below.
        // GitHub group (gh_list_my_prs, gh_list_assigned_issues, gh_get_issue,
        // gh_create_issue, gh_comment_issue, gh_search_code) lives in
        // src/tools/github.rs and is appended to this array below.
        // Markets group (tv_get_quote, tv_technical_rating, tv_search_symbol,
        // tv_economic_calendar, vestige_asa_info, vestige_search_asa,
        // vestige_top_movers) lives in src/tools/markets.rs and is appended
        // to this array below.
        // Telegram group (tg_send, tg_get_updates, tg_send_photo) lives in
        // src/tools/telegram.rs and is appended to this array below.
    ])
    .as_array()
    .cloned()
    .unwrap_or_default();
    tools.extend(facts::schemas());
    tools.extend(file_ops::schemas());
    tools.extend(git::schemas());
    tools.extend(github::schemas());
    tools.extend(ide::schemas());
    tools.extend(markets::schemas());
    tools.extend(notes::schemas());
    tools.extend(registry::schemas());
    tools.extend(search::schemas());
    tools.extend(telegram::schemas());
    tools.extend(web_search::schemas());
    Value::Array(tools)
}

// ────────────────────────────────────────────────────────────────────────────
// Dispatcher — entry point called by SecretaryToolExecutor
// ────────────────────────────────────────────────────────────────────────────

pub fn dispatch_tool(name: &str, input: &str) -> Result<String, String> {
    // Per-group dispatchers get first crack; each returns Some(_) if it owns
    // the tool, None otherwise. The `match` below handles everything that
    // hasn't migrated to a sub-module yet.
    if let Some(result) = facts::dispatch(name, input) {
        return result;
    }
    if let Some(result) = file_ops::dispatch(name, input) {
        return result;
    }
    if let Some(result) = git::dispatch(name, input) {
        return result;
    }
    if let Some(result) = github::dispatch(name, input) {
        return result;
    }
    if let Some(result) = ide::dispatch(name, input) {
        return result;
    }
    if let Some(result) = markets::dispatch(name, input) {
        return result;
    }
    if let Some(result) = notes::dispatch(name, input) {
        return result;
    }
    if let Some(result) = registry::dispatch(name, input) {
        return result;
    }
    if let Some(result) = search::dispatch(name, input) {
        return result;
    }
    if let Some(result) = telegram::dispatch(name, input) {
        return result;
    }
    if let Some(result) = web_search::dispatch(name, input) {
        return result;
    }

    match name {
        "get_current_time" => Ok(run_get_current_time()),
        // add_numbers removed from registry (model can do arithmetic).
        // Dispatch kept so old sessions with tool_calls still work.
        "add_numbers" => run_add_numbers(input),
        // Notes group (note_*) handled by the early-return above via
        // notes::dispatch.
        "todo_add" => run_todo_add(input),
        "todo_list" => run_todo_list(input),
        "todo_complete" => run_todo_complete(input),
        "todo_uncomplete" => run_todo_uncomplete(input),
        "todo_delete" => run_todo_delete(input),
        // File ops group (read_file, write_file, list_dir) handled by
        // the early-return above via file_ops::dispatch.
        "get_capabilities" => Ok(run_get_capabilities()),
        // Web-search group (web_search) handled by the early-return above
        // via web_search::dispatch.
        // Search group (glob_search, grep_search, web_fetch) is handled by
        // the early-return above via search::dispatch.
        // Git group (git_*) handled by the early-return above via
        // git::dispatch.
        "bash" => run_bash(input),
        "edit_file" => run_edit_file(input),
        "generate_code" => run_generate_code(input),
        "spawn_agent" => run_spawn_agent(input),
        // Facts group (wikipedia_*, weather_*) handled by the early-return
        // above via facts::dispatch.
        // Registry group (crate_info, crate_search, npm_info, npm_search)
        // is handled by the early-return above via registry::dispatch.
        // GitHub group (gh_*) handled by the early-return above via
        // github::dispatch.
        // Markets group (tv_*, vestige_*) handled by the early-return
        // above via markets::dispatch.
        // Telegram group (tg_*) handled by the early-return above via
        // telegram::dispatch.
        other => Err(format!("unknown tool: {other}")),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Time & math (the original two)
// ────────────────────────────────────────────────────────────────────────────

fn run_get_current_time() -> String {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    // Monday = 1 .. Sunday = 7 (ISO 8601).
    let dow = now.weekday().number_from_monday();
    // "Tuesday, April 14, 2026 at 3:42 PM" — a natural string the model can
    // drop into a response without re-formatting.
    let human = {
        let hour12 = match now.hour() % 12 {
            0 => 12,
            h => h,
        };
        let ampm = if now.hour() < 12 { "AM" } else { "PM" };
        format!(
            "{}, {} {}, {} at {}:{:02} {}",
            now.format("%A"),
            now.format("%B"),
            now.day(),
            now.year(),
            hour12,
            now.minute(),
            ampm,
        )
    };
    json!({
        "iso8601": now.to_rfc3339(),
        "weekday": now.format("%A").to_string(),
        "weekday_num": dow,
        "date": now.format("%Y-%m-%d").to_string(),
        "time": now.format("%H:%M:%S").to_string(),
        "timezone": now.format("%:z").to_string(),
        "unix_timestamp": now.timestamp(),
        "human": human,
    })
    .to_string()
}

fn run_add_numbers(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("add_numbers: invalid JSON input ({e}): {input}"))?;
    let a = v
        .get("a")
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("add_numbers: missing or non-numeric 'a' in {input}"))?;
    let b = v
        .get("b")
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("add_numbers: missing or non-numeric 'b' in {input}"))?;
    Ok(json!({ "a": a, "b": b, "sum": a + b }).to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Storage helpers — ~/.claudette layout
// ────────────────────────────────────────────────────────────────────────────

pub(super) fn user_home() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
}

pub(super) fn claudette_home() -> PathBuf {
    user_home().join(".claudette")
}

fn todos_path() -> PathBuf {
    claudette_home().join("todos.json")
}

/// Scratch directory the secretary is allowed to write into.
/// Sits next to notes/ and todos.json so it's clearly within the
/// claudette data home and easy for the user to inspect or wipe.
pub(super) fn files_dir() -> PathBuf {
    claudette_home().join("files")
}

pub(super) fn ensure_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("create dir {}: {e}", path.display()))
}

// Notes group (note_*) + the slugify helper live in src/tools/notes.rs.

// ────────────────────────────────────────────────────────────────────────────
// Todos (single todos.json file)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
struct Todo {
    id: String,
    text: String,
    done: bool,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<String>,
}

fn load_todos() -> Result<Vec<Todo>, String> {
    let path = todos_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let s = fs::read_to_string(&path).map_err(|e| format!("read todos: {e}"))?;
    if s.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&s).map_err(|e| format!("parse todos.json: {e}"))
}

fn save_todos(todos: &[Todo]) -> Result<(), String> {
    ensure_dir(&claudette_home())?;
    let s = serde_json::to_string_pretty(todos).map_err(|e| format!("serialize todos: {e}"))?;
    fs::write(todos_path(), s).map_err(|e| format!("write todos: {e}"))
}

fn run_todo_add(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("todo_add: invalid JSON ({e}): {input}"))?;
    // Prefer "text"; accept "content" as a fallback for older prompts.
    let text = v
        .get("text")
        .or_else(|| v.get("content"))
        .and_then(Value::as_str)
        .ok_or("todo_add: missing 'text'")?
        .trim()
        .to_string();
    if text.is_empty() {
        return Err("todo_add: 'text' cannot be empty".to_string());
    }

    let mut todos = load_todos()?;
    let now = chrono::Local::now();
    let id = format!("t_{}", now.timestamp_millis());
    todos.push(Todo {
        id: id.clone(),
        text: text.clone(),
        done: false,
        created_at: now.to_rfc3339(),
        completed_at: None,
    });
    save_todos(&todos)?;

    Ok(json!({ "ok": true, "id": id, "text": text }).to_string())
}

fn run_todo_list(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input).unwrap_or(json!({}));
    let pending_only = v
        .get("pending_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let todos = load_todos()?;
    let total = todos.len();
    let pending = todos.iter().filter(|t| !t.done).count();
    let view: Vec<Value> = todos
        .iter()
        .enumerate()
        .filter(|(_, t)| !pending_only || !t.done)
        .map(|(i, t)| {
            let mut obj = json!({
                "index": i + 1,
                "id": t.id,
                "text": t.text,
                "done": t.done,
                "created_at": t.created_at,
            });
            if let Some(ref c) = t.completed_at {
                obj["completed_at"] = json!(c);
            }
            obj
        })
        .collect();
    let mut result = json!({
        "count": view.len(),
        "total": total,
        "pending": pending,
        "todos": view,
    });
    if pending_only {
        result["pending_only"] = json!(true);
    }
    Ok(result.to_string())
}

fn run_todo_complete(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("todo_complete: invalid JSON ({e}): {input}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("todo_complete: missing 'id'")?
        .to_string();

    let mut todos = load_todos()?;
    let mut updated = None;
    for t in &mut todos {
        if t.id == id {
            t.done = true;
            t.completed_at = Some(chrono::Local::now().to_rfc3339());
            updated = Some(t.text.clone());
            break;
        }
    }
    let text = updated.ok_or_else(|| format!("todo_complete: no todo with id '{id}'"))?;
    save_todos(&todos)?;

    Ok(json!({ "ok": true, "id": id, "text": text, "done": true }).to_string())
}

fn run_todo_uncomplete(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("todo_uncomplete: invalid JSON ({e}): {input}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("todo_uncomplete: missing 'id'")?
        .to_string();

    let mut todos = load_todos()?;
    let mut updated = None;
    for t in &mut todos {
        if t.id == id {
            t.done = false;
            t.completed_at = None;
            updated = Some(t.text.clone());
            break;
        }
    }
    let text = updated.ok_or_else(|| format!("todo_uncomplete: no todo with id '{id}'"))?;
    save_todos(&todos)?;

    Ok(json!({ "ok": true, "id": id, "text": text, "done": false }).to_string())
}

fn run_todo_delete(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("todo_delete: invalid JSON ({e}): {input}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("todo_delete: missing 'id'")?
        .to_string();

    let mut todos = load_todos()?;
    let before = todos.len();
    let removed_text = todos.iter().find(|t| t.id == id).map(|t| t.text.clone());
    todos.retain(|t| t.id != id);
    if todos.len() == before {
        return Err(format!("todo_delete: no todo with id '{id}'"));
    }
    save_todos(&todos)?;

    Ok(json!({
        "ok": true,
        "id": id,
        "text": removed_text.unwrap_or_default(),
        "deleted": true,
    })
    .to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Self-introspection (get_capabilities)
//
// Without this tool, the model has no way to answer "what can you do" or
// "how much memory do you have" except by guessing — and we measured it
// guessing wrong (claiming "no fixed context window" when in fact num_ctx
// is 4096 with a sliding-window truncator). Returning real values from a
// tool fixes self-description without bloating the system prompt (which
// the README explicitly warns suppresses tool calling on qwen3.5:9b).
// ────────────────────────────────────────────────────────────────────────────

fn run_get_capabilities() -> String {
    // Sprint 8: report tools as core + optional groups. We build a fresh
    // ToolRegistry for inspection; the live runtime's registry may have
    // additional groups enabled, but this snapshot is still the right
    // answer for "what could you do if you needed to", which is what the
    // model is really asking.
    let registry = crate::tool_groups::ToolRegistry::new();
    let core_names = registry.core_tool_names();
    let groups_summary: Vec<Value> = crate::tool_groups::ToolGroup::all()
        .iter()
        .map(|g| {
            json!({
                "name": g.name(),
                "summary": g.summary(),
                "tools": registry.group_tool_names(*g),
            })
        })
        .collect();
    let total_tools = core_names.len()
        + crate::tool_groups::ToolGroup::all()
            .iter()
            .map(|g| registry.group_tool_names(*g).len())
            .sum::<usize>();

    json!({
        "name": "Claudette",
        "kind": "personal AI secretary",
        "model": crate::run::current_model(),
        "runtime": "crate::ConversationRuntime over Ollama /api/chat",
        "context_window": {
            "num_ctx_tokens": crate::api::current_num_ctx(),
            "num_predict_tokens": crate::api::current_num_predict(),
            "auto_compaction_threshold_tokens": crate::run::compact_threshold(),
            "notes": "Auto-compaction summarises old turns when cumulative input tokens cross the threshold; the most recent turns stay verbatim. A char-based sliding-window truncator inside api.rs is the in-iteration safety net.",
        },
        "tools": {
            "total": total_tools,
            "core": core_names,
            "optional_groups": groups_summary,
            "note": "Optional group tools are only advertised after you call enable_tools(group) — they cut the per-turn schema cost when unused.",
        },
        "sandbox": {
            "read": "user $HOME (/home/<user> or C:\\Users\\<user>) — symlinks/junctions resolved as such, system dirs not blocked but ACL-protected anyway",
            "write": files_dir().display().to_string(),
            "rationale": "writes are sandboxed to ~/.claudette/files/ so the secretary cannot overwrite the user's real documents by accident or hallucination",
        },
        "storage": {
            "notes": notes::notes_dir().display().to_string(),
            "todos": todos_path().display().to_string(),
            "scratch_files": files_dir().display().to_string(),
            "session": crate::run::default_session_path().display().to_string(),
        },
        "version": env!("CARGO_PKG_VERSION"),
    })
    .to_string()
}

// Web-search group (web_search — Brave API) lives in
// src/tools/web_search.rs.

// ────────────────────────────────────────────────────────────────────────────
// File ops (read_file, write_file, list_dir)
//
// Sandboxing policy (intentional, narrow by default — loosen on demand):
//   • read_file / list_dir: allowed anywhere under the user's $HOME.
//     Lets the secretary research and summarize the user's own documents
//     without exposing system files (/etc, C:\Windows, etc).
//   • write_file: allowed ONLY under ~/.claudette/files/. The secretary
//     gets its own scratch space; it can't overwrite anything important
//     by accident or hallucination. Users who want a draft moved to e.g.
//     Documents can copy it themselves.
//
// Path normalization is manual (no canonicalize) so it works for paths
// that don't yet exist (write_file targets) and avoids Windows UNC noise
// (\\?\C:\...). This does NOT defend against symlink escape — acceptable
// threat model for a local secretary running on the user's own machine.
// ────────────────────────────────────────────────────────────────────────────

pub(super) const MAX_FILE_BYTES: usize = 100 * 1024; // 100 KB

/// Expand a leading `~` to the user's home directory. Other tildes are left
/// alone (matching shell behaviour). `pub(crate)` so the `/validate` slash
/// command can reuse the same tilde logic as the file-ops tools.
pub(crate) fn expand_tilde(input: &str) -> PathBuf {
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        user_home().join(rest)
    } else if input == "~" {
        user_home()
    } else {
        PathBuf::from(input)
    }
}

/// Resolve `.` and `..` components without touching the filesystem.
/// Absolute paths stay absolute; relative paths stay relative (joined to
/// CWD by the caller if needed). Leading `..` on a relative path is kept.
pub(super) fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                // Pop only if the last component is a real directory name.
                // Don't pop a Prefix, RootDir, or another ParentDir.
                let popped =
                    matches!(out.components().next_back(), Some(Component::Normal(_))) && out.pop();
                if !popped {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Normalize an input path string, expanding `~` and resolving `.`/`..`.
/// Relative paths are made absolute by joining to the current working dir.
fn resolve_input_path(input: &str) -> Result<PathBuf, String> {
    let expanded = expand_tilde(input);
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        let cwd = std::env::current_dir().map_err(|e| format!("get cwd: {e}"))?;
        cwd.join(expanded)
    };
    Ok(normalize_path(&absolute))
}

// ────────────────────────────────────────────────────────────────────────────
// Reference-file extraction (brownfield API-matching — Sprint 13)
//
// When the user asks `generate_code` to write tests/code that references an
// existing file, extract every path-like token from the description, resolve
// each under $HOME or the scratch dir, and return the file contents. The
// collector is intentionally conservative: it will only surface files that
// (a) syntactically look like a path, and (b) actually exist on disk.
// ────────────────────────────────────────────────────────────────────────────

/// File extensions we'll include as reference context.
const REF_EXTENSIONS: &[&str] = &[
    "py", "rs", "js", "mjs", "cjs", "jsx", "ts", "tsx", "html", "htm", "css", "json", "toml",
    "yaml", "yml", "md", "txt", "sh", "bash", "go", "java", "c", "cpp", "cc", "cxx", "h", "hpp",
    "rb", "php", "sql", "xml", "ini", "cfg", "conf",
];

/// Max files, per-file byte cap, and total byte cap. Keeps the coder prompt
/// below ~70 KB even when the user references several modules.
const REF_MAX_FILES: usize = 4;
const REF_MAX_BYTES_PER_FILE: usize = 16 * 1024;
const REF_MAX_BYTES_TOTAL: usize = 64 * 1024;

// `is_code_extension` + CODE_EXTENSIONS moved with write_file into
// src/tools/file_ops.rs.

// ────────────────────────────────────────────────────────────────────────────
// Per-turn user-prompt path stash (Sprint 13.2 — bypass-the-brain brownfield)
//
// The brain summarises the user prompt before constructing tool calls and
// regularly drops file paths. Even with the explicit `reference_files` schema
// param, the 4b brain rarely populates it. Solution: extract paths from the
// raw user prompt at the entry point (REPL / single-shot / Telegram / TUI),
// stash them here, and merge in `collect_reference_files`. Bypasses the brain
// entirely. Each entry point overwrites the stash before submitting the turn.
// ────────────────────────────────────────────────────────────────────────────

use std::sync::{Mutex, OnceLock};

static CURRENT_TURN_PATHS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn current_turn_paths_mu() -> &'static Mutex<Vec<String>> {
    CURRENT_TURN_PATHS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Replace the per-turn path list. Called from each entry point with the paths
/// extracted from the raw user prompt. An empty Vec clears the stash, which is
/// the right thing to do for non-brownfield prompts (no leakage between turns).
pub fn set_current_turn_paths(paths: Vec<String>) {
    if let Ok(mut g) = current_turn_paths_mu().lock() {
        *g = paths;
    }
}

/// Read the current stash. Returns an empty Vec if poisoned (defensive — we'd
/// rather degrade to "no refs" than panic the agent loop).
pub(crate) fn current_turn_paths() -> Vec<String> {
    current_turn_paths_mu()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// Scan the raw user prompt for path tokens and keep only those that resolve
/// to an existing file under the read policy. Used by entry points to populate
/// the per-turn stash.
#[must_use]
pub fn extract_user_prompt_paths(prompt: &str) -> Vec<String> {
    extract_path_candidates(prompt)
        .into_iter()
        .filter(|t| resolve_reference(t).is_some())
        .collect()
}

/// Collect reference files for the coder prompt. Three sources, in priority order:
///   1. **Per-turn stash** — paths the system pre-extracted from the raw user
///      prompt (Sprint 13.2). Most reliable: bypasses the brain entirely.
///   2. `explicit` — paths the brain passed via the `reference_files` tool param.
///      Useful when the brain follows the schema instruction.
///   3. `description` — fallback path-scan for when the brain forgets BOTH the
///      param AND the path didn't make it into the user message verbatim.
///
/// All three go through the same `validate_read_path` policy and size caps,
/// and dedup by absolute path so a path hit on multiple sources only loads once.
pub(crate) fn collect_reference_files(
    explicit: &[&str],
    description: &str,
) -> Vec<crate::codet::ReferenceFile> {
    let mut out: Vec<crate::codet::ReferenceFile> = Vec::new();
    let mut seen_abs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut total_bytes: usize = 0;

    let stash_iter = current_turn_paths().into_iter();
    let explicit_iter = explicit.iter().map(|s| (*s).to_string());
    let scanner_iter = extract_path_candidates(description).into_iter();
    for token in stash_iter.chain(explicit_iter).chain(scanner_iter) {
        if out.len() >= REF_MAX_FILES {
            break;
        }
        let Some(resolved) = resolve_reference(&token) else {
            continue;
        };
        if !seen_abs.insert(resolved.clone()) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&resolved) else {
            continue;
        };
        let trimmed = truncate_content(content);
        if total_bytes.saturating_add(trimmed.len()) > REF_MAX_BYTES_TOTAL {
            break;
        }
        total_bytes += trimmed.len();
        out.push(crate::codet::ReferenceFile {
            path: token,
            content: trimmed,
        });
    }
    out
}

fn truncate_content(mut content: String) -> String {
    if content.len() > REF_MAX_BYTES_PER_FILE {
        // Truncate at a char boundary, then annotate.
        let mut cut = REF_MAX_BYTES_PER_FILE;
        while cut > 0 && !content.is_char_boundary(cut) {
            cut -= 1;
        }
        content.truncate(cut);
        content.push_str("\n... [truncated — file continues]\n");
    }
    content
}

/// Break a free-form description into path-shaped candidate tokens, stripping
/// surrounding quotes/brackets/trailing punctuation. Does NOT check the
/// filesystem — `resolve_reference` does that.
fn extract_path_candidates(text: &str) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    let mut buf = String::new();
    for c in text.chars() {
        if c.is_whitespace()
            || matches!(
                c,
                ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '`' | '<' | '>'
            )
        {
            if !buf.is_empty() {
                raw.push(std::mem::take(&mut buf));
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        raw.push(buf);
    }

    raw.into_iter()
        .filter_map(|t| {
            // Strip trailing sentence punctuation (em-dash, en-dash, etc).
            let trimmed = t
                .trim_end_matches(|c: char| {
                    matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '—' | '–' | ')')
                })
                .to_string();
            if trimmed.is_empty() {
                return None;
            }
            // URLs look like paths-with-extensions but aren't reachable via
            // the filesystem — drop them before they trip resolve_reference.
            if trimmed.contains("://") {
                return None;
            }
            if looks_like_path(&trimmed) || has_code_extension(&trimmed) {
                Some(trimmed)
            } else {
                None
            }
        })
        .collect()
}

/// `true` iff the token uses explicit path syntax (tilde, absolute, dotted
/// relative, or a Windows drive letter). URLs are excluded.
fn looks_like_path(s: &str) -> bool {
    if s.contains("://") {
        return false;
    }
    if s.starts_with("~/") || s.starts_with("~\\") {
        return true;
    }
    if s.starts_with("./") || s.starts_with(".\\") || s.starts_with("../") || s.starts_with("..\\")
    {
        return true;
    }
    if s.starts_with('/') || s.starts_with('\\') {
        return true;
    }
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn has_code_extension(s: &str) -> bool {
    Path::new(s)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            let lower = e.to_ascii_lowercase();
            REF_EXTENSIONS.contains(&lower.as_str())
        })
}

/// Resolve a token to an absolute path on disk, or `None` if no readable
/// file exists under $HOME, the scratch dir, or the current working dir.
fn resolve_reference(token: &str) -> Option<PathBuf> {
    // Explicit path: use the same read-policy as read_file.
    if looks_like_path(token) {
        return validate_read_path(token).ok().filter(|p| p.is_file());
    }
    // Bare filename with a code extension: try scratch dir then cwd.
    if !has_code_extension(token) {
        return None;
    }
    for dir in [
        files_dir(),
        std::env::current_dir().unwrap_or_else(|_| files_dir()),
    ] {
        let candidate = dir.join(token);
        if candidate.is_file() {
            let as_string = candidate.to_string_lossy().to_string();
            if let Ok(validated) = validate_read_path(&as_string) {
                return Some(validated);
            }
        }
    }
    None
}

/// Validate a read/list path: must resolve under the user's home directory
/// OR the current working directory (so the researcher agent can access
/// project files outside $HOME).
pub(super) fn validate_read_path(input: &str) -> Result<PathBuf, String> {
    let resolved = resolve_input_path(input)?;
    let home = normalize_path(&user_home());
    if resolved.starts_with(&home) {
        return Ok(resolved);
    }
    // Also allow paths under the current working directory (project root).
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_norm = normalize_path(&cwd);
        if resolved.starts_with(&cwd_norm) {
            return Ok(resolved);
        }
    }
    Err(format!(
        "path is outside both $HOME ({}) and the working directory; reads are restricted for safety",
        home.display()
    ))
}

/// Validate a write path: must resolve under `~/.claudette/files/`.
pub(super) fn validate_write_path(input: &str) -> Result<PathBuf, String> {
    let resolved = resolve_input_path(input)?;
    let scratch = normalize_path(&files_dir());
    if !resolved.starts_with(&scratch) {
        return Err(format!(
            "writes are sandboxed to {}. Use a path under that directory.",
            scratch.display()
        ));
    }
    Ok(resolved)
}

// File ops group (read_file, write_file, list_dir) lives in
// src/tools/file_ops.rs.

// Git group (git_status, git_diff, git_log, git_add, git_commit,
// git_branch, git_checkout, git_push) lives in src/tools/git.rs.

// ────────────────────────────────────────────────────────────────────────────
// Shell + edit — requires DangerFullAccess (user confirmation via CLI prompt)
// ────────────────────────────────────────────────────────────────────────────

const BASH_OUTPUT_MAX_CHARS: usize = 8192;

fn run_bash(input: &str) -> Result<String, String> {
    let v: Value =
        serde_json::from_str(input).map_err(|e| format!("bash: invalid JSON ({e}): {input}"))?;
    let command = v
        .get("command")
        .and_then(Value::as_str)
        .ok_or("bash: missing 'command'")?;

    if command.trim().is_empty() {
        return Err("bash: command is empty".to_string());
    }

    // Execute via the platform shell so pipes, redirects, and builtins work.
    #[cfg(target_os = "windows")]
    let (program, args) = ("cmd", vec!["/C", command]);
    #[cfg(not(target_os = "windows"))]
    let (program, args) = ("sh", vec!["-c", command]);

    let result = run_command_with_timeout(program, &args, 30, None);

    let stdout: String = result.stdout.chars().take(BASH_OUTPUT_MAX_CHARS).collect();
    let stderr: String = result.stderr.chars().take(BASH_OUTPUT_MAX_CHARS).collect();
    let truncated =
        result.stdout.len() > BASH_OUTPUT_MAX_CHARS || result.stderr.len() > BASH_OUTPUT_MAX_CHARS;

    Ok(json!({
        "exit_code": result.exit_code,
        "stdout": stdout,
        "stderr": stderr,
        "timed_out": result.timed_out,
        "truncated": truncated,
    })
    .to_string())
}

fn run_edit_file(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("edit_file: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("edit_file: missing 'path'")?;
    let old_text = v
        .get("old_text")
        .and_then(Value::as_str)
        .ok_or("edit_file: missing 'old_text'")?;
    let new_text = v
        .get("new_text")
        .and_then(Value::as_str)
        .ok_or("edit_file: missing 'new_text'")?;

    // $HOME-gated (broader than write_file's sandbox) because the user
    // explicitly confirmed via the permission prompt.
    let path = validate_read_path(path_str)?;

    let content = fs::read_to_string(&path)
        .map_err(|e| format!("edit_file: read {} failed: {e}", path.display()))?;

    if !content.contains(old_text) {
        return Err(format!(
            "edit_file: old_text not found in {}. The text to replace must match exactly.",
            path.display()
        ));
    }

    let new_content = content.replacen(old_text, new_text, 1);
    fs::write(&path, &new_content)
        .map_err(|e| format!("edit_file: write {} failed: {e}", path.display()))?;

    let mut result = json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": new_content.len(),
    });

    // Codet post-edit hook for code files (same as write_file).
    if let Some(validation) = crate::codet::validate_code_file(&path, &[]) {
        result["validation"] = validation.to_json();
        if let crate::codet::CodetStatus::CouldNotFix { ref last_error } = validation.status {
            let short_err: String = last_error.lines().take(3).collect::<Vec<_>>().join(" | ");
            eprintln!(
                "{} {}",
                crate::theme::warn(crate::theme::WARN_GLYPH),
                crate::theme::warn(&format!(
                    "codet: {} failed validation after {} attempt(s), {} landed — {}",
                    path.display(),
                    validation.attempts_made,
                    validation.fixes_applied,
                    short_err,
                ))
            );
        }
    }

    Ok(result.to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Code generation — delegates to the coder model via Codet
// ────────────────────────────────────────────────────────────────────────────

fn run_generate_code(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("generate_code: invalid JSON ({e}): {input}"))?;
    let description = v
        .get("description")
        .and_then(Value::as_str)
        .ok_or("generate_code: missing 'description'")?;
    let filename = v
        .get("filename")
        .and_then(Value::as_str)
        .ok_or("generate_code: missing 'filename'")?;

    // Infer language from extension.
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("text");
    let language = match ext {
        "py" => "Python",
        "rs" => "Rust",
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "php" => "PHP",
        "rb" => "Ruby",
        "go" => "Go",
        "java" => "Java",
        "c" | "h" => "C",
        "cpp" | "hpp" => "C++",
        "sh" | "bash" => "Bash",
        other => other,
    };

    // Collect reference files for the coder. Two signals:
    //   - `reference_files`: explicit array the brain passed (deterministic).
    //   - `description`: free-form scan for path tokens the brain mentioned in prose.
    // The explicit param is the contract; the scanner stays as a fallback so
    // brains that forget the param still get partial coverage.
    // Brownfield fix v2 (Sprint 13.1, 2026-04-18) — see project_sprint13_brownfield.
    let explicit_refs: Vec<&str> = v
        .get("reference_files")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let references = collect_reference_files(&explicit_refs, description);

    // Generate code via the coder model.
    let code = crate::codet::generate_code(description, language, &references)
        .ok_or("generate_code: coder model returned no usable output")?;

    // Write via the same sandbox logic as write_file (bare relative paths
    // resolve under ~/.claudette/files/).
    let resolved_input = if Path::new(filename).is_absolute()
        || filename.starts_with("~/")
        || filename.starts_with("~\\")
    {
        filename.to_string()
    } else {
        files_dir().join(filename).display().to_string()
    };
    let path = validate_write_path(&resolved_input)?;

    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    fs::write(&path, &code)
        .map_err(|e| format!("generate_code: write {} failed: {e}", path.display()))?;

    let mut result = json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": code.len(),
        "language": language,
        "generated_by": crate::codet::coder_model(),
        // Strong hint for the model: the file is on disk, do not paste
        "reply_hint": "File written. Reply with: file path + 1-sentence \
                       summary. DO NOT include the code in your response.",
    });

    // Run Codet validation (same as write_file post-write hook). Pass the
    // references so the fix-loop also sees the real API when repairing tests.
    if let Some(validation) = crate::codet::validate_code_file(&path, &references) {
        result["validation"] = validation.to_json();

        if let crate::codet::CodetStatus::CouldNotFix { ref last_error } = validation.status {
            let short_err: String = last_error.lines().take(3).collect::<Vec<_>>().join(" | ");
            eprintln!(
                "{} {}",
                crate::theme::warn(crate::theme::WARN_GLYPH),
                crate::theme::warn(&format!(
                    "codet: {} failed validation after {} attempt(s), {} landed — {}",
                    path.display(),
                    validation.attempts_made,
                    validation.fixes_applied,
                    short_err,
                ))
            );
        }
    }

    Ok(result.to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Agent delegation
// ────────────────────────────────────────────────────────────────────────────

fn run_spawn_agent(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("spawn_agent: invalid JSON ({e}): {input}"))?;
    let type_str = v
        .get("agent_type")
        .and_then(Value::as_str)
        .ok_or("spawn_agent: missing 'agent_type'")?;
    let agent_type = crate::agents::AgentType::parse(type_str).ok_or_else(|| {
        format!("spawn_agent: unknown agent type '{type_str}'. Use 'researcher' or 'gitops'.")
    })?;
    let task = v
        .get("task")
        .and_then(Value::as_str)
        .ok_or("spawn_agent: missing 'task'")?;
    let auto_mode = v.get("auto").and_then(Value::as_bool).unwrap_or(false);

    crate::agents::spawn_agent(agent_type, task, auto_mode)
}

// Search group (glob_search, grep_search, web_fetch) lives in
// src/tools/search.rs. `strip_html` and `strip_tag_block` stay here because
// `strip_html` is pub(super) shared with the facts group's web snippets.

/// Strip HTML to plain text. Two-step pipeline:
///   1. Drop the contents of `<script>` and `<style>` blocks (they're
///      garbage for the model and dwarf the visible text on most modern
///      pages — Twitter is multiple MB of JSON-in-JS).
///   2. Remove all remaining tags via a `<` / `>` state machine, decode a
///      handful of common HTML entities, and collapse whitespace runs.
///
/// This is intentionally cheap and dependency-free. It will mangle some
/// pages (anything that abuses `<` literally outside an attribute, or pages
/// that use exotic entities). The 8 KB output cap limits the blast radius.
pub(super) fn strip_html(html: &str) -> String {
    let no_scripts = strip_tag_block(html, "script");
    let no_styles = strip_tag_block(&no_scripts, "style");

    let mut out = String::with_capacity(no_styles.len());
    let mut in_tag = false;
    for c in no_styles.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }

    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    let mut collapsed = String::with_capacity(decoded.len());
    let mut last_ws = true;
    for c in decoded.chars() {
        if c.is_whitespace() {
            if !last_ws {
                collapsed.push(' ');
                last_ws = true;
            }
        } else {
            collapsed.push(c);
            last_ws = false;
        }
    }
    collapsed.trim().to_string()
}

/// Remove every `<tag ...>...</tag>` block (case-insensitive on the tag
/// name). Used by `strip_html` for `<script>` and `<style>`. Substring
/// based — no regex dep — so it's a little dumb but the test coverage
/// pins down the cases that matter.
fn strip_tag_block(html: &str, tag: &str) -> String {
    let open_lower = format!("<{tag}");
    let close_lower = format!("</{tag}>");
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut cursor: usize = 0;

    while cursor < html.len() {
        let Some(rel_open) = lower[cursor..].find(&open_lower) else {
            out.push_str(&html[cursor..]);
            break;
        };
        let abs_open = cursor + rel_open;
        out.push_str(&html[cursor..abs_open]);
        // Find the matching close after the open. If absent, drop the rest.
        match lower[abs_open..].find(&close_lower) {
            Some(rel_close) => {
                cursor = abs_open + rel_close + close_lower.len();
            }
            None => break,
        }
    }
    out
}

// ────────────────────────────────────────────────────────────────────────────
// Sprint 9 Phase 0a — external services (keyless + PAT)
//
// All HTTP calls go through `external_http_client()` which sets a sensible
// User-Agent (required by crates.io, polite for Wikipedia/npm) and a 15-sec
// timeout. Each `run_*` function:
//   • parses the input JSON,
//   • builds the request,
//   • maps HTTP errors to a descriptive `Err(String)`,
//   • returns a compact JSON string that Claudette can summarise.
//
// No cross-tool state — every call is self-contained. Token for GitHub
// tools is read from `GITHUB_TOKEN` (compatible with the GitHub CLI) and
// falls back to `CLAUDETTE_GITHUB_TOKEN`.
// ────────────────────────────────────────────────────────────────────────────

/// User-Agent sent on every external HTTP call. crates.io explicitly rejects
/// clients without a descriptive User-Agent (contact info required).
fn external_user_agent() -> String {
    format!(
        "claudette/{} (claudette; https://github.com/davidtzoar/claudette)",
        env!("CARGO_PKG_VERSION")
    )
}

pub(super) fn external_http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(external_user_agent())
        .build()
        .map_err(|e| format!("external http: build client failed: {e}"))
}

/// Generic "extract `key` as str from a JSON object, or a named error".
pub(super) fn extract_str<'a>(v: &'a Value, key: &str, tool: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{tool}: missing or non-string '{key}'"))
}

pub(super) fn parse_json_input(input: &str, tool: &str) -> Result<Value, String> {
    serde_json::from_str(input).map_err(|e| format!("{tool}: invalid JSON input ({e}): {input}"))
}

// GitHub group (gh_*) lives in src/tools/github.rs.
// Markets group (tv_*, vestige_*) lives in src/tools/markets.rs.

// Telegram group (tg_send, tg_get_updates, tg_send_photo) lives in
// src/tools/telegram.rs.

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Sprint 9 Phase 0a — input validation for new tools. No network.

    // Facts-group tests (wikipedia_*, weather_*) live in src/tools/facts.rs.

    // Registry-group tests (crate_info_rejects_missing_name,
    // npm_info_rejects_missing_name) live in src/tools/registry.rs.

    // GitHub-group tests (gh_*, github_token) live in src/tools/github.rs.

    // Markets-group tests (tv_*, vestige_*, resolve_tv_symbol) live in
    // src/tools/markets.rs.

    // wmo_label, resolve_location, hebrew_city_alias tests live in
    // src/tools/facts.rs alongside their implementations.

    #[test]
    fn parse_json_input_reports_tool_name() {
        let err = parse_json_input("not json", "my_tool").unwrap_err();
        assert!(err.contains("my_tool"));
        assert!(err.contains("invalid JSON"));
    }

    #[test]
    fn extract_str_reports_missing_field() {
        let v: Value = json!({ "foo": 42 });
        let err = extract_str(&v, "bar", "my_tool").unwrap_err();
        assert!(err.contains("my_tool"));
        assert!(err.contains("bar"));
    }

    // slugify test lives in src/tools/notes.rs alongside its implementation.

    #[test]
    fn normalize_path_collapses_dotdot() {
        let p = normalize_path(Path::new("/a/b/../c"));
        assert_eq!(p, PathBuf::from("/a/c"));
    }

    #[test]
    fn normalize_path_collapses_dot() {
        let p = normalize_path(Path::new("/a/./b/./c"));
        assert_eq!(p, PathBuf::from("/a/b/c"));
    }

    #[test]
    fn normalize_path_keeps_leading_dotdot_on_relative() {
        // Relative paths don't get to escape into nothing — leading .. stays.
        let p = normalize_path(Path::new("../foo"));
        assert_eq!(p, PathBuf::from("../foo"));
    }

    #[test]
    fn normalize_path_empty_becomes_dot() {
        let p = normalize_path(Path::new(""));
        assert_eq!(p, PathBuf::from("."));
    }

    #[test]
    fn expand_tilde_replaces_leading_tilde() {
        let home = user_home();
        assert_eq!(expand_tilde("~/foo/bar"), home.join("foo/bar"));
        assert_eq!(expand_tilde("~"), home);
    }

    #[test]
    fn expand_tilde_leaves_other_paths_alone() {
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(
            expand_tilde("relative/path"),
            PathBuf::from("relative/path")
        );
        // Tilde not at start: shells leave it alone, so do we.
        assert_eq!(expand_tilde("foo/~/bar"), PathBuf::from("foo/~/bar"));
    }

    #[test]
    fn validate_read_path_accepts_paths_under_home() {
        // user_home itself should be valid; subdirs of it should be valid.
        let home = user_home();
        let target = home.join("some-doc.txt");
        let result = validate_read_path(target.to_str().unwrap());
        assert!(result.is_ok(), "expected ok, got {result:?}");
    }

    #[test]
    fn validate_read_path_rejects_traversal_escape() {
        // ~/.claudette/../../../etc/passwd resolves to outside home → reject
        let bad = "~/.claudette/../../../../../../etc/passwd";
        let result = validate_read_path(bad);
        assert!(result.is_err(), "expected reject, got {result:?}");
        assert!(
            result.unwrap_err().contains("restricted for safety"),
            "wrong error message"
        );
    }

    #[test]
    fn validate_write_path_accepts_scratch_subdirs() {
        let target = files_dir().join("draft.md");
        let result = validate_write_path(target.to_str().unwrap());
        assert!(result.is_ok(), "expected ok, got {result:?}");
    }

    #[test]
    fn validate_write_path_rejects_outside_scratch() {
        // Even within home, anything outside ~/.claudette/files/ is rejected.
        let outside = user_home().join("Documents").join("draft.md");
        let result = validate_write_path(outside.to_str().unwrap());
        assert!(result.is_err(), "expected reject, got {result:?}");
        assert!(
            result.unwrap_err().contains("sandboxed"),
            "wrong error message"
        );
    }

    #[test]
    fn validate_write_path_rejects_dotdot_escape_from_scratch() {
        // ~/.claudette/files/../../etc → outside scratch → reject
        let bad = "~/.claudette/files/../../etc/passwd";
        let result = validate_write_path(bad);
        assert!(result.is_err(), "expected reject, got {result:?}");
    }

    // File-ops behavior tests (write_file_*, read_file_round_trip,
    // is_code_extension_classifies_correctly) live in src/tools/file_ops.rs.
    // Path-policy tests (validate_read_path_*, validate_write_path_*) stay
    // here because the helpers they test are shared across multiple groups.

    #[test]
    fn get_capabilities_reports_real_config() {
        let raw = dispatch_tool("get_capabilities", "{}").expect("get_capabilities");
        let v: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(v["name"], "Claudette");
        // Sprint 8: tools are now reported as core + optional groups.
        let core = v["tools"]["core"].as_array().expect("core tools array");
        assert!(core.iter().any(|n| n == "get_capabilities"));
        assert!(core.iter().any(|n| n == "read_file"));
        assert!(core.iter().any(|n| n == "todo_add"));
        assert!(
            core.iter().any(|n| n == "enable_tools"),
            "enable_tools meta-tool must be in core"
        );

        // Optional groups should include git, ide, search, advanced.
        let groups = v["tools"]["optional_groups"]
            .as_array()
            .expect("optional_groups array");
        let group_names: Vec<&str> = groups
            .iter()
            .filter_map(|g| g.get("name").and_then(Value::as_str))
            .collect();
        assert!(group_names.contains(&"git"));
        assert!(group_names.contains(&"ide"));
        assert!(group_names.contains(&"search"));
        assert!(group_names.contains(&"advanced"));

        // Total count should add up.
        let total = v["tools"]["total"].as_u64().unwrap() as usize;
        let group_sum: usize = groups
            .iter()
            .map(|g| g["tools"].as_array().map_or(0, Vec::len))
            .sum();
        assert_eq!(total, core.len() + group_sum);

        // Context window must report the real (env-resolved) value, not a
        // made-up number.
        assert_eq!(
            v["context_window"]["num_ctx_tokens"].as_u64().unwrap(),
            u64::from(crate::api::current_num_ctx())
        );

        // Sandbox write boundary must be the actual files_dir(), not a guess.
        let write_path = v["sandbox"]["write"].as_str().unwrap();
        assert!(write_path.contains(".claudette"));
        assert!(write_path.ends_with("files"));
    }

    #[test]
    fn list_dir_classifies_file_and_subdir_correctly() {
        // Regression for the Windows reparse-point bug: build a temp dir
        // containing one real file and one real subdirectory, then verify
        // list_dir returns them with the correct `type` (not "unknown" or
        // mis-classified as "file").
        let tmp = std::env::temp_dir().join(format!(
            "claudette-test-list-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("create tmp");
        fs::create_dir_all(tmp.join("subdir")).expect("create subdir");
        fs::write(tmp.join("hello.txt"), "hi").expect("write file");

        let input = json!({ "path": tmp.to_str().unwrap() }).to_string();
        let out = dispatch_tool("list_dir", &input).expect("list_dir should succeed");
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let entries = parsed["entries"].as_array().expect("entries array");

        let file_entry = entries
            .iter()
            .find(|e| e["name"] == "hello.txt")
            .expect("hello.txt should be listed");
        assert_eq!(file_entry["type"], "file", "hello.txt should be a file");
        assert_eq!(file_entry["size"], 2);

        let dir_entry = entries
            .iter()
            .find(|e| e["name"] == "subdir")
            .expect("subdir should be listed");
        assert_eq!(dir_entry["type"], "dir", "subdir should be a dir");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn list_dir_returns_known_entries() {
        // list the secretary's own data home — it must contain at least
        // notes/ and files/ once we've poked them.
        let _ = ensure_dir(&notes::notes_dir());
        let _ = ensure_dir(&files_dir());

        let input = json!({ "path": claudette_home().to_str().unwrap() }).to_string();
        let out = dispatch_tool("list_dir", &input).expect("list_dir should succeed");
        assert!(out.contains("\"name\":\"files\""));
        assert!(out.contains("\"name\":\"notes\""));
    }

    // === glob_search tests ==================================================

    /// Build a unique temp directory under `~/.claudette/files/` and seed
    /// it with `seed` files. Caller must clean up.
    fn temp_seed_dir(label: &str, seed: &[(&str, &str)]) -> PathBuf {
        let dir = files_dir().join(format!(
            "test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create tmp");
        for (rel, content) in seed {
            let p = dir.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&p, content).expect("write seed file");
        }
        dir
    }

    #[test]
    fn glob_search_matches_files_under_home() {
        // Seed three files inside the sandbox; glob a recursive .txt match.
        let dir = temp_seed_dir(
            "glob",
            &[
                ("a.txt", "alpha"),
                ("nested/b.txt", "bravo"),
                ("nested/c.md", "charlie"),
            ],
        );
        let pattern = format!("{}/**/*.txt", dir.display());
        let input = json!({ "pattern": pattern }).to_string();
        let out = dispatch_tool("glob_search", &input).expect("glob_search should succeed");
        let v: Value = serde_json::from_str(&out).unwrap();
        let count = v["count"].as_u64().unwrap();
        assert_eq!(count, 2, "expected 2 .txt matches, got {out}");
        let paths = v["paths"].as_array().unwrap();
        assert!(paths.iter().any(|p| p.as_str().unwrap().ends_with("a.txt")));
        assert!(paths.iter().any(|p| p.as_str().unwrap().ends_with("b.txt")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_search_rejects_path_outside_home() {
        // An absolute pattern under a system directory should be rejected
        // before we even invoke glob.
        let bad = if cfg!(windows) {
            "C:\\Windows\\**\\*.exe"
        } else {
            "/etc/**/*.conf"
        };
        let input = json!({ "pattern": bad }).to_string();
        let result = dispatch_tool("glob_search", &input);
        assert!(result.is_err(), "expected reject, got {result:?}");
        assert!(
            result.unwrap_err().contains("outside $HOME"),
            "wrong error message"
        );
    }

    #[test]
    fn glob_search_expands_tilde() {
        // ~/.claudette should resolve under home and find at least the
        // files/ directory we always create.
        let _ = ensure_dir(&files_dir());
        let input = json!({ "pattern": "~/.claudette/*" }).to_string();
        let out = dispatch_tool("glob_search", &input).expect("glob_search should succeed");
        assert!(out.contains(".claudette"));
    }

    // === grep_search tests ==================================================

    #[test]
    fn grep_search_finds_substring_match() {
        let dir = temp_seed_dir(
            "grep",
            &[
                ("notes.md", "TODO: write tests\nDONE: build tools\n"),
                ("other.txt", "nothing relevant here\n"),
            ],
        );
        let input = json!({
            "pattern": "todo",
            "path": dir.to_str().unwrap()
        })
        .to_string();
        let out = dispatch_tool("grep_search", &input).expect("grep_search should succeed");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["match_count"].as_u64().unwrap(), 1);
        let matches = v["matches"].as_array().unwrap();
        assert_eq!(matches[0]["line"].as_u64().unwrap(), 1);
        assert!(matches[0]["text"].as_str().unwrap().contains("TODO"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn grep_search_rejects_empty_pattern() {
        let input = json!({ "pattern": "", "path": "~" }).to_string();
        let result = dispatch_tool("grep_search", &input);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("pattern is empty"));
    }

    #[test]
    fn grep_search_rejects_path_outside_home() {
        let bad = if cfg!(windows) { "C:\\Windows" } else { "/etc" };
        let input = json!({ "pattern": "anything", "path": bad }).to_string();
        let result = dispatch_tool("grep_search", &input);
        assert!(result.is_err(), "expected reject, got {result:?}");
    }

    #[test]
    fn grep_search_skips_hidden_directories() {
        let dir = temp_seed_dir("grep-hidden", &[(".secret/inside.md", "FINDME")]);
        let input = json!({
            "pattern": "FINDME",
            "path": dir.to_str().unwrap()
        })
        .to_string();
        let out = dispatch_tool("grep_search", &input).expect("grep_search ok");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["match_count"].as_u64().unwrap(),
            0,
            "should skip hidden dir, got {out}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // === web_fetch / strip_html tests =======================================

    #[test]
    fn web_fetch_rejects_non_http_scheme() {
        let input = json!({ "url": "file:///etc/passwd" }).to_string();
        let result = dispatch_tool("web_fetch", &input);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("http://"));

        let input = json!({ "url": "ftp://example.com" }).to_string();
        let result = dispatch_tool("web_fetch", &input);
        assert!(result.is_err());
    }

    #[test]
    fn strip_html_removes_simple_tags() {
        let html = "<p>Hello <strong>world</strong></p>";
        assert_eq!(strip_html(html), "Hello world");
    }

    #[test]
    fn strip_html_decodes_common_entities() {
        let html = "<p>2 &lt; 5 &amp;&amp; 5 &gt; 2</p>";
        assert_eq!(strip_html(html), "2 < 5 && 5 > 2");
    }

    #[test]
    fn strip_html_collapses_whitespace() {
        let html = "<div>   lots\n\n\n  of    space   </div>";
        assert_eq!(strip_html(html), "lots of space");
    }

    #[test]
    fn strip_html_drops_script_and_style_blocks() {
        let html = "<html><head><style>body{color:red}</style></head>\
                    <body>visible<script>var x = 1;</script>also visible</body></html>";
        let cleaned = strip_html(html);
        assert!(cleaned.contains("visible"));
        assert!(cleaned.contains("also visible"));
        assert!(!cleaned.contains("color:red"), "style content leaked");
        assert!(!cleaned.contains("var x"), "script content leaked");
    }

    #[test]
    fn strip_html_handles_uppercase_script_tag() {
        let html = "before<SCRIPT>BAD</SCRIPT>after";
        let cleaned = strip_html(html);
        assert!(!cleaned.contains("BAD"));
        assert!(cleaned.contains("before"));
        assert!(cleaned.contains("after"));
    }

    // Git-group tests (extract_stat_number_*, git_commit_empty_message_triggers_auto,
    // reject_destructive_blocks_force) live in src/tools/git.rs.

    // Telegram-group tests (tg_*, telegram_token) live in src/tools/telegram.rs.

    // ── Sprint 12 polish pass — time, notes, todos ──────────────────────────

    #[test]
    fn get_current_time_has_new_fields() {
        let out = run_get_current_time();
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        // Original fields still present.
        assert!(v["iso8601"].is_string());
        assert!(v["weekday"].is_string());
        assert!(v["date"].is_string());
        assert!(v["time"].is_string());
        assert!(v["timezone"].is_string());
        // New fields.
        assert!(v["weekday_num"].is_number(), "missing weekday_num");
        let dow = v["weekday_num"].as_u64().unwrap();
        assert!((1..=7).contains(&dow), "weekday_num out of range: {dow}");
        assert!(v["unix_timestamp"].is_number(), "missing unix_timestamp");
        assert!(v["human"].is_string(), "missing human");
        let human = v["human"].as_str().unwrap();
        assert!(
            human.contains(" at "),
            "human should contain ' at ': {human}"
        );
    }

    // Note-handler tests (note_read_rejects_*, note_delete_rejects_*,
    // note_list_*) live in src/tools/notes.rs alongside their handlers.

    #[test]
    fn todo_add_rejects_empty_text() {
        let err = run_todo_add(r#"{"text":""}"#).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn todo_add_rejects_whitespace_only_text() {
        let err = run_todo_add(r#"{"text":"   "}"#).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn todo_add_rejects_missing_text() {
        let err = run_todo_add("{}").unwrap_err();
        assert!(err.contains("missing 'text'"), "got: {err}");
    }

    #[test]
    fn todo_uncomplete_rejects_missing_id() {
        let err = run_todo_uncomplete("{}").unwrap_err();
        assert!(err.contains("missing 'id'"), "got: {err}");
    }

    #[test]
    fn todo_uncomplete_rejects_unknown_id() {
        let err = run_todo_uncomplete(r#"{"id":"t_does_not_exist_99999"}"#).unwrap_err();
        assert!(err.contains("no todo with id"), "got: {err}");
    }

    #[test]
    fn todo_delete_rejects_missing_id() {
        let err = run_todo_delete("{}").unwrap_err();
        assert!(err.contains("missing 'id'"), "got: {err}");
    }

    #[test]
    fn todo_delete_rejects_unknown_id() {
        let err = run_todo_delete(r#"{"id":"t_does_not_exist_99999"}"#).unwrap_err();
        assert!(err.contains("no todo with id"), "got: {err}");
    }

    #[test]
    fn core_tool_names_include_new_tools() {
        use crate::tool_groups::CORE_TOOL_NAMES;
        for tool in &["note_read", "note_delete", "todo_uncomplete", "todo_delete"] {
            assert!(
                CORE_TOOL_NAMES.contains(tool),
                "CORE_TOOL_NAMES missing {tool}"
            );
        }
    }

    #[test]
    fn todo_list_pending_only_flag_passes_through() {
        // Schema accepts pending_only: bool; result reflects it.
        let out = run_todo_list(r#"{"pending_only":true}"#).expect("ok");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["total"].is_number());
        assert!(v["pending"].is_number());
        assert_eq!(v["pending_only"], Value::Bool(true));
    }

    #[test]
    fn notes_and_todos_round_trip() {
        // End-to-end smoke test: create a note, read it back, delete it;
        // add a todo, complete, uncomplete, delete.
        // Uses unique titles/texts so it's safe alongside real data.
        // Notes handlers live in src/tools/notes.rs — go through the public
        // dispatcher to exercise them from this cross-group test.
        let stamp = chrono::Local::now().timestamp_nanos_opt().unwrap_or(0);
        let title = format!("__test_note_{stamp}");
        let body = format!("body-{stamp}");

        let create_input = json!({
            "title": title,
            "body": body,
            "tags": "test,polish"
        })
        .to_string();
        let create_out = dispatch_tool("note_create", &create_input).expect("note_create");
        let created: Value = serde_json::from_str(&create_out).unwrap();
        let note_id = created["id"].as_str().unwrap().to_string();

        // Read it back.
        let read_out =
            dispatch_tool("note_read", &json!({ "id": note_id }).to_string()).expect("note_read");
        let read: Value = serde_json::from_str(&read_out).unwrap();
        assert_eq!(read["title"], Value::String(title.clone()));
        assert!(read["body"].as_str().unwrap().contains(&body));
        assert_eq!(read["tags"], json!(["test", "polish"]));

        // list with search finds it.
        let list_out =
            dispatch_tool("note_list", &json!({ "search": title }).to_string()).expect("note_list");
        let list: Value = serde_json::from_str(&list_out).unwrap();
        assert!(list["count"].as_u64().unwrap() >= 1);

        // Delete it.
        let del_out = dispatch_tool("note_delete", &json!({ "id": note_id }).to_string())
            .expect("note_delete");
        assert!(del_out.contains("\"deleted\":true"));

        // ── todos ─────────────────────────────────────────────────────────
        let todo_text = format!("__test_todo_{stamp}");
        let add_out = run_todo_add(&json!({ "text": todo_text }).to_string()).expect("todo_add");
        let added: Value = serde_json::from_str(&add_out).unwrap();
        let todo_id = added["id"].as_str().unwrap().to_string();

        // Complete.
        let comp_out =
            run_todo_complete(&json!({ "id": todo_id }).to_string()).expect("todo_complete");
        assert!(comp_out.contains("\"done\":true"));

        // Uncomplete.
        let uncomp_out =
            run_todo_uncomplete(&json!({ "id": todo_id }).to_string()).expect("todo_uncomplete");
        assert!(uncomp_out.contains("\"done\":false"));

        // pending_only list should now include it.
        let list_out = run_todo_list(r#"{"pending_only":true}"#).expect("todo_list");
        assert!(list_out.contains(&todo_id));

        // Delete.
        let del_out = run_todo_delete(&json!({ "id": todo_id }).to_string()).expect("todo_delete");
        assert!(del_out.contains("\"deleted\":true"));

        // Confirm gone — second delete errors.
        let err = run_todo_delete(&json!({ "id": todo_id }).to_string()).unwrap_err();
        assert!(err.contains("no todo with id"), "got: {err}");
    }

    // ─── Sprint 13 — reference-file extraction ────────────────────────

    #[test]
    fn looks_like_path_recognises_common_shapes() {
        assert!(looks_like_path("~/foo/bar.py"));
        assert!(looks_like_path("~\\foo\\bar.py"));
        assert!(looks_like_path("./foo"));
        assert!(looks_like_path("../foo"));
        assert!(looks_like_path("/abs/path"));
        assert!(looks_like_path("C:\\Users\\me\\x.py"));
        assert!(looks_like_path("D:/dev/claudette/x.py"));
        assert!(!looks_like_path("plainword"));
        assert!(!looks_like_path("file.py")); // bare filename — not a path per se
        assert!(!looks_like_path("https://example.com/x.py"));
        assert!(!looks_like_path("http://example.com/x.py"));
    }

    #[test]
    fn has_code_extension_recognises_code_files() {
        assert!(has_code_extension("calculator.py"));
        assert!(has_code_extension("lib.RS")); // case-insensitive
        assert!(has_code_extension("path/to/file.ts"));
        assert!(!has_code_extension("no-extension"));
        assert!(!has_code_extension("readme"));
        // Extensions we don't include shouldn't leak in.
        assert!(!has_code_extension("archive.zip"));
    }

    #[test]
    fn extract_path_candidates_strips_punctuation_and_brackets() {
        let text = "Read the file ~/.claudette/files/calculator.py — it's a module.";
        let cands = extract_path_candidates(text);
        assert!(
            cands
                .iter()
                .any(|t| t == "~/.claudette/files/calculator.py"),
            "missing tilde path, got: {cands:?}",
        );
    }

    #[test]
    fn extract_path_candidates_keeps_bare_code_filename() {
        let cands = extract_path_candidates("Please read calculator.py carefully.");
        assert!(
            cands.iter().any(|t| t == "calculator.py"),
            "missing bare filename, got: {cands:?}",
        );
    }

    #[test]
    fn extract_path_candidates_ignores_urls_and_prose() {
        let cands =
            extract_path_candidates("Visit https://example.com/x.py then write a greeting.");
        // No URL, no plain prose words.
        assert!(
            !cands.iter().any(|t| t.contains("example.com")),
            "leaked URL: {cands:?}",
        );
        assert!(
            !cands.iter().any(|t| t == "greeting"),
            "kept prose word: {cands:?}",
        );
    }

    #[test]
    fn collect_reference_files_reads_tilde_path() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]); // start clean
                                        // Write a fixture under the user's home so validate_read_path accepts it.
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_fixture.py");
        let body = "class RefFixture:\n    def hello(self):\n        return 'hi'\n";
        fs::write(&fixture, body).unwrap();

        let desc =
            "Read the file ~/.claudette/files/refsprint_fixture.py and write tests for its API."
                .to_string();
        let refs = collect_reference_files(&[], &desc);

        // Cleanup before asserting so we don't leak fixtures on failure.
        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "expected 1 reference, got {}", refs.len());
        assert!(
            refs[0].content.contains("class RefFixture"),
            "content missing, got: {:?}",
            refs[0].content
        );
        assert_eq!(refs[0].path, "~/.claudette/files/refsprint_fixture.py");
    }

    #[test]
    fn collect_reference_files_ignores_missing_and_non_code() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        // A description with a URL, a word, and a nonexistent filename.
        let desc = "Write a function. No file here. See http://example.com/foo.py and ghost.py.";
        let refs = collect_reference_files(&[], desc);
        assert!(
            refs.is_empty(),
            "expected no refs for missing files, got {refs:?}",
        );
    }

    #[test]
    fn collect_reference_files_caps_file_size() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_big_fixture.py");
        // 20 KB of Python, over the 16 KB per-file cap.
        let body: String = "x = 1\n".repeat(20 * 1024 / 6 + 1);
        fs::write(&fixture, &body).unwrap();

        let desc = "See ~/.claudette/files/refsprint_big_fixture.py".to_string();
        let refs = collect_reference_files(&[], &desc);

        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1);
        assert!(
            refs[0].content.contains("[truncated — file continues]"),
            "missing truncation marker",
        );
        assert!(
            refs[0].content.len() <= 16 * 1024 + 100,
            "content not truncated: {} bytes",
            refs[0].content.len()
        );
    }

    // ─── Sprint 13.1 — explicit reference_files param ────────────────

    #[test]
    fn collect_reference_files_uses_explicit_param() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_explicit_fixture.py");
        let body = "def explicit_marker():\n    return 'from explicit param'\n";
        fs::write(&fixture, body).unwrap();

        // Description has NO path tokens — only the explicit param does.
        let desc = "Write tests for the helper module.";
        let explicit = ["~/.claudette/files/refsprint_explicit_fixture.py"];
        let refs = collect_reference_files(&explicit, desc);

        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "expected 1 reference, got {}", refs.len());
        assert!(
            refs[0].content.contains("explicit_marker"),
            "content missing, got: {:?}",
            refs[0].content
        );
    }

    #[test]
    fn collect_reference_files_dedups_explicit_and_scanner() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_dedup_fixture.py");
        fs::write(&fixture, "x = 1\n").unwrap();

        // Same path appears in BOTH the explicit param and the description text.
        let desc = "Read ~/.claudette/files/refsprint_dedup_fixture.py and tests.";
        let explicit = ["~/.claudette/files/refsprint_dedup_fixture.py"];
        let refs = collect_reference_files(&explicit, desc);

        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "duplicate not collapsed: {refs:?}");
    }

    #[test]
    fn collect_reference_files_silently_drops_invalid_explicit_paths() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        // Explicit paths that don't exist on disk are filtered out, not erroring.
        let explicit = ["/this/path/does/not/exist.py", "~/no_such_file.py"];
        let refs = collect_reference_files(&explicit, "irrelevant description");
        assert!(refs.is_empty(), "expected empty, got {refs:?}");
    }

    // ─── Sprint 13.2 — per-turn user-prompt path stash ───────────────

    /// Serializer for any test that reads or writes `CURRENT_TURN_PATHS`.
    /// Cargo runs tests in parallel; without this guard, a stash-setting test
    /// can leak state into a stash-reading test running concurrently.
    static STASH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_stash() -> std::sync::MutexGuard<'static, ()> {
        // Recover from poisoning — a panic in one test must not block the rest.
        STASH_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn extract_user_prompt_paths_keeps_existing_files_only() {
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_stash_real.py");
        fs::write(&fixture, "x = 1\n").unwrap();

        let prompt = "Add tests for ~/.claudette/files/refsprint_stash_real.py \
                      and also for ~/.claudette/files/refsprint_stash_ghost.py";
        let paths = extract_user_prompt_paths(prompt);
        let _ = fs::remove_file(&fixture);

        assert!(
            paths.iter().any(|p| p.contains("refsprint_stash_real.py")),
            "real path missing: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains("refsprint_stash_ghost.py")),
            "ghost path leaked: {paths:?}"
        );
    }

    #[test]
    fn collect_reference_files_honours_turn_stash() {
        let _g = lock_stash();
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_stash_fixture.py");
        let body = "def stash_marker():\n    return 'from turn stash'\n";
        fs::write(&fixture, body).unwrap();

        // Stash one path; pass empty explicit, irrelevant description.
        set_current_turn_paths(vec![
            "~/.claudette/files/refsprint_stash_fixture.py".to_string()
        ]);
        let refs = collect_reference_files(&[], "Write tests for the helper.");

        // Always clear the stash so other tests aren't affected.
        set_current_turn_paths(vec![]);
        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "stash not honoured: {refs:?}");
        assert!(
            refs[0].content.contains("stash_marker"),
            "wrong content: {:?}",
            refs[0].content
        );
    }

    #[test]
    fn set_current_turn_paths_overwrites_previous_stash() {
        let _g = lock_stash();
        set_current_turn_paths(vec!["a.py".to_string(), "b.py".to_string()]);
        assert_eq!(current_turn_paths().len(), 2);
        set_current_turn_paths(vec!["c.py".to_string()]);
        assert_eq!(current_turn_paths(), vec!["c.py".to_string()]);
        set_current_turn_paths(vec![]);
        assert!(current_turn_paths().is_empty());
    }

    #[test]
    fn collect_reference_files_explicit_respects_max_files() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let mut fixtures = Vec::new();
        let mut explicit_paths = Vec::new();
        for i in 0..6 {
            let p = dir.join(format!("refsprint_cap_fixture_{i}.py"));
            fs::write(&p, format!("# fixture {i}\nx = {i}\n")).unwrap();
            fixtures.push(p);
            explicit_paths.push(format!("~/.claudette/files/refsprint_cap_fixture_{i}.py"));
        }
        let explicit_refs: Vec<&str> = explicit_paths.iter().map(String::as_str).collect();

        let refs = collect_reference_files(&explicit_refs, "");

        for f in &fixtures {
            let _ = fs::remove_file(f);
        }

        assert_eq!(
            refs.len(),
            REF_MAX_FILES,
            "expected cap, got {}",
            refs.len()
        );
    }
}
