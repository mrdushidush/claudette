//! Telegram bot mode — Claudette as a long-running Telegram bot.
//!
//! `claudette --telegram` starts a polling loop that reads messages from
//! the Telegram Bot API, feeds them through the same `ConversationRuntime`
//! used by the REPL, and sends responses back. Session persistence, auto-
//! compaction, and tool groups all work exactly as in the REPL.
//!
//! Security: only `chat_id`s in the allow-list are served. Set via
//! `--chat <id>` flag or `CLAUDETTE_TELEGRAM_CHAT` env var.

use std::time::Duration;

use anyhow::{Context, Result};
use crate::{ContentBlock, Session};
use serde_json::{json, Value};

use crate::run::{
    build_runtime_streaming, maybe_compact_session, save_session,
    try_load_session,
};
use crate::secrets::{read_secret, save_chat_id};
use crate::theme;
use crate::tts;
use crate::voice;

/// Telegram message size limit. Messages longer than this are split.
const TG_MAX_MESSAGE_LEN: usize = 4000;

/// Polling interval between `getUpdates` calls.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Run Claudette as a Telegram bot. Blocks forever (until Ctrl-C).
pub fn run_telegram_bot(allowed_chat_ids: Vec<i64>, resume: bool) -> Result<()> {
    let token = read_secret("telegram").map_err(|e| anyhow::anyhow!(e))?;
    let base_url = format!("https://api.telegram.org/bot{token}");

    // Verify the token works.
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let me: Value = http
        .get(format!("{base_url}/getMe"))
        .send()?
        .json()
        .context("failed to parse getMe response")?;
    let bot_name = me
        .pointer("/result/username")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    eprintln!(
        "{} {} {}",
        theme::ROBOT,
        theme::brand("telegram bot mode"),
        theme::dim(&format!("@{bot_name}"))
    );

    if allowed_chat_ids.is_empty() {
        eprintln!(
            "{} {}",
            theme::warn(theme::WARN_GLYPH),
            theme::warn(
                "no --chat filter set — will serve ALL incoming chats. \
                 Use --chat <id> to restrict."
            )
        );
    } else {
        eprintln!(
            "{} {}",
            theme::SPARKLES,
            theme::dim(&format!(
                "serving chat IDs: {:?}",
                allowed_chat_ids
            ))
        );
    }

    // Check voice transcription dependencies.
    match voice::check_voice_deps() {
        Ok(()) => eprintln!(
            "{} {}",
            theme::SPARKLES,
            theme::ok("voice transcription ready (ffmpeg + whisper)")
        ),
        Err(e) => eprintln!(
            "{} {}",
            theme::dim("○"),
            theme::dim(&format!("voice transcription disabled — {e}"))
        ),
    }

    // Check TTS dependencies.
    let tts_available = tts::check_tts_deps().is_ok();
    if tts_available {
        eprintln!(
            "{} {}",
            theme::SPARKLES,
            theme::ok("voice output ready (edge-tts)")
        );
    } else {
        eprintln!(
            "{} {}",
            theme::dim("○"),
            theme::dim("voice output disabled — install with: pip install edge-tts")
        );
    }

    // Load or create session.
    let session = if resume {
        match try_load_session()? {
            Some(s) => {
                eprintln!(
                    "{} {}",
                    theme::SAVE,
                    theme::ok(&format!(
                        "resumed session ({} messages)",
                        s.messages.len()
                    ))
                );
                s
            }
            None => {
                eprintln!(
                    "{} {}",
                    theme::dim("○"),
                    theme::dim("no saved session — starting fresh")
                );
                Session::default()
            }
        }
    } else {
        Session::default()
    };

    let mut runtime = build_runtime_streaming(session, true);
    let mut last_update_id: i64 = 0;
    // Current transcription/response language. "en" = English (default),
    // "he" = Hebrew. Switch with /lang he or /lang en.
    let mut voice_lang = "en".to_string();
    // TTS enabled — only if edge-tts is available. Toggle with /voice.
    let mut tts_enabled = tts_available;

    eprintln!(
        "{} {}",
        theme::BOLT,
        theme::ok("polling for messages... (Ctrl-C to stop)")
    );

    loop {
        match poll_updates(&http, &base_url, last_update_id + 1) {
            Ok(updates) => {
                for update in updates {
                    let update_id = update
                        .get("update_id")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    if update_id > last_update_id {
                        last_update_id = update_id;
                    }

                    let Some(message) = update.get("message") else {
                        continue;
                    };
                    let chat_id = message
                        .pointer("/chat/id")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    let from = message
                        .pointer("/from/first_name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");

                    // Security: skip unauthorized chats.
                    if !allowed_chat_ids.is_empty() && !allowed_chat_ids.contains(&chat_id) {
                        eprintln!(
                            "  {} {}",
                            theme::dim("○"),
                            theme::dim(&format!(
                                "ignoring message from unauthorized chat {chat_id} ({from})"
                            ))
                        );
                        continue;
                    }

                    // Extract text from message — either typed text or voice transcription.
                    let text: String = if let Some(voice_obj) = message.get("voice") {
                        // Voice message — download and transcribe via Whisper.
                        let file_id = voice_obj
                            .get("file_id")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if file_id.is_empty() {
                            continue;
                        }
                        eprintln!(
                            "\n  {} {} {}",
                            theme::accent("←"),
                            theme::accent(from),
                            theme::dim("[voice message]")
                        );
                        match voice::transcribe_telegram_voice(&http, &base_url, file_id, &voice_lang)
                        {
                            Ok(transcript) => {
                                eprintln!(
                                    "  {} {}",
                                    theme::dim("▸"),
                                    theme::dim(&format!(
                                        "transcribed: {}",
                                        transcript.chars().take(80).collect::<String>()
                                    ))
                                );
                                transcript
                            }
                            Err(e) => {
                                eprintln!(
                                    "  {} {}",
                                    theme::error(theme::ERR_GLYPH),
                                    theme::error(&format!("voice transcription failed: {e}"))
                                );
                                let _ = send_message(
                                    &http,
                                    &base_url,
                                    chat_id,
                                    &format!(
                                        "Sorry, I couldn't transcribe your voice message: {e}"
                                    ),
                                );
                                continue;
                            }
                        }
                    } else {
                        // Regular text message.
                        let t = message
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if t.is_empty() {
                            continue;
                        }
                        eprintln!(
                            "\n  {} {} {}",
                            theme::accent("←"),
                            theme::accent(from),
                            theme::dim(&t)
                        );
                        t
                    };

                    // Handle bot slash commands directly (not sent to model).
                    if text.starts_with('/') {
                        let reply = match text.as_str() {
                            "/start" => Some(
                                "Hello! I'm Claudette, your AI personal secretary. \
                                 Send me any message and I'll help you out."
                                    .to_string(),
                            ),
                            "/compact" => {
                                match maybe_compact_session(&mut runtime, true) {
                                    Some(removed) => {
                                        let _ = save_session(runtime.session());
                                        Some(format!("Compacted {removed} older messages."))
                                    }
                                    None => Some("Nothing to compact yet.".to_string()),
                                }
                            }
                            "/clear" => {
                                runtime =
                                    build_runtime_streaming(Session::default(), true);
                                Some("Session cleared.".to_string())
                            }
                            "/status" => {
                                let msgs = runtime.session().messages.len();
                                let est =
                                    crate::estimate_session_tokens(runtime.session());
                                Some(format!(
                                    "Messages: {msgs}\nEstimated tokens: {est}\n\
                                     Compact threshold: {}",
                                    crate::run::compact_threshold()
                                ))
                            }
                            "/voice" => {
                                if !tts_available {
                                    Some("Voice output unavailable — run: pip install edge-tts".to_string())
                                } else {
                                    tts_enabled = !tts_enabled;
                                    if tts_enabled {
                                        Some(format!("Voice output ON ({})", tts::voice_for_lang(&voice_lang)))
                                    } else {
                                        Some("Voice output OFF.".to_string())
                                    }
                                }
                            }
                            cmd if cmd.starts_with("/lang") => {
                                let arg = cmd
                                    .strip_prefix("/lang")
                                    .unwrap_or("")
                                    .trim()
                                    .to_lowercase();
                                match arg.as_str() {
                                    "he" | "hebrew" | "עברית" => {
                                        voice_lang = "he".to_string();
                                        Some("Language set to Hebrew. Voice messages will be transcribed and answered in Hebrew. Use /lang en to switch back.".to_string())
                                    }
                                    "en" | "english" | "" => {
                                        voice_lang = "en".to_string();
                                        Some("Language set to English (default). Voice messages will be translated to English. Use /lang he for Hebrew.".to_string())
                                    }
                                    other => Some(format!(
                                        "Unknown language '{other}'. Use /lang en or /lang he."
                                    )),
                                }
                            }
                            _ => None, // Unknown slash command — send to model.
                        };
                        if let Some(msg) = reply {
                            let _ = send_message(&http, &base_url, chat_id, &msg);
                            continue;
                        }
                    }

                    // Snapshot session so we can roll back on failure.
                    let session_snapshot = runtime.session().clone();

                    // Sprint 14: route through brain_selector so Auto-preset
                    // turns escalate to the fallback brain on stuck signals.
                    // Telegram has no prompter (every tool must be auto-OK)
                    // so we pass a permanently-None option.
                    let mut no_prompter: Option<&mut dyn crate::PermissionPrompter> = None;
                    match crate::brain_selector::run_turn_with_fallback(
                        &mut runtime,
                        &text,
                        &mut no_prompter,
                    ) {
                        Ok(summary) => {
                            let response = extract_response_text(&summary);

                            eprintln!(
                                "  {} {} {}",
                                theme::accent("→"),
                                theme::dim(&format!(
                                    "iter={} in={} out={}",
                                    summary.iterations,
                                    summary.usage.input_tokens,
                                    summary.usage.output_tokens,
                                )),
                                theme::dim(
                                    &response.chars().take(80).collect::<String>()
                                )
                            );

                            // Send text response (split if too long).
                            for chunk in split_message(&response, TG_MAX_MESSAGE_LEN) {
                                if let Err(e) =
                                    send_message(&http, &base_url, chat_id, chunk)
                                {
                                    eprintln!(
                                        "  {} {}",
                                        theme::error(theme::ERR_GLYPH),
                                        theme::error(&format!("send failed: {e}"))
                                    );
                                }
                            }

                            // Send voice response if TTS is enabled.
                            if tts_enabled {
                                if let Some(ogg_path) = tts::synthesize(&response, &voice_lang) {
                                    eprintln!(
                                        "  {} {}",
                                        theme::dim("▸"),
                                        theme::dim("sending voice response...")
                                    );
                                    if let Err(e) = tts::send_voice_message(
                                        &http, &base_url, chat_id, &ogg_path,
                                    ) {
                                        eprintln!(
                                            "  {} {}",
                                            theme::warn(theme::WARN_GLYPH),
                                            theme::warn(&format!("TTS send failed: {e}"))
                                        );
                                    }
                                    let _ = std::fs::remove_file(&ogg_path);
                                }
                            }

                            // Auto-compact if needed.
                            if let Some(removed) = maybe_compact_session(&mut runtime, true) {
                                eprintln!(
                                    "  {} {}",
                                    theme::SAVE,
                                    theme::ok(&format!(
                                        "auto-compacted {removed} older message(s)"
                                    ))
                                );
                            }

                            // Auto-save.
                            if let Err(e) = save_session(runtime.session()) {
                                eprintln!(
                                    "  {} {}",
                                    theme::warn(theme::WARN_GLYPH),
                                    theme::warn(&format!("session save failed: {e:#}"))
                                );
                            }

                            // Persist chat ID for future runs.
                            save_chat_id(chat_id);
                        }
                        Err(e) => {
                            eprintln!(
                                "  {} {}",
                                theme::error(theme::ERR_GLYPH),
                                theme::error(&format!("turn failed: {e}"))
                            );
                            // Roll back session to prevent corruption from
                            // partial messages left by the failed turn.
                            runtime = build_runtime_streaming(session_snapshot, true);
                            eprintln!(
                                "  {} {}",
                                theme::dim("▸"),
                                theme::dim("session rolled back to pre-turn state")
                            );
                            let _ = send_message(
                                &http,
                                &base_url,
                                chat_id,
                                &format!("Sorry, I encountered an error: {e}"),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "  {} {}",
                    theme::warn(theme::WARN_GLYPH),
                    theme::warn(&format!("poll error: {e} — retrying..."))
                );
            }
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Poll Telegram for new updates.
fn poll_updates(
    http: &reqwest::blocking::Client,
    base_url: &str,
    offset: i64,
) -> Result<Vec<Value>> {
    let resp: Value = http
        .get(format!("{base_url}/getUpdates"))
        .query(&[
            ("offset", offset.to_string()),
            ("limit", "10".to_string()),
            ("timeout", "1".to_string()),
        ])
        .send()?
        .json()
        .context("failed to parse getUpdates")?;

    Ok(resp
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// Send a text message via Telegram.
fn send_message(
    http: &reqwest::blocking::Client,
    base_url: &str,
    chat_id: i64,
    text: &str,
) -> Result<()> {
    let resp = http
        .post(format!("{base_url}/sendMessage"))
        .json(&json!({
            "chat_id": chat_id,
            "text": text,
        }))
        .send()?;

    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        anyhow::bail!("Telegram API error: {body}");
    }
    Ok(())
}

/// Extract the final text response from a turn summary.
fn extract_response_text(summary: &crate::TurnSummary) -> String {
    let mut texts = Vec::new();
    for msg in &summary.assistant_messages {
        for block in &msg.blocks {
            if let ContentBlock::Text { text } = block {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    texts.push(trimmed.to_string());
                }
            }
        }
    }
    if texts.is_empty() {
        "(I processed your request but have no text to show.)".to_string()
    } else {
        texts.join("\n\n")
    }
}

/// Split a message into chunks that fit Telegram's size limit.
fn split_message(text: &str, max_len: usize) -> Vec<&str> {
    if text.len() <= max_len {
        return vec![text];
    }
    let mut chunks = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining);
            break;
        }
        // Try to split at a newline boundary.
        let split_at = remaining[..max_len]
            .rfind('\n')
            .unwrap_or(max_len);
        let (chunk, rest) = remaining.split_at(split_at);
        chunks.push(chunk);
        remaining = rest.trim_start_matches('\n');
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_message_short() {
        let chunks = split_message("hello", 100);
        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn split_message_at_newline() {
        let text = "line one\nline two\nline three";
        let chunks = split_message(text, 15);
        assert_eq!(chunks[0], "line one");
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn split_message_no_newline() {
        let text = "a".repeat(100);
        let chunks = split_message(&text, 30);
        assert!(chunks.len() >= 4);
        for chunk in &chunks {
            assert!(chunk.len() <= 30);
        }
    }

    #[test]
    fn extract_response_text_empty() {
        let summary = crate::TurnSummary {
            assistant_messages: vec![],
            tool_results: vec![],
            iterations: 0,
            usage: crate::TokenUsage::default(),
            auto_compaction: None,
        };
        let text = extract_response_text(&summary);
        assert!(text.contains("no text"));
    }
}
