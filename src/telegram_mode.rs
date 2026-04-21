//! Telegram bot mode — Claudette as a long-running Telegram bot.
//!
//! `claudette --telegram` starts a polling loop that reads messages from
//! the Telegram Bot API, feeds them through the same `ConversationRuntime`
//! used by the REPL, and sends responses back. Session persistence, auto-
//! compaction, and tool groups all work exactly as in the REPL.
//!
//! Security: only `chat_id`s in the allow-list are served. Set via
//! `--chat <id>` flag or `CLAUDETTE_TELEGRAM_CHAT` env var.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use crate::{ContentBlock, Session};
use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::clock::SystemClock;
use crate::run::{build_runtime_streaming, maybe_compact_session, save_session, try_load_session};
use crate::scheduler::{self, Firing, Scheduler};
use crate::secrets::{read_secret, save_chat_id};
use crate::theme;
use crate::tts;
use crate::voice;

/// One event the single-consumer main loop processes. AD-1: two producer
/// threads (Telegram getUpdates poller + scheduler tick) feed this channel,
/// so the consumer has exclusive `&mut` ownership of the runtime and a
/// mid-turn user message never races with a scheduled firing.
enum Event {
    /// A raw `message` object from Telegram's getUpdates (not the wrapping
    /// update record — the update_id is stripped by the poller).
    TgUpdate(Value),
    /// A scheduled entry came due. Treated by the consumer as if the user
    /// had just typed `prompt` in `chat_id`.
    Scheduled {
        prompt: String,
        chat_id: i64,
        entry_id: String,
        scheduled_for: chrono::DateTime<chrono::Utc>,
    },
}

/// How often the scheduler tick thread wakes to check for due firings.
const SCHEDULER_TICK: Duration = Duration::from_secs(1);

/// Telegram message size limit. Messages longer than this are split.
const TG_MAX_MESSAGE_LEN: usize = 4000;

/// Polling interval between `getUpdates` calls.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Minimum dwell before a paragraph lands. Also the time the "typing…"
/// bubble is visible for very short paragraphs (one-liners, confirmations).
const PARAGRAPH_PACING_MIN: Duration = Duration::from_millis(2000);

/// Upper bound on adaptive pacing. A single paragraph should never stall
/// longer than this — if someone wanted that much delay they'd just stop
/// reading.
const PARAGRAPH_PACING_MAX: Duration = Duration::from_millis(8000);

/// Extra delay per character in the upcoming paragraph. 15ms/char works out
/// to roughly real reading speed (~250 wpm ≈ 8ms/char) plus enough cushion
/// that the reader finishes a beat before the next message appears. At a
/// typical ~70-char paragraph this lands the pacing around 3 seconds.
const PARAGRAPH_PACING_PER_CHAR_MS: u64 = 15;

/// Adaptive pacing: longer paragraphs get more dwell time so the reader can
/// actually finish the previous one before the next bubble lands.
fn paragraph_pacing(text: &str) -> Duration {
    let chars = text.chars().count() as u64;
    let dwell = PARAGRAPH_PACING_MIN.as_millis() as u64 + chars * PARAGRAPH_PACING_PER_CHAR_MS;
    let cap = PARAGRAPH_PACING_MAX.as_millis() as u64;
    Duration::from_millis(dwell.min(cap))
}

/// Paragraphs shorter than this merge forward into the next one, so a
/// single short reply doesn't get spread across multiple messages.
const PARAGRAPH_MERGE_THRESHOLD: usize = 80;

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
            theme::dim(&format!("serving chat IDs: {:?}", allowed_chat_ids))
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
                    theme::ok(&format!("resumed session ({} messages)", s.messages.len()))
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
    // Current transcription/response language. "en" = English (default),
    // "he" = Hebrew. Switch with /lang he or /lang en.
    let mut voice_lang = "en".to_string();
    // TTS enabled — only if edge-tts is available. Toggle with /voice.
    let mut tts_enabled = tts_available;

    // ── Scheduler bootstrap (AD-1 / AD-4) ─────────────────────────────
    // Load persisted schedule, apply catch-up policy. Immediate firings
    // (entries we missed while offline) are queued into the channel
    // before the Telegram poller even starts, so they dispatch on the
    // consumer's first pass.
    let default_scheduled_chat = allowed_chat_ids.first().copied();
    let scheduler_path = scheduler::default_path();
    let clock: Arc<dyn crate::clock::Clock> = Arc::new(SystemClock);
    let catch_up_firings: Vec<Firing> =
        match Scheduler::load(scheduler_path.clone(), clock.clone()) {
            Ok((loaded, firings)) => {
                eprintln!(
                    "{} {}",
                    theme::SAVE,
                    theme::ok(&format!(
                        "scheduler loaded ({} active, {} catch-up)",
                        loaded.list().len(),
                        firings.len(),
                    ))
                );
                scheduler::install(loaded);
                firings
            }
            Err(e) => {
                eprintln!(
                    "{} {}",
                    theme::warn(theme::WARN_GLYPH),
                    theme::warn(&format!("scheduler load failed: {e} — starting empty"))
                );
                scheduler::install(Scheduler::new(scheduler_path, clock));
                Vec::new()
            }
        };

    // ── mpsc channel: one consumer, two producers ──────────────────────
    let (tx, rx) = mpsc::channel::<Event>();

    // Queue catch-up firings before the producers start so they land
    // first in FIFO order.
    for firing in catch_up_firings {
        let chat_id = firing.chat_id.or(default_scheduled_chat);
        if let Some(chat_id) = chat_id {
            let _ = tx.send(Event::Scheduled {
                prompt: firing.prompt,
                chat_id,
                entry_id: firing.entry_id,
                scheduled_for: firing.scheduled_for,
            });
        }
    }

    // Producer 1: Telegram getUpdates poller. Owns `last_update_id` and
    // forwards `message` objects (not updates) into the channel.
    let tx_tg = tx.clone();
    let http_tg = http.clone();
    let base_tg = base_url.clone();
    std::thread::spawn(move || {
        let mut last_update_id: i64 = 0;
        loop {
            match poll_updates(&http_tg, &base_tg, last_update_id + 1) {
                Ok(updates) => {
                    for update in updates {
                        let update_id =
                            update.get("update_id").and_then(Value::as_i64).unwrap_or(0);
                        if update_id > last_update_id {
                            last_update_id = update_id;
                        }
                        if let Some(message) = update.get("message").cloned() {
                            if tx_tg.send(Event::TgUpdate(message)).is_err() {
                                return; // consumer gone
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
    });

    // Producer 2: scheduler tick. Every SCHEDULER_TICK, locks the global
    // scheduler, drains due firings, sends them as Events. Firings with
    // no chat_id fall back to the first allowed chat so a scheduled
    // briefing goes somewhere rather than silently dropping.
    let tx_sch = tx.clone();
    std::thread::spawn(move || loop {
        let firings: Vec<Firing> = match scheduler::global().lock() {
            Ok(mut g) => g.fire_due().unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        for firing in firings {
            let chat_id = firing.chat_id.or(default_scheduled_chat);
            let Some(chat_id) = chat_id else {
                eprintln!(
                    "  {} {}",
                    theme::warn(theme::WARN_GLYPH),
                    theme::warn(&format!(
                        "scheduled firing '{}' has no chat_id and no default; dropping",
                        firing.entry_id
                    ))
                );
                continue;
            };
            if tx_sch
                .send(Event::Scheduled {
                    prompt: firing.prompt,
                    chat_id,
                    entry_id: firing.entry_id,
                    scheduled_for: firing.scheduled_for,
                })
                .is_err()
            {
                return;
            }
        }
        std::thread::sleep(SCHEDULER_TICK);
    });

    drop(tx); // so channel closes iff every producer thread exits

    eprintln!(
        "{} {}",
        theme::BOLT,
        theme::ok("polling for messages... (Ctrl-C to stop)")
    );

    // ── Consumer: single-owner of `runtime`. Events serialise through
    //    the channel so a scheduled firing can never interleave with a
    //    mid-turn user message.
    while let Ok(event) = rx.recv() {
        match event {
            Event::Scheduled {
                prompt,
                chat_id,
                entry_id,
                scheduled_for,
            } => {
                if !allowed_chat_ids.is_empty() && !allowed_chat_ids.contains(&chat_id) {
                    eprintln!(
                        "  {} {}",
                        theme::dim("○"),
                        theme::dim(&format!(
                            "dropping scheduled firing to unauthorized chat {chat_id}"
                        ))
                    );
                    continue;
                }
                eprintln!(
                    "\n  {} {} {}",
                    theme::accent("⏰"),
                    theme::accent(&entry_id),
                    theme::dim(&format!(
                        "scheduled_for={} prompt={}",
                        scheduled_for.to_rfc3339(),
                        prompt.chars().take(60).collect::<String>()
                    ))
                );
                run_synthetic_turn(
                    &http,
                    &base_url,
                    &mut runtime,
                    chat_id,
                    &prompt,
                    &voice_lang,
                );
                if let Err(e) = save_session(runtime.session()) {
                    eprintln!(
                        "  {} {}",
                        theme::warn(theme::WARN_GLYPH),
                        theme::warn(&format!("session save failed: {e:#}"))
                    );
                }
            }
            Event::TgUpdate(message) => {
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

                    // Extract text from message — either typed text or voice
                    // transcription. Track which one it was so the reply
                    // mode (text-only vs text+voice) can match the input.
                    let input_was_voice = message.get("voice").is_some();
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
                        match voice::transcribe_telegram_voice(
                            &http,
                            &base_url,
                            file_id,
                            &voice_lang,
                        ) {
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
                            "/compact" => match maybe_compact_session(&mut runtime, true) {
                                Some(removed) => {
                                    let _ = save_session(runtime.session());
                                    Some(format!("Compacted {removed} older messages."))
                                }
                                None => Some("Nothing to compact yet.".to_string()),
                            },
                            "/clear" => {
                                runtime = build_runtime_streaming(Session::default(), true);
                                Some("Session cleared.".to_string())
                            }
                            "/status" => {
                                let msgs = runtime.session().messages.len();
                                let est = crate::estimate_session_tokens(runtime.session());
                                Some(format!(
                                    "Messages: {msgs}\nEstimated tokens: {est}\n\
                                     Compact threshold: {}",
                                    crate::run::compact_threshold()
                                ))
                            }
                            "/voice" => {
                                if !tts_available {
                                    Some(
                                        "Voice output unavailable — run: pip install edge-tts"
                                            .to_string(),
                                    )
                                } else {
                                    tts_enabled = !tts_enabled;
                                    if tts_enabled {
                                        Some(format!(
                                            "Voice output ON ({})",
                                            tts::voice_for_lang(&voice_lang)
                                        ))
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

                    // Show typing while the model is thinking so the user
                    // gets immediate feedback that the message was received.
                    send_typing(&http, &base_url, chat_id);

                    // Start a streaming paragraph poller. The brain's
                    // text-delta callback appends tokens to the global
                    // `telegram_stream_buffer`; this poller reads from that
                    // buffer, detects completed paragraphs (blank-line
                    // boundaries outside code fences), and sends each one
                    // progressively. Net effect: paragraphs arrive in the
                    // chat as Claudette writes them, not all at the end.
                    crate::api::telegram_stream_reset();
                    let active = Arc::new(AtomicBool::new(true));
                    let poller_http = http.clone();
                    let poller_base = base_url.clone();
                    let poller_active = active.clone();
                    let poller = std::thread::spawn(move || {
                        run_streaming_poller(
                            poller_http,
                            poller_base,
                            chat_id,
                            poller_active,
                        )
                    });

                    // Sprint 14: route through brain_selector so Auto-preset
                    // turns escalate to the fallback brain on stuck signals.
                    // Telegram has no prompter (every tool must be auto-OK)
                    // so we pass a permanently-None option.
                    let mut no_prompter: Option<&mut dyn crate::PermissionPrompter> = None;
                    let turn_result = crate::brain_selector::run_turn_with_fallback(
                        &mut runtime,
                        &text,
                        &mut no_prompter,
                    );

                    // Signal poller to flush and exit, then wait for it.
                    active.store(false, Ordering::SeqCst);
                    let streamed_bytes = poller.join().unwrap_or(0);

                    match turn_result {
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
                                theme::dim(&response.chars().take(80).collect::<String>())
                            );

                            // If the streaming poller didn't emit anything
                            // (tool-only turn, or callback never fired), fall
                            // back to the classic paragraph-split-and-send so
                            // the user still gets a reply.
                            if streamed_bytes == 0 && !response.trim().is_empty() {
                                let paragraphs =
                                    split_into_paragraphs(&response, TG_MAX_MESSAGE_LEN);
                                for chunk in &paragraphs {
                                    send_typing(&http, &base_url, chat_id);
                                    std::thread::sleep(paragraph_pacing(chunk));
                                    if let Err(e) = send_message(&http, &base_url, chat_id, chunk) {
                                        eprintln!(
                                            "  {} {}",
                                            theme::error(theme::ERR_GLYPH),
                                            theme::error(&format!("send failed: {e}"))
                                        );
                                    }
                                }
                            }

                            // Send voice response only when the input was
                            // also voice. Text-in → text-out keeps typed
                            // chats fast; voice-in → voice-out preserves
                            // the hands-free experience. TTS must also be
                            // enabled (master toggle via /voice).
                            if tts_enabled && input_was_voice {
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
                } // end Event::TgUpdate arm
            } // end match event
    } // end while let Ok(event)
    Ok(())
}

/// Run a synthetic turn on behalf of the scheduler — feed `prompt` into the
/// runtime as if the user had typed it, then stream the response to
/// `chat_id` with the same paragraph-pacing poller the live path uses.
/// Scheduled turns never play TTS (they're proactive pings, not voice
/// conversations) and don't apply the permission prompter (scheduled
/// entries are pre-approved by virtue of the user having created them).
fn run_synthetic_turn(
    http: &reqwest::blocking::Client,
    base_url: &str,
    runtime: &mut crate::ConversationRuntime<crate::OllamaApiClient, crate::SecretaryToolExecutor>,
    chat_id: i64,
    prompt: &str,
    _voice_lang: &str,
) {
    let session_snapshot = runtime.session().clone();

    send_typing(http, base_url, chat_id);

    // Same streaming-poller contract as the Telegram path.
    crate::api::telegram_stream_reset();
    let active = Arc::new(AtomicBool::new(true));
    let poller_http = http.clone();
    let poller_base = base_url.to_string();
    let poller_active = active.clone();
    let poller = std::thread::spawn(move || {
        run_streaming_poller(poller_http, poller_base, chat_id, poller_active)
    });

    let mut no_prompter: Option<&mut dyn crate::PermissionPrompter> = None;
    let turn_result = crate::brain_selector::run_turn_with_fallback(runtime, prompt, &mut no_prompter);

    active.store(false, Ordering::SeqCst);
    let streamed_bytes = poller.join().unwrap_or(0);

    match turn_result {
        Ok(summary) => {
            let response = extract_response_text(&summary);
            if streamed_bytes == 0 && !response.trim().is_empty() {
                for chunk in split_into_paragraphs(&response, TG_MAX_MESSAGE_LEN) {
                    send_typing(http, base_url, chat_id);
                    std::thread::sleep(paragraph_pacing(&chunk));
                    if let Err(e) = send_message(http, base_url, chat_id, &chunk) {
                        eprintln!(
                            "  {} {}",
                            theme::error(theme::ERR_GLYPH),
                            theme::error(&format!("scheduled send failed: {e}"))
                        );
                    }
                }
            }
        }
        Err(e) => {
            eprintln!(
                "  {} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("scheduled turn failed: {e}"))
            );
            *runtime = build_runtime_streaming(session_snapshot, true);
            let _ = send_message(
                http,
                base_url,
                chat_id,
                &format!("Sorry, a scheduled reminder ran into an error: {e}"),
            );
        }
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

/// Streaming poller: while the brain is generating, watch the shared
/// `telegram_stream_buffer` for completed paragraph/code-fence units and
/// send each one as its own Telegram message. Returns the total number of
/// bytes successfully emitted — the caller uses this to decide whether it
/// also needs to fall back to the classic bulk send (zero bytes ⇒ no
/// streaming happened, probably a tool-only turn).
fn run_streaming_poller(
    http: reqwest::blocking::Client,
    base_url: String,
    chat_id: i64,
    active: Arc<AtomicBool>,
) -> usize {
    let buffer = crate::api::telegram_stream_buffer();
    let mut sent: usize = 0;
    loop {
        // Snapshot the buffer so we don't hold the lock across the HTTP
        // send (which takes seconds thanks to pacing).
        let snapshot = match buffer.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => String::new(),
        };

        if sent < snapshot.len() {
            let tail = &snapshot[sent..];
            if let Some(cut) = find_safe_cut(tail) {
                let paragraph = tail[..cut].trim_matches('\n').trim().to_string();
                sent += cut;
                if !paragraph.is_empty() {
                    for chunk in split_message(&paragraph, TG_MAX_MESSAGE_LEN) {
                        send_typing(&http, &base_url, chat_id);
                        std::thread::sleep(paragraph_pacing(chunk));
                        if let Err(e) = send_message(&http, &base_url, chat_id, chunk) {
                            eprintln!(
                                "  {} {}",
                                theme::error(theme::ERR_GLYPH),
                                theme::error(&format!("stream send failed: {e}"))
                            );
                        }
                    }
                }
                continue;
            }
        }

        if !active.load(Ordering::SeqCst) {
            // Turn finished — flush whatever is left as a final paragraph.
            if sent < snapshot.len() {
                let tail = snapshot[sent..].trim().to_string();
                sent = snapshot.len();
                if !tail.is_empty() {
                    for chunk in split_message(&tail, TG_MAX_MESSAGE_LEN) {
                        send_typing(&http, &base_url, chat_id);
                        std::thread::sleep(paragraph_pacing(chunk));
                        if let Err(e) = send_message(&http, &base_url, chat_id, chunk) {
                            eprintln!(
                                "  {} {}",
                                theme::error(theme::ERR_GLYPH),
                                theme::error(&format!("stream flush failed: {e}"))
                            );
                        }
                    }
                }
            }
            break;
        }

        std::thread::sleep(Duration::from_millis(150));
    }
    sent
}

/// Find the largest prefix of `text` that is safe to emit — i.e. that ends
/// exactly at a paragraph boundary (`\n\n` outside a code fence) or at the
/// closing ``` of a fenced code block. Returns `None` when nothing complete
/// is available yet (we're mid-paragraph or mid-fence).
fn find_safe_cut(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut safe: Option<usize> = None;
    let mut in_code = false;
    let mut i = 0;
    while i < bytes.len() {
        let at_line_start = i == 0 || bytes[i - 1] == b'\n';
        if at_line_start && bytes[i..].starts_with(b"```") {
            // Skip any leading spaces we already walked past — here we're
            // strictly at a line start. Find end of line.
            let line_end = bytes[i..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(bytes.len(), |p| i + p);
            if in_code {
                in_code = false;
                let after = if line_end < bytes.len() {
                    line_end + 1
                } else {
                    line_end
                };
                safe = Some(after);
                i = after;
            } else {
                in_code = true;
                i = if line_end < bytes.len() {
                    line_end + 1
                } else {
                    line_end
                };
            }
            continue;
        }
        if !in_code && bytes[i] == b'\n' && bytes.get(i + 1) == Some(&b'\n') {
            safe = Some(i + 2);
            i += 2;
            continue;
        }
        i += 1;
    }
    safe
}

/// Show a "typing…" indicator in the chat. Fire-and-forget: errors are
/// ignored because a missing indicator is cosmetic, not a failure.
fn send_typing(http: &reqwest::blocking::Client, base_url: &str, chat_id: i64) {
    let _ = http
        .post(format!("{base_url}/sendChatAction"))
        .json(&json!({
            "chat_id": chat_id,
            "action": "typing",
        }))
        .send();
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

/// Split a response into messages along paragraph and code-fence boundaries
/// so each paragraph can be sent as its own Telegram message for a more
/// interactive feel.
///
/// Rules:
/// - Fenced code blocks (``` … ```) stay in a single message, even if they
///   span blank lines.
/// - Outside of code fences, split on blank-line boundaries (`\n\n`).
/// - Paragraphs shorter than `PARAGRAPH_MERGE_THRESHOLD` merge forward into
///   the next one, so short replies don't fragment into multiple pings.
/// - Any resulting paragraph still longer than `max_len` falls back to a hard
///   newline-boundary split via [`split_message`].
fn split_into_paragraphs(text: &str, max_len: usize) -> Vec<String> {
    let chunks = split_at_code_fences(text);

    // Break text chunks on blank lines; keep code chunks whole.
    let mut raw: Vec<(bool, String)> = Vec::new();
    for (is_code, body) in chunks {
        if is_code {
            raw.push((true, body));
            continue;
        }
        for para in body.split("\n\n") {
            let trimmed = para.trim();
            if !trimmed.is_empty() {
                raw.push((false, trimmed.to_string()));
            }
        }
    }

    // Merge short non-code paragraphs forward. Code blocks flush any pending
    // short paragraph first and are never merged themselves.
    let mut merged: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    for (is_code, body) in raw {
        if is_code {
            let combined = match pending.take() {
                Some(prev) => format!("{prev}\n\n{body}"),
                None => body,
            };
            merged.push(combined);
        } else if body.chars().count() < PARAGRAPH_MERGE_THRESHOLD {
            pending = Some(match pending.take() {
                Some(prev) => format!("{prev}\n\n{body}"),
                None => body,
            });
        } else {
            let combined = match pending.take() {
                Some(prev) => format!("{prev}\n\n{body}"),
                None => body,
            };
            merged.push(combined);
        }
    }
    if let Some(p) = pending.take() {
        // Trailing short paragraph — attach to the previous message if any,
        // otherwise emit it on its own.
        if let Some(last) = merged.last_mut() {
            last.push_str("\n\n");
            last.push_str(&p);
        } else {
            merged.push(p);
        }
    }

    // Enforce Telegram's per-message size cap.
    let mut out: Vec<String> = Vec::new();
    for p in merged {
        if p.len() <= max_len {
            out.push(p);
        } else {
            for chunk in split_message(&p, max_len) {
                out.push(chunk.to_string());
            }
        }
    }
    out
}

/// Split a string into alternating (is_code, body) chunks based on ```
/// fences at line starts. An unclosed fence is treated as code through EOF.
fn split_at_code_fences(text: &str) -> Vec<(bool, String)> {
    let mut result: Vec<(bool, String)> = Vec::new();
    let mut in_code = false;
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            if in_code {
                current.push_str(line);
                result.push((true, std::mem::take(&mut current)));
                in_code = false;
            } else {
                if !current.is_empty() {
                    result.push((false, std::mem::take(&mut current)));
                }
                current.push_str(line);
                in_code = true;
            }
        } else {
            current.push_str(line);
        }
    }
    if !current.is_empty() {
        result.push((in_code, current));
    }
    result
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
        let split_at = remaining[..max_len].rfind('\n').unwrap_or(max_len);
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
    fn split_paragraphs_single_short() {
        let out = split_into_paragraphs("just a quick hello", TG_MAX_MESSAGE_LEN);
        assert_eq!(out, vec!["just a quick hello".to_string()]);
    }

    #[test]
    fn split_paragraphs_breaks_on_blank_line() {
        let text = "First paragraph is long enough to clearly exceed the eighty-character merge threshold and therefore stand on its own.\n\nSecond paragraph is also well above the merge threshold so it should end up as a separate output message.";
        let out = split_into_paragraphs(text, TG_MAX_MESSAGE_LEN);
        assert_eq!(out.len(), 2);
        assert!(out[0].starts_with("First"));
        assert!(out[1].starts_with("Second"));
    }

    #[test]
    fn split_paragraphs_keeps_code_block_whole() {
        let text = "Here is the code you asked for:\n\n```rust\nfn main() {\n    println!(\"hi\");\n\n    println!(\"bye\");\n}\n```\n\nThat should do it.";
        let out = split_into_paragraphs(text, TG_MAX_MESSAGE_LEN);
        // Short trailing "That should do it." merges back into the code
        // block's message; intro "Here is the code..." is short so it merges
        // forward into the code block. Expect one combined message.
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("```rust"));
        assert!(out[0].contains("println!(\"hi\");"));
        assert!(out[0].contains("println!(\"bye\");"));
    }

    #[test]
    fn split_paragraphs_merges_short_forward() {
        // Short intro paragraph followed by a long one — expect merged into
        // one message, not two.
        let text = "Sure!\n\nHere is a detailed explanation that is definitely long enough to exceed the merge threshold and therefore stay on its own unless merged with the preceding short line.";
        let out = split_into_paragraphs(text, TG_MAX_MESSAGE_LEN);
        assert_eq!(out.len(), 1);
        assert!(out[0].starts_with("Sure!"));
        assert!(out[0].contains("detailed explanation"));
    }

    #[test]
    fn split_paragraphs_hard_splits_oversize() {
        let big = "a".repeat(100);
        let out = split_into_paragraphs(&big, 30);
        assert!(out.len() >= 4);
        for chunk in &out {
            assert!(chunk.len() <= 30);
        }
    }

    #[test]
    fn split_paragraphs_unterminated_code_fence_stays_together() {
        let text = "intro line that is reasonably long so it isn't merged away by itself ok\n\n```\nno close fence here\nline two\n```";
        let out = split_into_paragraphs(text, TG_MAX_MESSAGE_LEN);
        // Either one combined message (if merged) or two separate — but the
        // code must not fragment internally.
        let joined = out.join("\n---\n");
        assert!(joined.contains("no close fence here"));
        assert!(joined.contains("line two"));
    }

    #[test]
    fn paragraph_pacing_respects_min_and_max() {
        // Empty text bottoms out at the minimum.
        assert_eq!(paragraph_pacing(""), PARAGRAPH_PACING_MIN);
        // Short text is close to the minimum but not below.
        let short = paragraph_pacing("hi");
        assert!(short >= PARAGRAPH_PACING_MIN);
        assert!(short < PARAGRAPH_PACING_MIN + Duration::from_millis(100));
        // Very long text clamps to the maximum.
        let huge = "x".repeat(10_000);
        assert_eq!(paragraph_pacing(&huge), PARAGRAPH_PACING_MAX);
        // Medium text scales between min and max.
        let mid = "x".repeat(100);
        let pacing = paragraph_pacing(&mid);
        assert!(pacing > PARAGRAPH_PACING_MIN);
        assert!(pacing < PARAGRAPH_PACING_MAX);
    }

    #[test]
    fn find_safe_cut_incomplete_paragraph() {
        // Mid-sentence — nothing complete yet.
        assert_eq!(find_safe_cut("hello world"), None);
        assert_eq!(find_safe_cut("line one\nline two"), None);
    }

    #[test]
    fn find_safe_cut_one_completed_paragraph() {
        // One full paragraph followed by start of the next.
        let text = "Hello world.\n\nAnd then";
        let cut = find_safe_cut(text).expect("should find cut");
        assert_eq!(&text[..cut], "Hello world.\n\n");
    }

    #[test]
    fn find_safe_cut_two_completed_paragraphs() {
        // Cut should advance to AFTER the last safe boundary.
        let text = "First one.\n\nSecond one.\n\nStill writing";
        let cut = find_safe_cut(text).expect("should find cut");
        assert_eq!(&text[..cut], "First one.\n\nSecond one.\n\n");
    }

    #[test]
    fn find_safe_cut_inside_open_code_fence_waits() {
        // Fence opened, not closed — don't cut even though there's a blank
        // line inside the code block.
        let text = "Here's the code:\n\n```rust\nfn a() {}\n\nfn b() {}";
        let cut = find_safe_cut(text).expect("should find cut");
        // The only safe cut is after the intro paragraph's blank line.
        assert_eq!(&text[..cut], "Here's the code:\n\n");
    }

    #[test]
    fn find_safe_cut_closed_code_fence() {
        let text = "```rust\nfn main() {}\n```\nmore";
        let cut = find_safe_cut(text).expect("should find cut");
        assert_eq!(&text[..cut], "```rust\nfn main() {}\n```\n");
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
