//! Scheduling group — 4 tools against the process-wide
//! [`crate::scheduler::Scheduler`].
//!
//! Natural-language expressions ("tomorrow at 3pm", "every weekday at 7am")
//! are parsed deterministically in Rust, never by the LLM. On a parse
//! failure the tool returns a structured error with examples so the model
//! can retry with a corrected expression.
//!
//! State lives in the process-wide scheduler singleton; the Telegram
//! consumer is the only code path that actually fires scheduled events
//! (via `Scheduler::fire_due`), so handler behaviour here is limited to
//! add/list/cancel.

use serde_json::{json, Value};

use super::{extract_str, parse_json_input};
use crate::scheduler::{self, CatchUp, ScheduleEntry, ScheduleKind};

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "schedule_once",
                "description": "Schedule a one-shot reminder. The prompt is what the assistant will act on at fire time (e.g. 'send me a message: call the dentist'). Accepts human expressions like 'in 30 minutes', 'tomorrow at 15:00', 'at 7pm', 'today at 3pm', or an RFC3339 datetime.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "when":    { "type": "string", "description": "When to fire (natural language or RFC3339)." },
                        "prompt":  { "type": "string", "description": "What the assistant should do at fire time." },
                        "chat_id": { "type": "number", "description": "Telegram chat to notify. Defaults to the current chat if called from the Telegram bot." },
                        "catch_up": { "type": "string", "enum": ["once", "skip", "all"], "description": "What to do if the bot was offline at fire time. Default: 'once'." }
                    },
                    "required": ["when", "prompt"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "schedule_recurring",
                "description": "Schedule a recurring reminder. Accepts human expressions like 'every weekday at 07:00', 'daily at 09:30', 'every 15 minutes', 'every monday at 10:00', or a raw cron string prefixed with 'cron:'.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "when":    { "type": "string", "description": "Recurrence expression (see description)." },
                        "prompt":  { "type": "string", "description": "What the assistant should do at fire time." },
                        "chat_id": { "type": "number", "description": "Telegram chat to notify. Defaults to the current chat if called from the Telegram bot." },
                        "catch_up": { "type": "string", "enum": ["once", "skip", "all"], "description": "What to do about missed occurrences. Default: 'skip' for recurring." }
                    },
                    "required": ["when", "prompt"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "schedule_list",
                "description": "List active scheduled reminders (one-shot + recurring).",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "schedule_cancel",
                "description": "Cancel a scheduled reminder by its id.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Entry id from schedule_list (e.g. 'sch_abc123')." }
                    },
                    "required": ["id"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "schedule_once" => run_schedule_once(input),
        "schedule_recurring" => run_schedule_recurring(input),
        "schedule_list" => run_schedule_list(),
        "schedule_cancel" => run_schedule_cancel(input),
        _ => return None,
    };
    Some(result)
}

fn parse_catch_up(v: &Value) -> Option<CatchUp> {
    v.get("catch_up").and_then(Value::as_str).and_then(|s| {
        match s.to_lowercase().as_str() {
            "once" => Some(CatchUp::Once),
            "skip" => Some(CatchUp::Skip),
            "all" => Some(CatchUp::All),
            _ => None,
        }
    })
}

fn run_schedule_once(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "schedule_once")?;
    let when = extract_str(&v, "when", "schedule_once")?;
    let prompt = extract_str(&v, "prompt", "schedule_once")?;
    let chat_id = v.get("chat_id").and_then(Value::as_i64);
    let catch_up = parse_catch_up(&v);

    let mut g = scheduler::global()
        .lock()
        .map_err(|e| format!("schedule_once: scheduler mutex poisoned: {e}"))?;
    let entry = g.add(when, prompt.to_string(), chat_id, catch_up)?;
    Ok(serialize_entry(&entry, "scheduled"))
}

fn run_schedule_recurring(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "schedule_recurring")?;
    let when = extract_str(&v, "when", "schedule_recurring")?;
    let prompt = extract_str(&v, "prompt", "schedule_recurring")?;
    let chat_id = v.get("chat_id").and_then(Value::as_i64);
    let catch_up = parse_catch_up(&v);

    let mut g = scheduler::global()
        .lock()
        .map_err(|e| format!("schedule_recurring: scheduler mutex poisoned: {e}"))?;
    let entry = g.add(when, prompt.to_string(), chat_id, catch_up)?;

    if entry.kind != ScheduleKind::Recurring {
        // User asked for recurring but expression parsed as one-shot —
        // roll back so the scheduler doesn't silently downgrade intent.
        let _ = g.cancel(&entry.id);
        return Err(format!(
            "schedule_recurring: expression '{when}' parsed as a one-shot. \
             Use schedule_once for that, or try 'every weekday at HH:MM', \
             'daily at HH:MM', 'every N minutes', or 'cron: …'."
        ));
    }

    Ok(serialize_entry(&entry, "scheduled"))
}

fn run_schedule_list() -> Result<String, String> {
    let g = scheduler::global()
        .lock()
        .map_err(|e| format!("schedule_list: scheduler mutex poisoned: {e}"))?;
    let entries: Vec<Value> = g.list().iter().map(summarize_entry).collect();
    Ok(json!({
        "count": entries.len(),
        "entries": entries,
    })
    .to_string())
}

fn run_schedule_cancel(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "schedule_cancel")?;
    let id = extract_str(&v, "id", "schedule_cancel")?;

    let mut g = scheduler::global()
        .lock()
        .map_err(|e| format!("schedule_cancel: scheduler mutex poisoned: {e}"))?;
    let removed = g.cancel(id)?;
    if !removed {
        return Err(format!("schedule_cancel: no entry with id '{id}'"));
    }
    Ok(json!({ "ok": true, "cancelled": true, "id": id }).to_string())
}

fn serialize_entry(entry: &ScheduleEntry, status: &str) -> String {
    json!({
        "ok": true,
        "status": status,
        "entry": summarize_entry(entry),
    })
    .to_string()
}

fn summarize_entry(entry: &ScheduleEntry) -> Value {
    json!({
        "id": entry.id,
        "kind": match entry.kind {
            ScheduleKind::OneShot => "once",
            ScheduleKind::Recurring => "recurring",
        },
        "when": entry.original_expr,
        "next_fire_at": entry.next_fire_at.to_rfc3339(),
        "recurrence": entry.recurrence,
        "prompt": entry.prompt,
        "chat_id": entry.chat_id,
        "catch_up": match entry.catch_up {
            CatchUp::Once => "once",
            CatchUp::Skip => "skip",
            CatchUp::All => "all",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_lists_four_tools() {
        let s = schemas();
        assert_eq!(s.len(), 4);
        let names: Vec<&str> = s
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "schedule_once",
                "schedule_recurring",
                "schedule_list",
                "schedule_cancel",
            ]
        );
    }

    #[test]
    fn schedule_once_rejects_missing_when() {
        let err = run_schedule_once(r#"{"prompt":"hi"}"#).unwrap_err();
        assert!(err.contains("when"), "got: {err}");
    }

    #[test]
    fn schedule_once_rejects_missing_prompt() {
        let err = run_schedule_once(r#"{"when":"in 5 minutes"}"#).unwrap_err();
        assert!(err.contains("prompt"), "got: {err}");
    }

    #[test]
    fn schedule_once_rejects_invalid_expression() {
        let err = run_schedule_once(
            r#"{"when":"sometime soon","prompt":"x"}"#,
        )
        .unwrap_err();
        assert!(err.contains("could not parse"), "got: {err}");
    }

    #[test]
    fn schedule_recurring_rejects_oneshot_expression() {
        // "in 5 minutes" parses as one-shot — the recurring tool must
        // reject that rather than silently downgrade.
        let err = run_schedule_recurring(
            r#"{"when":"in 5 minutes","prompt":"x"}"#,
        )
        .unwrap_err();
        assert!(err.contains("parsed as a one-shot"), "got: {err}");
    }

    #[test]
    fn schedule_cancel_rejects_missing_id() {
        let err = run_schedule_cancel("{}").unwrap_err();
        assert!(err.contains("id"), "got: {err}");
    }

    #[test]
    fn schedule_cancel_rejects_unknown_id() {
        let err =
            run_schedule_cancel(r#"{"id":"sch_definitely_not_real_xyz123"}"#).unwrap_err();
        assert!(err.contains("no entry with id"), "got: {err}");
    }

    #[test]
    fn parse_catch_up_accepts_all_three_values() {
        assert_eq!(
            parse_catch_up(&json!({ "catch_up": "once" })),
            Some(CatchUp::Once)
        );
        assert_eq!(
            parse_catch_up(&json!({ "catch_up": "SKIP" })),
            Some(CatchUp::Skip)
        );
        assert_eq!(
            parse_catch_up(&json!({ "catch_up": "all" })),
            Some(CatchUp::All)
        );
        assert_eq!(parse_catch_up(&json!({})), None);
        assert_eq!(parse_catch_up(&json!({ "catch_up": "junk" })), None);
    }
}
