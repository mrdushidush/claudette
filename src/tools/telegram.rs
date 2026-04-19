//! Telegram bot group — 3 tools against the Bot API (Sprint 10). Token
//! comes from `crate::secrets::read_secret("telegram")`
//! (CLAUDETTE_TELEGRAM_TOKEN / TELEGRAM_BOT_TOKEN env or
//! `~/.claudette/secrets/telegram.token`).
//!
//! Self-contained: all helpers (`telegram_token`, `tg_extract_chat_id`,
//! `tg_api_url`) are private to this module. Handlers reuse the pub(super)
//! parent helpers `parse_json_input`, `extract_str`, `external_http_client`.

use serde_json::{json, Value};

use super::{external_http_client, extract_str, parse_json_input};

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "tg_send",
                "description": "Send a text message via Telegram bot. Supports Markdown formatting.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "string", "description": "Telegram chat ID (user or group). Use tg_get_updates to discover chat IDs." },
                        "text":    { "type": "string", "description": "Message text (supports Markdown)" }
                    },
                    "required": ["chat_id", "text"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "tg_get_updates",
                "description": "Poll recent messages/commands sent to the Telegram bot. Use this to discover chat IDs and read incoming messages.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "limit":  { "type": "number", "description": "Max updates to return (default 10, max 100)" },
                        "offset": { "type": "number", "description": "Update offset — pass last update_id+1 to acknowledge previous updates" }
                    },
                    "required": []
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "tg_send_photo",
                "description": "Send a photo via Telegram bot by URL.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "string", "description": "Telegram chat ID" },
                        "url":     { "type": "string", "description": "Public URL of the image to send" },
                        "caption": { "type": "string", "description": "Optional caption for the photo" }
                    },
                    "required": ["chat_id", "url"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "tg_send" => run_tg_send(input),
        "tg_get_updates" => run_tg_get_updates(input),
        "tg_send_photo" => run_tg_send_photo(input),
        _ => return None,
    };
    Some(result)
}

/// Resolve the Telegram Bot API token via the unified secret store.
fn telegram_token() -> Result<String, String> {
    crate::secrets::read_secret("telegram").map_err(|_| {
        format!(
            "telegram: bot token not found. Message @BotFather on Telegram to create a bot, \
             then either export TELEGRAM_BOT_TOKEN or save it to {}",
            crate::secrets::secret_file_path("telegram").display()
        )
    })
}

/// Extract `chat_id` from a JSON value, accepting both string and number.
/// The model often passes `chat_id` as a number (e.g. `123456789`) rather
/// than a string, so we handle both.
fn tg_extract_chat_id(v: &Value, tool: &str) -> Result<String, String> {
    if let Some(s) = v.get("chat_id").and_then(Value::as_str) {
        return Ok(s.to_string());
    }
    if let Some(n) = v.get("chat_id").and_then(Value::as_i64) {
        return Ok(n.to_string());
    }
    Err(format!("{tool}: missing 'chat_id' (string or number)"))
}

/// Base URL for the Telegram Bot API.
fn tg_api_url(token: &str) -> String {
    format!("https://api.telegram.org/bot{token}")
}

/// `tg_send` — send a text message to a chat.
fn run_tg_send(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tg_send")?;
    // chat_id can be a string or number — the model often passes it as a number.
    let chat_id = tg_extract_chat_id(&v, "tg_send")?;
    let text = extract_str(&v, "text", "tg_send")?;

    let token = telegram_token()?;
    let client = external_http_client()?;
    let resp = client
        .post(format!("{}/sendMessage", tg_api_url(&token)))
        .json(&json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "Markdown",
        }))
        .send()
        .map_err(|e| format!("tg_send: request failed: {e}"))?;

    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!("tg_send: HTTP error: {body}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tg_send: parse failed: {e}"))?;

    let message_id = data
        .pointer("/result/message_id")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    Ok(json!({
        "ok": true,
        "message_id": message_id,
        "chat_id": chat_id,
    })
    .to_string())
}

/// `tg_get_updates` — poll recent messages/commands sent to the bot.
fn run_tg_get_updates(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tg_get_updates")?;
    let limit = v
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(10)
        .clamp(1, 100);
    let offset = v.get("offset").and_then(Value::as_i64);

    let token = telegram_token()?;
    let client = external_http_client()?;

    let mut params = vec![("limit", limit.to_string())];
    if let Some(off) = offset {
        params.push(("offset", off.to_string()));
    }

    let resp = client
        .get(format!("{}/getUpdates", tg_api_url(&token)))
        .query(&params)
        .send()
        .map_err(|e| format!("tg_get_updates: request failed: {e}"))?;

    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!("tg_get_updates: HTTP error: {body}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tg_get_updates: parse failed: {e}"))?;

    let updates = data
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Compact each update into a user-friendly shape.
    let results: Vec<Value> = updates
        .iter()
        .filter_map(|u| {
            let update_id = u.get("update_id").and_then(Value::as_i64)?;
            let msg = u.get("message")?;
            let from = msg
                .pointer("/from/first_name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let username = msg
                .pointer("/from/username")
                .and_then(Value::as_str)
                .unwrap_or("");
            let chat_id = msg.pointer("/chat/id").and_then(Value::as_i64)?;
            let text = msg
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("[non-text message]");
            let date = msg.get("date").and_then(Value::as_i64).unwrap_or(0);
            Some(json!({
                "update_id": update_id,
                "chat_id": chat_id,
                "from": from,
                "username": username,
                "text": text,
                "date": date,
            }))
        })
        .collect();

    Ok(json!({
        "count": results.len(),
        "updates": results,
    })
    .to_string())
}

/// `tg_send_photo` — send a photo by URL to a chat.
fn run_tg_send_photo(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tg_send_photo")?;
    let chat_id = tg_extract_chat_id(&v, "tg_send_photo")?;
    let url = extract_str(&v, "url", "tg_send_photo")?;
    let caption = v.get("caption").and_then(Value::as_str).unwrap_or("");

    let token = telegram_token()?;
    let client = external_http_client()?;

    let mut body = json!({
        "chat_id": chat_id,
        "photo": url,
    });
    if !caption.is_empty() {
        body["caption"] = json!(caption);
        body["parse_mode"] = json!("Markdown");
    }

    let resp = client
        .post(format!("{}/sendPhoto", tg_api_url(&token)))
        .json(&body)
        .send()
        .map_err(|e| format!("tg_send_photo: request failed: {e}"))?;

    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!("tg_send_photo: HTTP error: {body}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tg_send_photo: parse failed: {e}"))?;

    let message_id = data
        .pointer("/result/message_id")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    Ok(json!({
        "ok": true,
        "message_id": message_id,
        "chat_id": chat_id,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tg_send_rejects_missing_chat_id() {
        let err = run_tg_send(r#"{"text":"hello"}"#).unwrap_err();
        assert!(err.contains("chat_id"), "got: {err}");
    }

    #[test]
    fn tg_send_rejects_missing_text() {
        let err = run_tg_send(r#"{"chat_id":"123"}"#).unwrap_err();
        assert!(err.contains("text"), "got: {err}");
    }

    #[test]
    fn tg_send_photo_rejects_missing_url() {
        let err = run_tg_send_photo(r#"{"chat_id":"123"}"#).unwrap_err();
        assert!(err.contains("url"), "got: {err}");
    }

    #[test]
    fn tg_send_photo_rejects_missing_chat_id() {
        let err = run_tg_send_photo(r#"{"url":"https://example.com/img.jpg"}"#).unwrap_err();
        assert!(err.contains("chat_id"), "got: {err}");
    }

    #[test]
    fn telegram_token_error_mentions_botfather() {
        // If neither env var nor file is set, error should guide the user.
        let result = telegram_token();
        if let Err(msg) = result {
            assert!(msg.contains("BotFather"), "got: {msg}");
            assert!(msg.contains("telegram.token"), "got: {msg}");
        }
    }

    #[test]
    fn schemas_lists_three_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 3);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["tg_send", "tg_get_updates", "tg_send_photo"]);
    }
}
