//! Todos group — 5 tools (todo_add, todo_list, todo_complete,
//! todo_uncomplete, todo_delete).
//!
//! Storage: a single `todos.json` file under `~/.claudette/`. Reads the
//! whole file on every call and writes it back — fine at personal-agent
//! volume, no need for incremental updates or a DB.
//!
//! Self-contained: `todos_path` (pub(super) so get_capabilities can
//! show it), the `Todo` struct, and the `load_todos` / `save_todos`
//! helpers are private. Handlers reuse the parent-module `ensure_dir`
//! and `claudette_home` helpers.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{claudette_home, ensure_dir};

pub(super) fn todos_path() -> PathBuf {
    claudette_home().join("todos.json")
}

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

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
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
        }),
        json!({
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
        }),
        json!({
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
        }),
        json!({
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
        }),
        json!({
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
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "todo_add" => run_todo_add(input),
        "todo_list" => run_todo_list(input),
        "todo_complete" => run_todo_complete(input),
        "todo_uncomplete" => run_todo_uncomplete(input),
        "todo_delete" => run_todo_delete(input),
        _ => return None,
    };
    Some(result)
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn todo_list_pending_only_flag_passes_through() {
        // Schema accepts pending_only: bool; result reflects it.
        let out = run_todo_list(r#"{"pending_only":true}"#).expect("ok");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["total"].is_number());
        assert!(v["pending"].is_number());
        assert_eq!(v["pending_only"], Value::Bool(true));
    }

    #[test]
    fn schemas_lists_five_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 5);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "todo_add",
                "todo_list",
                "todo_complete",
                "todo_uncomplete",
                "todo_delete"
            ]
        );
    }
}
