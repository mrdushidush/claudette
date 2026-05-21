//! Dialog group — `ask_user`. Sprint v0.6.0 Phase 3.4a.
//!
//! The brief calls for `ask_user` as a turn-suspender across all
//! surfaces (TUI modal, Telegram buttons, REPL stdin). This MVP ships
//! the REPL/stdin path that's also the fallback for piped invocations
//! — it works whenever the agent is running in a foreground terminal,
//! which covers the majority of single-shot and REPL sessions.
//!
//! Full TUI integration (modal rendering, async wait) and Telegram
//! integration (inline buttons via Bot API) are documented as
//! follow-up work in CHANGELOG.md. The dispatch surface is identical
//! between surfaces, so adding those paths later doesn't break this
//! tool's schema or any prior-turn tool calls.

use std::io::{IsTerminal, Write};

use serde_json::{json, Value};

use super::parse_json_input;

pub(super) fn schemas() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "ask_user",
            "description": "Ask the user a clarifying question and wait for their answer. Use when you genuinely can't proceed without input (NOT for every step — emit text directly when you can). Returns {answer}.",
            "parameters": {
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "The question to ask (one line)." },
                    "options":  { "type": "array", "description": "Optional: list of allowed answers (case-insensitive).", "items": { "type": "string" } },
                    "default":  { "type": "string", "description": "Optional: returned if the user submits an empty line." }
                },
                "required": ["question"]
            }
        }
    })]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "ask_user" => run_ask_user(input),
        _ => return None,
    };
    Some(result)
}

fn run_ask_user(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "ask_user")?;
    let question = v
        .get("question")
        .and_then(Value::as_str)
        .ok_or("ask_user: missing 'question'")?
        .trim();
    if question.is_empty() {
        return Err("ask_user: 'question' is empty".to_string());
    }
    let options: Vec<String> = v
        .get("options")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let default = v.get("default").and_then(Value::as_str);

    // If stdin isn't a TTY (piped/non-interactive run, the harness
    // hasn't wired modal yet), fall back to `default` if given;
    // otherwise return a clear error so the brain knows interactive
    // input isn't available rather than blocking on a read that won't
    // ever complete.
    if !std::io::stdin().is_terminal() {
        if let Some(d) = default {
            return Ok(json!({
                "question": question,
                "answer": d,
                "source": "default-fallback (no TTY)",
            })
            .to_string());
        }
        return Err(
            "ask_user: stdin is not a TTY — pass `default` so the brain has a fallback when running non-interactively"
                .to_string(),
        );
    }

    let mut stderr = std::io::stderr();
    let _ = writeln!(stderr);
    let _ = writeln!(stderr, "❓ {question}");
    if !options.is_empty() {
        let _ = writeln!(stderr, "   options: {}", options.join(" | "));
    }
    if let Some(d) = default {
        let _ = writeln!(stderr, "   (press Enter for default: {d})");
    }
    let _ = write!(stderr, "> ");
    let _ = stderr.flush();

    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("ask_user: read failed: {e}"))?;
    let answer_raw = line.trim().to_string();
    let answer = if answer_raw.is_empty() {
        match default {
            Some(d) => d.to_string(),
            None => return Err("ask_user: no answer given (empty line, no default)".to_string()),
        }
    } else {
        answer_raw
    };

    if !options.is_empty() {
        let lower = answer.to_lowercase();
        if !options.iter().any(|o| o.to_lowercase() == lower) {
            return Err(format!(
                "ask_user: answer '{answer}' not in allowed options ({})",
                options.join(", ")
            ));
        }
    }

    Ok(json!({
        "question": question,
        "answer": answer,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_lists_one_tool() {
        let s = schemas();
        assert_eq!(s.len(), 1);
        let name = s[0]
            .pointer("/function/name")
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(name, "ask_user");
    }

    #[test]
    fn ask_user_rejects_missing_question() {
        let err = run_ask_user("{}").unwrap_err();
        assert!(err.contains("missing 'question'"), "got: {err}");
    }

    #[test]
    fn ask_user_rejects_empty_question() {
        let err = run_ask_user(r#"{"question":"   "}"#).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn ask_user_uses_default_when_no_tty() {
        // CI runs are non-TTY; we exploit that to exercise the
        // default-fallback path without standing up a pty.
        if std::io::stdin().is_terminal() {
            // Interactive run — skip; the read would actually wait.
            return;
        }
        let out = run_ask_user(&json!({ "question": "ready?", "default": "yes" }).to_string())
            .expect("ok");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["answer"], "yes");
        assert!(v["source"].as_str().unwrap_or("").contains("default"));
    }

    #[test]
    fn ask_user_errors_without_default_when_no_tty() {
        if std::io::stdin().is_terminal() {
            return;
        }
        let err = run_ask_user(r#"{"question":"go?"}"#).unwrap_err();
        assert!(err.contains("not a TTY"), "got: {err}");
    }
}
