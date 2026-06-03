//! Telegram bot group — 1 polymorphic tool against the Bot API. Token comes
//! from `crate::secrets::read_secret("telegram")`
//! (CLAUDETTE_TELEGRAM_TOKEN / TELEGRAM_BOT_TOKEN env or
//! `~/.claudette/secrets/telegram.token`).
//!
//! Sprint v0.6.0 (2026-05-21) decom:
//!  - dropped `tg_get_updates` — making it model-callable was a
//!    prompt-injection footgun (a hostile incoming message could appear
//!    inside the tool result and steer the model). The bot loop still
//!    polls at the transport layer in [`crate::run`]; the model just
//!    doesn't get to drive that polling itself.
//!  - merged `tg_send_photo` into `tg_send` via an optional `photo` arg
//!    (URL). When `photo` is set, `text` becomes the caption and the
//!    request hits `/sendPhoto` instead of `/sendMessage`.
//!
//! Self-contained: all helpers (`telegram_token`, `tg_extract_chat_id`,
//! `tg_api_url`) are private to this module. Handlers reuse the pub(super)
//! parent helpers `parse_json_input`, `extract_str`, `external_http_client`.

use serde_json::{json, Value};

use super::{external_http_client, extract_str, parse_json_input};

pub(super) fn schemas() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "tg_send",
            "description": "Send a message via Telegram bot. Pass `photo` (URL) to send an image instead — `text` becomes the caption.",
            "parameters": {
                "type": "object",
                "properties": {
                    "chat_id": { "type": "string", "description": "Telegram chat ID (user or group)" },
                    "text":    { "type": "string", "description": "Message text or photo caption (supports Markdown)" },
                    "photo":   { "type": "string", "description": "Optional: public URL of an image to send. When set, the message is sent as a photo with `text` as caption." }
                },
                "required": ["chat_id", "text"]
            }
        }
    })]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    if name != "tg_send" {
        return None;
    }
    // tg_send relays through api.telegram.org (cloud); block under offline
    // mode before any request leaves the process.
    if let Err(e) = crate::egress::guard("https://api.telegram.org") {
        return Some(Err(e));
    }
    Some(run_tg_send(input))
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

/// `tg_send` — send a text message, or a photo with caption when `photo` is
/// supplied. Single entry point for both `/sendMessage` and `/sendPhoto`.
fn run_tg_send(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tg_send")?;
    let chat_id = tg_extract_chat_id(&v, "tg_send")?;
    let text = extract_str(&v, "text", "tg_send")?;
    let photo = v
        .get("photo")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());

    let token = telegram_token()?;
    let client = external_http_client()?;

    let (endpoint, body) = if let Some(url) = photo {
        let mut body = json!({
            "chat_id": chat_id,
            "photo": url,
        });
        if !text.is_empty() {
            body["caption"] = json!(text);
            body["parse_mode"] = json!("Markdown");
        }
        ("sendPhoto", body)
    } else {
        (
            "sendMessage",
            json!({
                "chat_id": chat_id,
                "text": text,
                "parse_mode": "Markdown",
            }),
        )
    };

    let resp = client
        .post(format!("{}/{endpoint}", tg_api_url(&token)))
        .json(&body)
        .send()
        .map_err(|e| format!("tg_send: request failed: {e}"))?;

    if !resp.status().is_success() {
        let err_body = resp.text().unwrap_or_default();
        return Err(format!("tg_send: HTTP error: {err_body}"));
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
    fn telegram_token_error_mentions_botfather() {
        // If neither env var nor file is set, error should guide the user.
        let result = telegram_token();
        if let Err(msg) = result {
            assert!(msg.contains("BotFather"), "got: {msg}");
            assert!(msg.contains("telegram.token"), "got: {msg}");
        }
    }

    #[test]
    fn schemas_lists_one_tool() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 1);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["tg_send"]);
    }
}
