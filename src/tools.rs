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

use serde_json::{json, Value};

// Per-group sub-modules. Each exports `schemas()` and `dispatch()`; see the
// group-module contract at the top of `registry.rs`.
mod calendar;
mod codegen;
mod facts;
mod file_ops;
mod git;
mod github;
mod ide;
mod markets;
mod notes;
mod registry;
mod schedule;
mod search;
mod shell;
mod telegram;
mod todos;
mod web_search;

// Pub re-exports for entry points that pre-extract paths from the raw
// user prompt before each turn (REPL / single-shot / Telegram / TUI).
// Moved to src/tools/codegen.rs alongside the reference-file
// infrastructure they feed into; re-exported here so call sites keep
// the stable `crate::tools::set_current_turn_paths` / `...extract_user_prompt_paths`
// paths.
pub use codegen::{extract_user_prompt_paths, set_current_turn_paths};

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
        // Todos group (todo_add, todo_list, todo_complete, todo_uncomplete,
        // todo_delete) lives in src/tools/todos.rs and is appended below.
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
        // Shell + edit group (bash, edit_file — DangerFullAccess) lives
        // in src/tools/shell.rs and is appended to this array below.
        // Codegen group (generate_code, spawn_agent — plus reference-file
        // extraction infrastructure) lives in src/tools/codegen.rs and is
        // appended to this array below.
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
    tools.extend(calendar::schemas());
    tools.extend(codegen::schemas());
    tools.extend(facts::schemas());
    tools.extend(file_ops::schemas());
    tools.extend(git::schemas());
    tools.extend(github::schemas());
    tools.extend(ide::schemas());
    tools.extend(markets::schemas());
    tools.extend(notes::schemas());
    tools.extend(registry::schemas());
    tools.extend(schedule::schemas());
    tools.extend(search::schemas());
    tools.extend(shell::schemas());
    tools.extend(telegram::schemas());
    tools.extend(todos::schemas());
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
    if let Some(result) = calendar::dispatch(name, input) {
        return result;
    }
    if let Some(result) = codegen::dispatch(name, input) {
        return result;
    }
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
    if let Some(result) = schedule::dispatch(name, input) {
        return result;
    }
    if let Some(result) = search::dispatch(name, input) {
        return result;
    }
    if let Some(result) = shell::dispatch(name, input) {
        return result;
    }
    if let Some(result) = telegram::dispatch(name, input) {
        return result;
    }
    if let Some(result) = todos::dispatch(name, input) {
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
        // Todos group (todo_*) handled by the early-return above via
        // todos::dispatch.
        // File ops group (read_file, write_file, list_dir) handled by
        // the early-return above via file_ops::dispatch.
        "get_capabilities" => Ok(run_get_capabilities()),
        // Web-search group (web_search) handled by the early-return above
        // via web_search::dispatch.
        // Search group (glob_search, grep_search, web_fetch) is handled by
        // the early-return above via search::dispatch.
        // Git group (git_*) handled by the early-return above via
        // git::dispatch.
        // Shell + edit group (bash, edit_file) handled by the early-return
        // above via shell::dispatch.
        // Codegen group (generate_code, spawn_agent) handled by the
        // early-return above via codegen::dispatch.
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
// Todos group (todo_*) + Todo struct + load/save_todos + todos_path
// live in src/tools/todos.rs.

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
            "todos": todos::todos_path().display().to_string(),
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

// Codegen group — generate_code + spawn_agent, along with reference-file
// extraction (collect_reference_files, extract_path_candidates,
// resolve_reference, looks_like_path, has_code_extension) and the
// per-turn user-prompt path stash (set_current_turn_paths,
// extract_user_prompt_paths, CURRENT_TURN_PATHS) live in
// src/tools/codegen.rs. set_current_turn_paths and extract_user_prompt_paths
// are pub-re-exported from this module for REPL/Telegram/TUI entry points.
// `is_code_extension` + CODE_EXTENSIONS moved with write_file into
// src/tools/file_ops.rs.

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

// Shell + edit group (bash, edit_file) lives in src/tools/shell.rs.

// Codegen group (generate_code, spawn_agent) lives in src/tools/codegen.rs.

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

    // Todo-handler tests (todo_add_rejects_*, todo_uncomplete_rejects_*,
    // todo_delete_rejects_*, todo_list_pending_only_flag_passes_through)
    // live in src/tools/todos.rs alongside their handlers.

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
        // Todos handlers live in src/tools/todos.rs — go through dispatch_tool
        // for the same reason as the notes side.
        let todo_text = format!("__test_todo_{stamp}");
        let add_out =
            dispatch_tool("todo_add", &json!({ "text": todo_text }).to_string()).expect("todo_add");
        let added: Value = serde_json::from_str(&add_out).unwrap();
        let todo_id = added["id"].as_str().unwrap().to_string();

        // Complete.
        let comp_out = dispatch_tool("todo_complete", &json!({ "id": todo_id }).to_string())
            .expect("todo_complete");
        assert!(comp_out.contains("\"done\":true"));

        // Uncomplete.
        let uncomp_out = dispatch_tool("todo_uncomplete", &json!({ "id": todo_id }).to_string())
            .expect("todo_uncomplete");
        assert!(uncomp_out.contains("\"done\":false"));

        // pending_only list should now include it.
        let list_out = dispatch_tool("todo_list", r#"{"pending_only":true}"#).expect("todo_list");
        assert!(list_out.contains(&todo_id));

        // Delete.
        let del_out = dispatch_tool("todo_delete", &json!({ "id": todo_id }).to_string())
            .expect("todo_delete");
        assert!(del_out.contains("\"deleted\":true"));

        // Confirm gone — second delete errors.
        let err = dispatch_tool("todo_delete", &json!({ "id": todo_id }).to_string()).unwrap_err();
        assert!(err.contains("no todo with id"), "got: {err}");
    }

    // Reference-file extraction tests (looks_like_path_*, has_code_extension_*,
    // extract_path_candidates_*, collect_reference_files_*,
    // extract_user_prompt_paths_*, set_current_turn_paths_*) live in
    // src/tools/codegen.rs alongside their implementations.
}
