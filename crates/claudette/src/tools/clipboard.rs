//! Clipboard group — `clipboard_read` + `clipboard_write` (Sprint v0.6.0
//! Phase 3.4b). Cross-platform via the `arboard` crate (already a
//! dependency for the TUI's Alt+V image paste).
//!
//! Only text is exposed at the tool layer; arboard can also push images
//! but those are paste-driven and the model has the vision pair for
//! that direction. Keeping the surface small avoids accidentally
//! exposing a "write a screenshot to the clipboard" path that nobody
//! has asked for.

use serde_json::{json, Value};

use super::parse_json_input;

const MAX_WRITE_BYTES: usize = 1_000_000;

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "clipboard_read",
                "description": "Read text from the OS clipboard. Returns {text}. Errors if the clipboard is empty or contains a non-text payload (e.g. an image).",
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
                "name": "clipboard_write",
                "description": "Write text to the OS clipboard. Returns {ok: true, bytes}.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "Text to copy to the clipboard. Capped at 1,000,000 bytes." }
                    },
                    "required": ["text"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "clipboard_read" => run_clipboard_read(input),
        "clipboard_write" => run_clipboard_write(input),
        _ => return None,
    };
    Some(result)
}

fn run_clipboard_read(_input: &str) -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| format!("clipboard_read: open clipboard failed: {e}"))?;
    match clipboard.get_text() {
        Ok(text) => Ok(json!({
            "ok": true,
            "text": text,
            "bytes": text.len(),
        })
        .to_string()),
        Err(arboard::Error::ContentNotAvailable) => {
            Err("clipboard_read: clipboard is empty or contains a non-text payload".to_string())
        }
        Err(e) => Err(format!("clipboard_read: read failed: {e}")),
    }
}

fn run_clipboard_write(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "clipboard_write")?;
    let text = v
        .get("text")
        .and_then(Value::as_str)
        .ok_or("clipboard_write: missing 'text'")?;
    if text.len() > MAX_WRITE_BYTES {
        return Err(format!(
            "clipboard_write: 'text' is {} bytes, exceeds {} cap",
            text.len(),
            MAX_WRITE_BYTES
        ));
    }
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| format!("clipboard_write: open clipboard failed: {e}"))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| format!("clipboard_write: write failed: {e}"))?;
    Ok(json!({
        "ok": true,
        "bytes": text.len(),
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_lists_two_tools() {
        let s = schemas();
        assert_eq!(s.len(), 2);
        let names: Vec<&str> = s
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["clipboard_read", "clipboard_write"]);
    }

    #[test]
    fn clipboard_write_rejects_missing_text() {
        let err = run_clipboard_write("{}").unwrap_err();
        assert!(err.contains("missing 'text'"), "got: {err}");
    }

    #[test]
    fn clipboard_write_rejects_oversize() {
        let big = "x".repeat(MAX_WRITE_BYTES + 1);
        let err = run_clipboard_write(&json!({ "text": &big }).to_string()).unwrap_err();
        assert!(err.contains("exceeds"), "got: {err}");
    }

    #[test]
    fn clipboard_round_trip() {
        // Best-effort — CI runners often have no clipboard. Skip on
        // ContentNotAvailable / open failure rather than fail the test.
        let unique = format!(
            "claudette-clipboard-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let write_out = run_clipboard_write(&json!({ "text": &unique }).to_string());
        if write_out.is_err() {
            return;
        }
        let read_out = run_clipboard_read("{}");
        if read_out.is_err() {
            return;
        }
        let v: Value = serde_json::from_str(&read_out.unwrap()).unwrap();
        assert_eq!(v["text"].as_str().unwrap_or(""), unique);
    }
}
