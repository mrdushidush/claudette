//! `ApiClient` implementation that talks to a local Ollama instance.
//!
//! Critical knobs:
//! - Calls `/api/chat` directly with `think: false` so reasoning models like
//!   qwen3.5:9b skip their chain-of-thought.
//! - Uses native tool calling: passes a `tools` array on every request and
//!   parses `message.tool_calls` from the response.
//! - Holds the tool list and context window on the client itself, so the
//!   `crate::ApiClient` trait does not need to change.

use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{
    ApiClient, ApiRequest, AssistantEvent, ContentBlock, MessageRole, RuntimeError, TokenUsage,
};
use serde_json::{json, Value};

use crate::tool_groups::ToolRegistry;

/// Callback type fired once per text delta when streaming is enabled. The
/// callback owns no state shared with the runtime — it just receives bytes
/// and is expected to side-effect (print, accumulate to a buffer, etc.).
/// `Send + Sync` so the client can be used across threads if a future
/// runtime ever wants to.
pub type TextCallback = Box<dyn Fn(&str) + Send + Sync>;

/// Convenience constructor for the standard "print to stdout immediately"
/// callback used by the REPL. Lives here (and not in `run.rs`) so other
/// callers — tests, future TUIs — can pick it up without re-implementing
/// the flush dance.
#[must_use]
pub fn stdout_text_callback() -> TextCallback {
    Box::new(|delta: &str| {
        use std::io::Write;
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let _ = out.write_all(delta.as_bytes());
        let _ = out.flush();
    })
}

/// Convenience constructor for forwarding text deltas to the TUI via a
/// sync channel. Each delta fires one `TuiEvent::Token`. Used by the TUI
/// worker thread instead of the REPL's stdout callback.
#[must_use]
pub fn tui_text_callback(
    tx: std::sync::mpsc::SyncSender<crate::tui_events::TuiEvent>,
) -> TextCallback {
    Box::new(move |delta: &str| {
        let _ = tx.send(crate::tui_events::TuiEvent::Token(delta.to_string()));
    })
}

/// Shared streaming buffer used by Telegram mode. The brain's text-delta
/// callback appends to this buffer as tokens arrive; a poller thread in
/// `telegram_mode` scans the buffer for completed paragraphs and sends
/// them to the chat, so responses arrive progressively during generation
/// instead of after the turn finishes.
static TELEGRAM_STREAM_BUFFER: std::sync::OnceLock<Mutex<String>> = std::sync::OnceLock::new();

/// Accessor for the Telegram stream buffer. Lazily initialised on first call.
#[must_use]
pub fn telegram_stream_buffer() -> &'static Mutex<String> {
    TELEGRAM_STREAM_BUFFER.get_or_init(|| Mutex::new(String::new()))
}

/// Reset the Telegram stream buffer. Called at the start of each turn so
/// leftover bytes from the previous turn don't leak.
pub fn telegram_stream_reset() {
    if let Ok(mut buf) = telegram_stream_buffer().lock() {
        buf.clear();
    }
}

/// Callback for Telegram mode: appends deltas to the shared stream buffer
/// and also mirrors them to stdout so the server terminal still shows the
/// model's output as it streams.
#[must_use]
pub fn telegram_text_callback() -> TextCallback {
    Box::new(|delta: &str| {
        use std::io::Write;
        if let Ok(mut buf) = telegram_stream_buffer().lock() {
            buf.push_str(delta);
        }
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let _ = out.write_all(delta.as_bytes());
        let _ = out.flush();
    })
}

const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";
/// Default Ollama context window.
///
/// History:
/// - 2048 (initial)
/// - 4096 (2026-04-08, paired with `qwen3.5:9b` — anything bigger blew the
///   8 GB VRAM budget because the 9b model itself ate ~6.6 GB)
/// - 32768 (2026-04-09 morning, paired with `qwen3.5:4b`) — 8x the prior
///   ceiling. The 4b model freed enough VRAM for the KV cache to hold
///   ~32 K tokens at `q8_0`. But the 4b model proved too unreliable on
///   real tool-using turns (hallucinated `write_file` success).
/// - **16384 (2026-04-09 evening, paired with `qwen3:8b`)** — middle
///   ground. The 8b model is ~5 GB at Q4 (between the 4b and 9b sizes),
///   leaving enough VRAM for a 16 K KV cache at `q8_0` on a 3060 Ti.
///   4x the original `qwen3.5:9b` ceiling without the hallucination
///   penalty of the 4b.
///
/// Override per-process with `CLAUDETTE_NUM_CTX`. The truncator +
/// session-size auto-compaction still keep requests under the chosen
/// ceiling.
pub const DEFAULT_NUM_CTX: u32 = 16384;
/// Maximum tokens the model can generate per request. 6144 gives ~50%
/// headroom over the original 4096 ceiling — room for researcher summaries
/// and long multi-turn answers without eating too much of the input budget.
/// Override with `CLAUDETTE_NUM_PREDICT`.
pub const DEFAULT_NUM_PREDICT: u32 = 6144;

/// Resolve the actual `num_ctx` to use. Sprint 14: reads from
/// `model_config::active().brain.num_ctx`, which itself merges
/// `CLAUDETTE_NUM_CTX` on first init. Keeps `/status` and
/// `get_capabilities` in sync with slash-command overrides.
#[must_use]
pub fn current_num_ctx() -> u32 {
    crate::model_config::active().brain.num_ctx
}

/// Resolve the actual `num_predict` to use. Same story as
/// [`current_num_ctx`] — delegates to the active model config so
/// slash-command overrides are reflected immediately.
#[must_use]
pub fn current_num_predict() -> u32 {
    crate::model_config::active().brain.num_predict
}
const REQUEST_TIMEOUT_SECS: u64 = 300;
/// Rough chars-per-token estimate used by `truncate_to_budget`. Ollama doesn't
/// expose its tokenizer to the client; ~4 chars/token is the standard
/// English-text rule of thumb and is conservative enough for sandboxing.
const CHARS_PER_TOKEN: usize = 4;
/// Reserve this many chars (~256 tokens) of headroom inside `num_ctx` for
/// rounding error in the chars/token heuristic plus chat-template overhead.
const SAFETY_CHARS: usize = 1024;

/// How the client sources the `tools` array for each request. Agents use a
/// [`ToolsProvider::Fixed`] value (a pre-filtered allowlist); the main Claudette
/// runtime uses [`ToolsProvider::Dynamic`] so the model can enable tool groups
/// mid-conversation.
///
/// Kept as an enum (not a trait object) so the common case stays
/// allocation-free and the type stays `Send + Sync` without ceremony.
pub enum ToolsProvider {
    /// Static JSON array — shipped unchanged on every request.
    /// Used by spawned agents whose tool allowlist doesn't change mid-session.
    Fixed(Value),
    /// Shared, mutable registry — queried on every request. Used by the main
    /// Claudette runtime so `enable_tools` calls take effect on the next turn.
    Dynamic(Arc<Mutex<ToolRegistry>>),
}

impl ToolsProvider {
    /// Resolve the current tools array. `Dynamic` calls `current_tools` on
    /// the shared registry; on a poisoned lock the thread that originally
    /// poisoned it already surfaced the error, so we fall back to the
    /// `into_inner` payload (`PoisonError::into_inner`) to stay operational.
    #[must_use]
    pub fn current(&self) -> Value {
        match self {
            Self::Fixed(v) => v.clone(),
            Self::Dynamic(reg) => match reg.lock() {
                Ok(g) => g.current_tools(),
                Err(poisoned) => poisoned.into_inner().current_tools(),
            },
        }
    }
}

/// Ollama-backed `ApiClient` for the secretary mode.
pub struct OllamaApiClient {
    http: reqwest::blocking::Client,
    base_url: String,
    model: String,
    tools: ToolsProvider,
    num_ctx: u32,
    num_predict: u32,
    /// When set, every streamed text delta is forwarded to this callback as
    /// it arrives. The runtime still gets the fully accumulated text in the
    /// returned `AssistantEvent::TextDelta` — the callback is purely for UX
    /// (e.g. the REPL prints tokens to stdout as they appear).
    text_callback: Option<TextCallback>,
    /// When true, swap the request shape + endpoint to OpenAI Chat
    /// Completions (`/v1/chat/completions`) and parse a single non-streaming
    /// JSON response. Driven by `CLAUDETTE_OPENAI_COMPAT=1`. Lets a single
    /// client point at LM Studio (or any OpenAI-format server) without a
    /// second `ApiClient` impl. Trade-off: no token-by-token streaming UX in
    /// compat mode (the text callback fires once with the full content).
    openai_compat: bool,
}

impl OllamaApiClient {
    /// Create a new client with a fixed tool list — the JSON array advertised
    /// to the model on every request never changes. Used by spawned agents
    /// (who have a hard tool allowlist) and by tests.
    ///
    /// For the main Claudette runtime, use [`Self::with_registry`] instead so
    /// the model can enable tool groups mid-conversation.
    ///
    /// Honors `OLLAMA_HOST` env var the same way Ollama itself does.
    #[must_use]
    pub fn new(model: impl Into<String>, tools: Value) -> Self {
        Self::build(model.into(), ToolsProvider::Fixed(tools))
    }

    /// Create a new client backed by a shared [`ToolRegistry`]. The registry
    /// is read on every request, so if another thread calls `enable_group`
    /// between turns, the next `/api/chat` call will advertise the expanded
    /// tool set.
    #[must_use]
    pub fn with_registry(model: impl Into<String>, registry: Arc<Mutex<ToolRegistry>>) -> Self {
        Self::build(model.into(), ToolsProvider::Dynamic(registry))
    }

    fn build(model: String, tools: ToolsProvider) -> Self {
        let base_url = resolve_ollama_url();
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("failed to build reqwest blocking client");

        Self {
            http,
            base_url,
            model,
            tools,
            num_ctx: current_num_ctx(),
            num_predict: current_num_predict(),
            text_callback: None,
            openai_compat: resolve_openai_compat(),
        }
    }

    /// Force OpenAI-compat mode on or off, overriding the
    /// `CLAUDETTE_OPENAI_COMPAT` env var. Mostly for tests; production code
    /// should set the env var so every code path that constructs a client
    /// (REPL, TUI, agents, fallback) inherits it consistently.
    #[must_use]
    pub fn with_openai_compat(mut self, on: bool) -> Self {
        self.openai_compat = on;
        self
    }

    #[must_use]
    pub fn with_context(mut self, num_ctx: u32) -> Self {
        self.num_ctx = num_ctx;
        self
    }

    #[must_use]
    pub fn with_max_predict(mut self, num_predict: u32) -> Self {
        self.num_predict = num_predict;
        self
    }

    /// Install a text-delta callback. The callback fires once per streamed
    /// chunk with the new content (which may be a single token or several).
    /// REPL mode uses this to print tokens to stdout as they arrive; the
    /// runtime still receives the full accumulated text in the returned
    /// event vec.
    #[must_use]
    pub fn with_text_callback(mut self, callback: TextCallback) -> Self {
        self.text_callback = Some(callback);
        self
    }

    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }
}

/// Resolve the Ollama base URL (no trailing slash). Honors `OLLAMA_HOST`;
/// falls back to `http://localhost:11434`.
#[must_use]
pub fn resolve_ollama_url() -> String {
    match std::env::var("OLLAMA_HOST") {
        Ok(host) if !host.is_empty() => {
            let host = host.trim_end_matches('/');
            if host.starts_with("http://") || host.starts_with("https://") {
                host.to_string()
            } else {
                format!("http://{host}")
            }
        }
        _ => DEFAULT_OLLAMA_URL.to_string(),
    }
}

/// Returns true when LM Studio (or any OpenAI Chat Completions-compatible)
/// mode is requested. Set `CLAUDETTE_OPENAI_COMPAT=1` to opt in. The brain
/// client will then POST to `/v1/chat/completions` instead of `/api/chat`,
/// parse a non-streaming JSON response (no SSE yet), and skip Ollama-specific
/// request fields (`think`, `options.num_*`, `keep_alive`).
///
/// Pair with `OLLAMA_HOST=http://localhost:1234` for a local LM Studio
/// server, and a model id that LM Studio recognises (e.g.
/// `CLAUDETTE_MODEL=openai/gpt-oss-20b`). Disable the
/// 4b→9b fallback dance with `CLAUDETTE_FALLBACK_BRAIN_MODEL=` since
/// LM Studio doesn't speak Ollama's keep-alive eviction protocol.
#[must_use]
pub fn resolve_openai_compat() -> bool {
    std::env::var("CLAUDETTE_OPENAI_COMPAT")
        .ok()
        .is_some_and(|v| !v.is_empty() && v != "0")
}

/// Returns true when the given URL's host is a loopback / localhost
/// address. Used to warn users when `OLLAMA_HOST` points at a remote
/// endpoint — the README tagline is "runs entirely on your hardware,"
/// so a `OLLAMA_HOST=https://someone-elses-server:11434` (accidentally
/// inherited from `~/.claudette/.env` or a shell snippet copied from
/// a tutorial) is worth surfacing loudly. As of the dotenv-CWD fix,
/// arbitrary project `.env` files no longer feed into this path.
#[must_use]
pub fn is_local_ollama_url(url: &str) -> bool {
    // Strip scheme if present, case-insensitively. We only need the host.
    let rest = if url.len() >= 8 && url[..8].eq_ignore_ascii_case("https://") {
        &url[8..]
    } else if url.len() >= 7 && url[..7].eq_ignore_ascii_case("http://") {
        &url[7..]
    } else {
        url
    };
    // Drop any path suffix (not expected for the probe URL but be safe).
    let rest = rest.split('/').next().unwrap_or(rest);
    // Drop userinfo (user[:pass]@). Without this, `localhost:fake@evil.com`
    // would parse host as `localhost` instead of `evil.com`. RFC 3986
    // requires the last `@` to be the userinfo/host boundary.
    let host_and_port = match rest.rfind('@') {
        Some(idx) => &rest[idx + 1..],
        None => rest,
    };
    // Drop the port. IPv6 bracket form `[::1]:11434` — take inside brackets.
    let host = if let Some(inside) = host_and_port
        .strip_prefix('[')
        .and_then(|s| s.split(']').next())
    {
        inside
    } else {
        host_and_port.split(':').next().unwrap_or(host_and_port)
    };
    let host_lower = host.to_ascii_lowercase();

    // `0.0.0.0` and `::` are valid BIND addresses (Ollama listens on all
    // interfaces) but not valid DESTINATION addresses — a TCP connect to
    // 0.0.0.0 usually routes to the default local interface on Unix and
    // errors on Windows. Treating them as loopback suppresses the warning
    // even though OLLAMA_HOST=http://0.0.0.0:11434 does not mean "stay
    // local". Drop them from the loopback list; a user who really wants
    // that config can set CLAUDETTE_ALLOW_REMOTE_OLLAMA=1.
    if host_lower == "localhost" || host_lower == "::1" {
        return true;
    }
    // 127.0.0.0/8 — any loopback IPv4.
    if let Some(rest) = host_lower.strip_prefix("127.") {
        return rest.split('.').count() == 3
            && rest
                .split('.')
                .all(|s| !s.is_empty() && s.parse::<u8>().is_ok());
    }
    false
}

/// Short-timeout GET on the resolved Ollama base URL to verify the daemon
/// is reachable before we drop into any interactive mode. Ollama answers
/// its root path with "Ollama is running" and a 200; we only care that the
/// TCP connect + HTTP round-trip succeed.
///
/// Returns the resolved URL on success so callers can echo it for context.
/// Returns a user-facing message on failure — main.rs prints this verbatim.
///
/// Set `CLAUDETTE_SKIP_OLLAMA_PROBE=1` to bypass (CI / offline sessions
/// that will only hit saved state).
///
/// Prints a loud stderr warning when the resolved URL is not a loopback
/// address, unless `CLAUDETTE_ALLOW_REMOTE_OLLAMA=1` is set. Claudette's
/// marketing story is local-first; a surprise remote host is a footgun
/// worth surfacing at startup.
pub fn probe_ollama() -> Result<String, String> {
    let url = resolve_ollama_url();

    // Warn on non-loopback hosts. Runs regardless of the skip-probe flag
    // because "I skipped the probe" doesn't imply "I consented to a
    // remote brain" — both apply independently.
    if !is_local_ollama_url(&url)
        && std::env::var("CLAUDETTE_ALLOW_REMOTE_OLLAMA")
            .ok()
            .map_or(true, |v| v.is_empty() || v == "0")
    {
        eprintln!(
            "⚠  OLLAMA_HOST points at a non-loopback address: {url}\n\
             Every prompt, tool call, and piece of memory/email/calendar\n\
             data will be sent to that host. Claudette's default posture\n\
             is local-only; a remote endpoint turns it into a cloud client.\n\
             If this is intentional, set CLAUDETTE_ALLOW_REMOTE_OLLAMA=1\n\
             to silence this warning."
        );
    }

    if std::env::var("CLAUDETTE_SKIP_OLLAMA_PROBE")
        .ok()
        .is_some_and(|v| !v.is_empty() && v != "0")
    {
        return Ok(url);
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| format!("could not build probe client: {e}"))?;
    // In OpenAI-compat mode hit `/v1/models` instead of the bare root, since
    // LM Studio doesn't answer GET / with a 200 the way Ollama does.
    let (probe_url, mode_label) = if resolve_openai_compat() {
        (format!("{url}/v1/models"), "OpenAI-compat brain")
    } else {
        (url.clone(), "Ollama")
    };
    match client.get(&probe_url).send() {
        Ok(resp) if resp.status().is_success() || resp.status().is_redirection() => Ok(url),
        Ok(resp) => Err(format!(
            "{mode_label} at {probe_url} returned HTTP {} — is a different service bound to that port?",
            resp.status()
        )),
        Err(e) => Err(format!(
            "{mode_label} not reachable at {probe_url} ({e}). Start the server \
             (or set OLLAMA_HOST), then retry. Set CLAUDETTE_SKIP_OLLAMA_PROBE=1 to bypass."
        )),
    }
}

impl ApiClient for OllamaApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let body = self.build_chat_body(&request);
        let path = if self.openai_compat {
            "/v1/chat/completions"
        } else {
            "/api/chat"
        };
        let url = format!("{}{}", self.base_url, path);

        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| RuntimeError::new(format!("Brain request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            return Err(RuntimeError::new(format!(
                "Brain HTTP {status}: {}",
                text.chars().take(400).collect::<String>()
            )));
        }

        if self.openai_compat {
            // Non-streaming for now — single JSON response, no SSE parsing.
            // Trade-off: the text callback fires once with the full content,
            // not token-by-token. Adding SSE support is a follow-up.
            let body: Value = resp.json().map_err(|e| {
                RuntimeError::new(format!("OpenAI-compat response parse failed: {e}"))
            })?;
            self.parse_openai_response(&body)
        } else {
            // Reqwest's blocking Response implements Read, so we can wrap it
            // in a BufReader and consume the NDJSON stream line by line. The
            // text callback (if installed) is fired for every non-empty
            // content delta.
            self.consume_stream_lines(BufReader::new(resp))
        }
    }
}

impl OllamaApiClient {
    fn build_chat_body(&self, request: &ApiRequest) -> Value {
        // Resolve `tools` ONCE per request and pass the same value to both
        // `history_budget_chars` (which subtracts its char cost) and the
        // request body, so we never race with a concurrent `enable_tools`
        // call that would make the two views disagree.
        let tools = self.tools.current();
        let history_budget = self.history_budget_chars_for_tools(request, &tools);
        let messages = build_messages(request, history_budget);
        if self.openai_compat {
            // OpenAI Chat Completions shape. `temperature` and `max_tokens`
            // are top-level (not nested in `options`). `num_ctx` has no
            // analogue — context is set at model-load time in LM Studio
            // (e.g. `lms load --context-length 32768`). The Ollama-only
            // `think: false` flag is dropped — gpt-oss and other reasoning
            // models on LM Studio benefit from their reasoning trace.
            json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "stream": false,
                "temperature": 0.0,
                "max_tokens": self.num_predict,
            })
        } else {
            json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                // `stream: true` switches Ollama into NDJSON mode — one JSON
                // object per line. Each chunk carries a `message.content`
                // delta (often a single token) and the final chunk has
                // `done: true` plus the prompt/eval token counts. See
                // `consume_stream_lines` for the parser.
                "stream": true,
                "think": false,
                "options": {
                    "temperature": 0.0,
                    "num_ctx": self.num_ctx,
                    "num_predict": self.num_predict
                }
            })
        }
    }

    /// Parse a non-streaming OpenAI Chat Completions response into the same
    /// `Vec<AssistantEvent>` shape the runtime expects. The single-message
    /// JSON body is much simpler than Ollama's NDJSON stream — we just
    /// extract `choices[0].message.{content,tool_calls}` and the top-level
    /// `usage` block.
    ///
    /// **Tool call argument shape diff:** Ollama emits
    /// `function.arguments` as a JSON object; OpenAI emits it as a JSON
    /// **string** containing the arguments JSON. We pass the raw string
    /// straight through to `AssistantEvent::ToolUse.input` (which is itself
    /// a `String` of JSON), matching the Ollama path's behaviour after its
    /// own `serde_json::to_string` round-trip.
    fn parse_openai_response(&self, body: &Value) -> Result<Vec<AssistantEvent>, RuntimeError> {
        if let Some(err) = body.pointer("/error/message").and_then(Value::as_str) {
            return Err(RuntimeError::new(format!("OpenAI-compat error: {err}")));
        }

        let message = body
            .pointer("/choices/0/message")
            .ok_or_else(|| RuntimeError::new("OpenAI response missing choices[0].message"))?;

        let mut events = Vec::new();

        let content = message.get("content").and_then(Value::as_str).unwrap_or("");
        if !content.is_empty() {
            // Compat mode is non-streaming, but the REPL/TUI text callback
            // still expects to see the assistant's prose at some point. Fire
            // it once with the full content, then once with a trailing
            // newline so the next REPL line lands cleanly — same contract as
            // the Ollama streaming path's terminal newline.
            if let Some(cb) = &self.text_callback {
                cb(content);
                cb("\n");
            }
            events.push(AssistantEvent::TextDelta(content.to_string()));
        }

        if let Some(arr) = message.get("tool_calls").and_then(Value::as_array) {
            for (idx, tc) in arr.iter().enumerate() {
                let name = tc
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                // OpenAI tool-call arguments are a JSON-encoded string, not
                // a nested object. Keep it as-is — the runtime hands this
                // straight to the tool dispatcher which parses it with
                // `serde_json::from_str`.
                let arguments_str = tc
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .map_or_else(|| "{}".to_string(), str::to_string);
                let id = tc
                    .get("id")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("call_{idx}"), String::from);
                events.push(AssistantEvent::ToolUse {
                    id,
                    name,
                    input: arguments_str,
                });
            }
        }

        let usage = body.get("usage");
        let input_tokens = usage
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let output_tokens = usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;

        events.push(AssistantEvent::Usage(TokenUsage {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        }));
        events.push(AssistantEvent::MessageStop);

        Ok(events)
    }

    /// Consume an NDJSON stream from Ollama and assemble the same
    /// `Vec<AssistantEvent>` the runtime expects. Each line is a self-
    /// contained JSON object; we accumulate text deltas, capture any
    /// `tool_calls` (Ollama emits these on the final chunk, not incrementally),
    /// and read the token counts off the `done: true` chunk.
    ///
    /// Generic over `BufRead` so the unit tests can pass a `Cursor` directly
    /// instead of needing a real HTTP response.
    fn consume_stream_lines<R: BufRead>(
        &self,
        reader: R,
    ) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let mut accumulated_text = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut input_tokens: u32 = 0;
        let mut output_tokens: u32 = 0;

        for line in reader.lines() {
            let line =
                line.map_err(|e| RuntimeError::new(format!("Ollama stream read failed: {e}")))?;
            if line.trim().is_empty() {
                continue;
            }
            let chunk: Value = serde_json::from_str(&line)
                .map_err(|e| RuntimeError::new(format!("Ollama stream parse failed: {e}")))?;

            if let Some(err) = chunk.get("error").and_then(Value::as_str) {
                return Err(RuntimeError::new(format!("Ollama error: {err}")));
            }

            // Forward any text delta to the callback (and accumulate).
            if let Some(content) = chunk.pointer("/message/content").and_then(Value::as_str) {
                if !content.is_empty() {
                    accumulated_text.push_str(content);
                    if let Some(cb) = &self.text_callback {
                        cb(content);
                    }
                }
            }

            // Tool calls usually arrive only on the final (done) chunk, but
            // we accept them on any chunk to be defensive against future
            // Ollama behaviour changes. Last writer wins.
            if let Some(arr) = chunk
                .pointer("/message/tool_calls")
                .and_then(Value::as_array)
            {
                tool_calls.clone_from(arr);
            }

            // The terminal chunk carries the token usage.
            if chunk.get("done").and_then(Value::as_bool) == Some(true) {
                input_tokens = chunk
                    .get("prompt_eval_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                output_tokens = chunk.get("eval_count").and_then(Value::as_u64).unwrap_or(0) as u32;
            }
        }

        // Terminate the visible stream with a newline so the next REPL line
        // (status, prompt, tool result, etc.) lands cleanly. Only fired when
        // the callback is installed AND the model actually produced text.
        // Note: we deliberately do NOT push this newline into
        // `accumulated_text` — the runtime should see clean assistant text
        // without trailing whitespace.
        if !accumulated_text.is_empty() {
            if let Some(cb) = &self.text_callback {
                cb("\n");
            }
        }

        let mut events = Vec::new();
        if !accumulated_text.is_empty() {
            events.push(AssistantEvent::TextDelta(accumulated_text));
        }
        for (idx, tc) in tool_calls.iter().enumerate() {
            let name = tc
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let arguments = tc
                .pointer("/function/arguments")
                .cloned()
                .unwrap_or(json!({}));
            let input = serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string());
            let id = tc
                .get("id")
                .and_then(Value::as_str)
                .map_or_else(|| format!("call_{idx}"), String::from);
            events.push(AssistantEvent::ToolUse { id, name, input });
        }
        events.push(AssistantEvent::Usage(TokenUsage {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        }));
        events.push(AssistantEvent::MessageStop);

        Ok(events)
    }

    /// Compute how many chars of conversation history we can send before
    /// exceeding `num_ctx`, after subtracting the output reservation, the
    /// system prompt, the tools schema, and a safety margin.
    ///
    /// **Why subtract tools:** Ollama sends the `tools` field as part of the
    /// chat-template prompt and it counts against the context window the
    /// same as messages. Missing this subtraction caused the truncator to think it
    /// had ~2x the budget it actually did, so big tool results blew past
    /// the real ceiling and the next turn lost all context. Measured the
    /// 11-tool secretary registry at 4731 chars (~1182 tokens) — about 29%
    /// of a `num_ctx: 4096` window.
    ///
    /// Test-only wrapper that resolves `tools` through
    /// [`ToolsProvider::current`] before delegating. Production code uses
    /// [`Self::history_budget_chars_for_tools`] directly so the same `tools`
    /// value is reused across budget-subtraction and request serialization.
    #[cfg(test)]
    fn history_budget_chars(&self, request: &ApiRequest) -> usize {
        self.history_budget_chars_for_tools(request, &self.tools.current())
    }

    fn history_budget_chars_for_tools(&self, request: &ApiRequest, tools: &Value) -> usize {
        let total = self.num_ctx as usize * CHARS_PER_TOKEN;
        let output = self.num_predict as usize * CHARS_PER_TOKEN;
        let system: usize = request
            .system_prompt
            .iter()
            .map(|s| s.len() + 2) // +2 for the "\n\n" join
            .sum();
        let tools_chars = tools.to_string().len();
        total
            .saturating_sub(output)
            .saturating_sub(system)
            .saturating_sub(tools_chars)
            .saturating_sub(SAFETY_CHARS)
    }
}

/// Build the full Ollama `messages` array: system prompt (always kept) plus
/// the conversation history truncated to fit `history_budget_chars`.
fn build_messages(request: &ApiRequest, history_budget_chars: usize) -> Vec<Value> {
    let history = build_history_messages(&request.messages);
    let history = truncate_to_budget(history, history_budget_chars);

    let mut messages = Vec::with_capacity(history.len() + 1);
    let system_prompt = request.system_prompt.join("\n\n");
    if !system_prompt.is_empty() {
        messages.push(json!({
            "role": "system",
            "content": system_prompt,
        }));
    }
    messages.extend(history);
    messages
}

/// Convert conversation messages into the Ollama JSON shape, without the
/// system prompt and without truncation.
fn build_history_messages(msgs: &[crate::ConversationMessage]) -> Vec<Value> {
    let mut messages = Vec::with_capacity(msgs.len());
    for msg in msgs {
        let role = role_str(msg.role);
        let mut content_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<Value> = Vec::new();

        for block in &msg.blocks {
            match block {
                ContentBlock::Text { text } => {
                    content_parts.push(text.clone());
                }
                ContentBlock::ToolUse { id, name, input } => {
                    let arguments: Value =
                        serde_json::from_str(input).unwrap_or_else(|_| json!({}));
                    tool_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": arguments,
                        }
                    }));
                }
                ContentBlock::ToolResult { output, .. } => {
                    // For MVP we coalesce tool results into the message content.
                    // Multi-step tool conversations may need a dedicated `tool`
                    // role message keyed by tool_use_id; revisit when we add the
                    // second tool.
                    content_parts.push(output.clone());
                }
            }
        }

        let content = content_parts.join("\n");
        let mut obj = json!({
            "role": role,
            "content": content,
        });
        if !tool_calls.is_empty() {
            obj["tool_calls"] = json!(tool_calls);
        }
        messages.push(obj);
    }
    messages
}

/// Greedy sliding-window truncation. Walks `messages` from newest to oldest,
/// keeping each one that still fits in `budget_chars`, then returns the kept
/// messages in original chronological order.
///
/// **Always keeps the most recent message**, even if it overshoots the
/// budget — better to send one oversized request and let Ollama auto-extend
/// for one turn than to drop the very thing the user just typed. The runtime
/// layer's `auto_compaction` handles the real long-term cleanup; this
/// truncator is the in-iteration safety net.
///
/// **Skips oversized older messages instead of aborting** the walk: if
/// message N is too big to add but message N-1 (older) would still fit,
/// we keep N-1. Previously a `break` here meant a single huge tool result
/// (e.g. `list_dir` on a deep home directory) wiped out *every* prior turn.
///
/// **Why a free function and not inside `build_messages`:** keeps it pure and
/// directly testable (no `ApiRequest` ceremony to construct in tests).
///
/// **Why char count instead of real tokens:** Ollama doesn't expose its
/// tokenizer to the client. `chars / 4` is the standard English rule of thumb
/// and we pad with `SAFETY_CHARS` to absorb the inaccuracy.
fn truncate_to_budget(messages: Vec<Value>, budget_chars: usize) -> Vec<Value> {
    let mut kept: Vec<Value> = Vec::with_capacity(messages.len());
    let mut used = 0usize;
    let total = messages.len();
    for (idx_from_end, msg) in messages.into_iter().rev().enumerate() {
        let cost = estimate_message_chars(&msg);
        // Always keep the most recent message regardless of budget — the user
        // (or the agent loop) just produced it and dropping it breaks the
        // current turn. budget_chars==0 still admits this one message.
        let is_newest = idx_from_end == 0 && total > 0;
        if !is_newest && used.saturating_add(cost) > budget_chars {
            // Skip this older message but keep walking — a smaller older
            // message might still fit. NB: this can produce a "history with
            // a hole", but the model handles missing chronological pieces
            // better than missing the immediate context.
            continue;
        }
        used = used.saturating_add(cost);
        kept.push(msg);
    }
    kept.reverse();
    kept
}

/// Estimate the character cost of an Ollama message: text content plus
/// the JSON-encoded length of any `tool_calls` block.
fn estimate_message_chars(msg: &Value) -> usize {
    let content = msg
        .get("content")
        .and_then(Value::as_str)
        .map_or(0, str::len);
    let tools = msg.get("tool_calls").map_or(0, |v| v.to_string().len());
    content + tools
}

fn role_str(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::Tool => "tool",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConversationMessage, MessageRole};

    #[test]
    fn is_local_ollama_url_recognises_loopback() {
        for url in [
            "http://localhost:11434",
            "https://localhost:11434",
            "http://LOCALHOST:11434",
            "HTTP://localhost:11434",
            "HTTPS://localhost:11434",
            "http://user:pass@localhost:11434",
            "http://127.0.0.1:11434",
            "http://127.0.0.2:11434",
            "http://127.255.255.255:11434",
            "http://[::1]:11434",
            "localhost:11434",
            "127.0.0.1",
        ] {
            assert!(
                is_local_ollama_url(url),
                "expected local, but {url} was flagged remote"
            );
        }
    }

    #[test]
    fn is_local_ollama_url_rejects_remote() {
        for url in [
            "http://ollama.example.com:11434",
            "https://attacker.evil:11434",
            "http://192.168.1.10:11434", // LAN but not loopback — still warrants warning
            "http://10.0.0.1:11434",     // private but not loopback
            "http://1.2.3.4:11434",
            "http://[2001:db8::1]:11434",
            // 0.0.0.0 and :: are bind addresses, not valid destinations;
            // treating them as "local" masks a real misconfiguration.
            "http://0.0.0.0:11434",
            "http://[::]:11434",
            // Userinfo smuggling — without stripping last `@` the host
            // would parse as `localhost` and bypass the remote warning.
            "http://localhost:fakepass@evil.com:11434",
            "http://localhost@evil.com:11434",
        ] {
            assert!(
                !is_local_ollama_url(url),
                "expected remote, but {url} was flagged local"
            );
        }
    }

    #[test]
    fn probe_ollama_skip_env_short_circuits() {
        // An unroutable host would normally fail the probe quickly; with
        // the skip env var set we must return Ok without touching the
        // network. Using a .invalid TLD guarantees no DNS, so a live probe
        // would definitely fail — if we see Ok here, we know the skip path
        // ran.
        let prev_host = std::env::var("OLLAMA_HOST").ok();
        let prev_skip = std::env::var("CLAUDETTE_SKIP_OLLAMA_PROBE").ok();
        std::env::set_var(
            "OLLAMA_HOST",
            "http://definitely-not-a-real-host.invalid:11434",
        );
        std::env::set_var("CLAUDETTE_SKIP_OLLAMA_PROBE", "1");

        let result = probe_ollama();
        assert!(
            result.is_ok(),
            "skip env should bypass the probe; got {result:?}"
        );

        match prev_host {
            Some(v) => std::env::set_var("OLLAMA_HOST", v),
            None => std::env::remove_var("OLLAMA_HOST"),
        }
        match prev_skip {
            Some(v) => std::env::set_var("CLAUDETTE_SKIP_OLLAMA_PROBE", v),
            None => std::env::remove_var("CLAUDETTE_SKIP_OLLAMA_PROBE"),
        }
    }

    fn text_msg(role: &str, content: &str) -> Value {
        json!({ "role": role, "content": content })
    }

    fn user_text(text: &str) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            usage: None,
        }
    }

    #[test]
    fn truncate_keeps_everything_when_under_budget() {
        let messages = vec![
            text_msg("user", "hello"),
            text_msg("assistant", "hi"),
            text_msg("user", "how are you"),
        ];
        let kept = truncate_to_budget(messages, 1000);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[0]["content"], "hello");
        assert_eq!(kept[2]["content"], "how are you");
    }

    #[test]
    fn truncate_drops_oldest_first() {
        // Each message ~10 chars; budget 25 should keep newest 2 (≈20 chars)
        // and drop the oldest.
        let messages = vec![
            text_msg("user", "first-old0"),     // 10 chars
            text_msg("assistant", "second-mi"), // 9 chars
            text_msg("user", "third-new0"),     // 10 chars
        ];
        let kept = truncate_to_budget(messages, 25);
        assert_eq!(kept.len(), 2, "expected 2 kept, got {kept:?}");
        assert_eq!(kept[0]["content"], "second-mi");
        assert_eq!(kept[1]["content"], "third-new0");
    }

    #[test]
    fn truncate_zero_budget_still_keeps_newest() {
        // Regression: with the always-keep-newest rule, even a 0 budget
        // returns the most recent message rather than dropping everything.
        // The next turn's auto_compaction is the real long-term cleanup.
        let messages = vec![text_msg("user", "anything")];
        let kept = truncate_to_budget(messages, 0);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["content"], "anything");
    }

    #[test]
    fn truncate_empty_input_returns_empty() {
        let kept = truncate_to_budget(Vec::new(), 1000);
        assert!(kept.is_empty());
    }

    #[test]
    fn truncate_keeps_oversized_newest_alone() {
        // Regression for the "lets explore Downloads" bug: a single message
        // bigger than the budget must NOT cause the truncator to return
        // empty. Better to send one oversized request than to lose the
        // current turn entirely.
        let messages = vec![text_msg("user", "way too long for the budget")];
        let kept = truncate_to_budget(messages, 5);
        assert_eq!(kept.len(), 1, "newest must always survive");
        assert_eq!(kept[0]["content"], "way too long for the budget");
    }

    #[test]
    fn truncate_skips_oversized_older_keeps_smaller_oldest() {
        // Regression for the `break`-on-overflow bug: when an OLDER message
        // is too big to fit, we must skip it (not abort the walk) so that
        // even-older smaller messages can still survive. Previously a giant
        // tool result in the middle of a session wiped the entire history.
        let messages = vec![
            text_msg("user", "tiny old"),            // 8 chars
            text_msg("assistant", &"X".repeat(500)), // 500 chars (oversized)
            text_msg("user", "newest"),              // 6 chars
        ];
        // Budget 30: room for newest (6) + tiny old (8) = 14, well under.
        // But the oversized middle (500) doesn't fit and must be skipped.
        let kept = truncate_to_budget(messages, 30);
        assert_eq!(kept.len(), 2, "kept: {kept:?}");
        assert_eq!(kept[0]["content"], "tiny old");
        assert_eq!(kept[1]["content"], "newest");
    }

    #[test]
    fn estimate_message_chars_counts_content_and_tool_calls() {
        let plain = text_msg("user", "hello"); // 5 chars
        assert_eq!(estimate_message_chars(&plain), 5);

        let with_tools = json!({
            "role": "assistant",
            "content": "ok",
            "tool_calls": [{ "id": "x", "type": "function", "function": { "name": "f", "arguments": {} }}],
        });
        // 2 chars content + JSON-encoded tool_calls length
        let chars = estimate_message_chars(&with_tools);
        assert!(chars > 2, "expected >2, got {chars}");
    }

    #[test]
    fn build_messages_always_keeps_system_prompt_and_newest() {
        // System prompt always kept (separate path); the newest history
        // message also always survives even at budget=0 thanks to the
        // always-keep-newest rule in truncate_to_budget.
        let request = ApiRequest {
            messages: vec![user_text("this is the only thing the user said")],
            system_prompt: vec!["you are an assistant".to_string()],
        };
        let result = build_messages(&request, 0);
        assert_eq!(result.len(), 2, "expected system + newest, got {result:?}");
        assert_eq!(result[0]["role"], "system");
        assert_eq!(result[0]["content"], "you are an assistant");
        assert_eq!(result[1]["content"], "this is the only thing the user said");
    }

    #[test]
    fn build_messages_truncates_history_under_budget() {
        let request = ApiRequest {
            messages: vec![
                user_text("ancient turn that should fall off"),
                user_text("middle turn that should also fall off"),
                user_text("newest"),
            ],
            system_prompt: vec!["sys".to_string()],
        };
        // Budget large enough only for the last message (~6 chars).
        let result = build_messages(&request, 20);
        assert_eq!(
            result.len(),
            2,
            "expected system + 1 history, got {result:?}"
        );
        assert_eq!(result[0]["role"], "system");
        assert_eq!(result[1]["content"], "newest");
    }

    #[test]
    fn history_budget_shrinks_with_larger_system_prompt() {
        let mut client = OllamaApiClient::new("test", json!([]));
        client.num_ctx = 1000; // 4000 chars total
        client.num_predict = 100; // 400 chars output reservation

        let small_sys = ApiRequest {
            messages: Vec::new(),
            system_prompt: vec!["short".to_string()],
        };
        let big_sys = ApiRequest {
            messages: Vec::new(),
            system_prompt: vec!["x".repeat(500)],
        };
        let small_budget = client.history_budget_chars(&small_sys);
        let big_budget = client.history_budget_chars(&big_sys);
        assert!(
            small_budget > big_budget,
            "smaller system prompt should leave more room for history"
        );
        assert!(big_budget + 500 <= small_budget + 10);
    }

    // === Streaming tests ====================================================
    //
    // `consume_stream_lines` is generic over `BufRead`, so we hand it a
    // `Cursor<Vec<u8>>` containing fake NDJSON instead of a real HTTP body.
    // This exercises the parser, the text-delta callback, and the event
    // assembly without ever touching a network or an Ollama install.

    use std::io::Cursor;

    fn fake_stream(lines: &[&str]) -> Cursor<Vec<u8>> {
        Cursor::new(lines.join("\n").into_bytes())
    }

    #[test]
    fn stream_text_only_single_chunk() {
        let client = OllamaApiClient::new("test", json!([]));
        let stream = fake_stream(&[
            r#"{"message":{"role":"assistant","content":"Hello"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":10,"eval_count":3}"#,
        ]);
        let events = client.consume_stream_lines(stream).unwrap();
        assert_eq!(events.len(), 3);
        match &events[0] {
            AssistantEvent::TextDelta(t) => assert_eq!(t, "Hello"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
        match &events[1] {
            AssistantEvent::Usage(u) => {
                assert_eq!(u.input_tokens, 10);
                assert_eq!(u.output_tokens, 3);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
        assert!(matches!(events[2], AssistantEvent::MessageStop));
    }

    #[test]
    fn stream_text_accumulates_multiple_deltas() {
        let client = OllamaApiClient::new("test", json!([]));
        let stream = fake_stream(&[
            r#"{"message":{"role":"assistant","content":"Hel"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":"lo, "},"done":false}"#,
            r#"{"message":{"role":"assistant","content":"world"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":5,"eval_count":7}"#,
        ]);
        let events = client.consume_stream_lines(stream).unwrap();
        match &events[0] {
            AssistantEvent::TextDelta(t) => assert_eq!(t, "Hello, world"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn stream_tool_call_on_done_chunk() {
        let client = OllamaApiClient::new("test", json!([]));
        let stream = fake_stream(&[
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"get_time","arguments":{}}}]},"done":true,"prompt_eval_count":20,"eval_count":2}"#,
        ]);
        let events = client.consume_stream_lines(stream).unwrap();
        // Expect: ToolUse, Usage, MessageStop — no TextDelta because the
        // content was empty.
        assert_eq!(events.len(), 3);
        match &events[0] {
            AssistantEvent::ToolUse { name, id, .. } => {
                assert_eq!(name, "get_time");
                assert_eq!(id, "call_1");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn stream_text_then_tool_call() {
        let client = OllamaApiClient::new("test", json!([]));
        let stream = fake_stream(&[
            r#"{"message":{"role":"assistant","content":"Let me check"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"id":"x","type":"function","function":{"name":"get_time","arguments":{}}}]},"done":true,"prompt_eval_count":15,"eval_count":4}"#,
        ]);
        let events = client.consume_stream_lines(stream).unwrap();
        assert_eq!(events.len(), 4);
        assert!(matches!(&events[0], AssistantEvent::TextDelta(t) if t == "Let me check"));
        assert!(matches!(&events[1], AssistantEvent::ToolUse { name, .. } if name == "get_time"));
    }

    #[test]
    fn stream_error_chunk_returns_error() {
        let client = OllamaApiClient::new("test", json!([]));
        let stream = fake_stream(&[r#"{"error":"model not found"}"#]);
        let result = client.consume_stream_lines(stream);
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(err.contains("model not found"), "got: {err}");
    }

    #[test]
    fn stream_missing_id_synthesises_one() {
        let client = OllamaApiClient::new("test", json!([]));
        // Some Ollama versions don't include an `id` on tool_calls. The
        // parser must synthesise one so the runtime's tool_use_id mapping
        // doesn't blow up.
        let stream = fake_stream(&[
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"type":"function","function":{"name":"a","arguments":{}}}]},"done":true,"prompt_eval_count":0,"eval_count":0}"#,
        ]);
        let events = client.consume_stream_lines(stream).unwrap();
        match &events[0] {
            AssistantEvent::ToolUse { id, .. } => {
                assert!(id.starts_with("call_"), "expected synthesised id, got {id}");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn stream_empty_returns_only_usage_and_stop() {
        let client = OllamaApiClient::new("test", json!([]));
        let stream = fake_stream(&[]);
        let events = client.consume_stream_lines(stream).unwrap();
        assert_eq!(events.len(), 2);
        match &events[0] {
            AssistantEvent::Usage(u) => {
                assert_eq!(u.input_tokens, 0);
                assert_eq!(u.output_tokens, 0);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
        assert!(matches!(events[1], AssistantEvent::MessageStop));
    }

    #[test]
    fn stream_callback_fires_per_delta_and_trailing_newline() {
        use std::sync::{Arc, Mutex};
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log_clone = log.clone();
        let cb: TextCallback = Box::new(move |s: &str| {
            log_clone.lock().unwrap().push(s.to_string());
        });
        let client = OllamaApiClient::new("test", json!([])).with_text_callback(cb);
        let stream = fake_stream(&[
            r#"{"message":{"role":"assistant","content":"foo"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":"bar"},"done":true,"prompt_eval_count":1,"eval_count":1}"#,
        ]);
        let _ = client.consume_stream_lines(stream).unwrap();
        let entries = log.lock().unwrap();
        assert_eq!(
            *entries,
            vec!["foo".to_string(), "bar".to_string(), "\n".to_string()],
            "callback should fire foo, bar, then trailing \\n"
        );
    }

    #[test]
    fn stream_callback_no_trailing_newline_when_only_tool_call() {
        use std::sync::{Arc, Mutex};
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log_clone = log.clone();
        let cb: TextCallback = Box::new(move |s: &str| {
            log_clone.lock().unwrap().push(s.to_string());
        });
        let client = OllamaApiClient::new("test", json!([])).with_text_callback(cb);
        let stream = fake_stream(&[
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"id":"x","type":"function","function":{"name":"a","arguments":{}}}]},"done":true,"prompt_eval_count":0,"eval_count":0}"#,
        ]);
        let _ = client.consume_stream_lines(stream).unwrap();
        let entries = log.lock().unwrap();
        assert!(
            entries.is_empty(),
            "no callbacks expected when content is empty (only a tool call), got {entries:?}"
        );
    }

    #[test]
    fn stream_skips_blank_lines() {
        let client = OllamaApiClient::new("test", json!([]));
        let stream = fake_stream(&[
            "",
            r#"{"message":{"role":"assistant","content":"hi"},"done":false}"#,
            "",
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":1,"eval_count":1}"#,
            "",
        ]);
        let events = client.consume_stream_lines(stream).unwrap();
        assert!(matches!(&events[0], AssistantEvent::TextDelta(t) if t == "hi"));
    }

    #[test]
    fn history_budget_subtracts_tools_schema() {
        // Regression: the `tools` field is sent to Ollama on every request
        // and counts against num_ctx. Omitting the subtraction caused the
        // budget to be ~2x reality, leading to context loss after a big
        // tool result. Verify two clients with identical settings but
        // different tool registry sizes produce different budgets.
        let request = ApiRequest {
            messages: Vec::new(),
            system_prompt: vec!["sys".to_string()],
        };
        // Use a large enough num_ctx that even the full 27-tool schema
        // (~12 K chars) doesn't saturate the budget to zero. With 4096 the
        // budget goes to 0 for the full-tools client (tools schema > budget)
        // and the delta test becomes meaningless.
        let mut empty_tools = OllamaApiClient::new("test", json!([]));
        empty_tools.num_ctx = 16384;
        empty_tools.num_predict = 1024;
        let mut full_tools = OllamaApiClient::new("test", crate::secretary_tools_json());
        full_tools.num_ctx = 16384;
        full_tools.num_predict = 1024;

        let empty_budget = empty_tools.history_budget_chars(&request);
        let full_budget = full_tools.history_budget_chars(&request);
        let tools_chars = crate::secretary_tools_json().to_string().len();

        assert!(
            full_budget < empty_budget,
            "tool registry must shrink the history budget"
        );
        // The delta should be (almost) exactly the tools-JSON char count.
        // Allow 4 chars of slack for the empty `[]` literal counted in the
        // empty case.
        let delta = empty_budget - full_budget;
        assert!(
            delta + 4 >= tools_chars && delta <= tools_chars + 4,
            "delta {delta} should approximately equal tools_chars {tools_chars}"
        );
    }

    // === OpenAI-compat tests ================================================

    #[test]
    fn resolve_openai_compat_unset_returns_false() {
        let prev = std::env::var("CLAUDETTE_OPENAI_COMPAT").ok();
        std::env::remove_var("CLAUDETTE_OPENAI_COMPAT");
        assert!(!resolve_openai_compat());
        if let Some(v) = prev {
            std::env::set_var("CLAUDETTE_OPENAI_COMPAT", v);
        }
    }

    #[test]
    fn resolve_openai_compat_set_to_one_returns_true() {
        let prev = std::env::var("CLAUDETTE_OPENAI_COMPAT").ok();
        std::env::set_var("CLAUDETTE_OPENAI_COMPAT", "1");
        assert!(resolve_openai_compat());
        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_OPENAI_COMPAT", v),
            None => std::env::remove_var("CLAUDETTE_OPENAI_COMPAT"),
        }
    }

    #[test]
    fn resolve_openai_compat_set_to_zero_returns_false() {
        let prev = std::env::var("CLAUDETTE_OPENAI_COMPAT").ok();
        std::env::set_var("CLAUDETTE_OPENAI_COMPAT", "0");
        assert!(!resolve_openai_compat());
        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_OPENAI_COMPAT", v),
            None => std::env::remove_var("CLAUDETTE_OPENAI_COMPAT"),
        }
    }

    #[test]
    fn build_chat_body_compat_uses_openai_shape() {
        let client = OllamaApiClient::new("openai/gpt-oss-20b", json!([])).with_openai_compat(true);
        let req = ApiRequest {
            messages: vec![user_text("hi")],
            system_prompt: vec!["sys".to_string()],
        };
        let body = client.build_chat_body(&req);
        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["temperature"], json!(0.0));
        assert!(body.get("max_tokens").is_some(), "max_tokens missing");
        assert!(
            body.get("think").is_none(),
            "think field must NOT be sent in compat mode"
        );
        assert!(
            body.get("options").is_none(),
            "options.* must NOT be sent in compat mode"
        );
    }

    #[test]
    fn build_chat_body_default_stays_ollama_shape() {
        let client = OllamaApiClient::new("qwen3.5:4b", json!([]));
        let req = ApiRequest {
            messages: vec![user_text("hi")],
            system_prompt: vec!["sys".to_string()],
        };
        let body = client.build_chat_body(&req);
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["think"], json!(false));
        assert!(
            body.get("options").is_some(),
            "options.* required for ollama"
        );
        assert!(
            body.get("max_tokens").is_none(),
            "max_tokens is openai-only"
        );
    }

    #[test]
    fn parse_openai_response_text_only() {
        let client = OllamaApiClient::new("test", json!([])).with_openai_compat(true);
        let body = json!({
            "id": "chatcmpl-x",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hello world"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13}
        });
        let events = client.parse_openai_response(&body).unwrap();
        assert_eq!(events.len(), 3);
        match &events[0] {
            AssistantEvent::TextDelta(t) => assert_eq!(t, "Hello world"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
        match &events[1] {
            AssistantEvent::Usage(u) => {
                assert_eq!(u.input_tokens, 10);
                assert_eq!(u.output_tokens, 3);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
        assert!(matches!(events[2], AssistantEvent::MessageStop));
    }

    #[test]
    fn parse_openai_response_with_tool_calls() {
        let client = OllamaApiClient::new("test", json!([])).with_openai_compat(true);
        // OpenAI emits function.arguments as a JSON-encoded STRING (note the
        // outer quotes on the arguments value), unlike Ollama which uses a
        // nested object.
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "get_time",
                            "arguments": "{\"tz\":\"UTC\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 50, "completion_tokens": 12}
        });
        let events = client.parse_openai_response(&body).unwrap();
        // Expect: ToolUse, Usage, MessageStop — no TextDelta (content was null).
        assert_eq!(events.len(), 3);
        match &events[0] {
            AssistantEvent::ToolUse { id, name, input } => {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "get_time");
                assert_eq!(input, "{\"tz\":\"UTC\"}");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_openai_response_text_then_tool_call() {
        let client = OllamaApiClient::new("test", json!([])).with_openai_compat(true);
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Let me check the time.",
                    "tool_calls": [{
                        "id": "x",
                        "type": "function",
                        "function": {"name": "get_time", "arguments": "{}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let events = client.parse_openai_response(&body).unwrap();
        assert_eq!(events.len(), 4); // text, tool, usage(0), stop
        assert!(
            matches!(&events[0], AssistantEvent::TextDelta(t) if t == "Let me check the time.")
        );
        assert!(matches!(&events[1], AssistantEvent::ToolUse { name, .. } if name == "get_time"));
    }

    #[test]
    fn parse_openai_response_error_field_returns_err() {
        let client = OllamaApiClient::new("test", json!([])).with_openai_compat(true);
        let body =
            json!({"error": {"message": "model not found", "type": "invalid_request_error"}});
        let result = client.parse_openai_response(&body);
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(err.contains("model not found"), "got: {err}");
    }

    #[test]
    fn parse_openai_response_missing_choices_is_err() {
        let client = OllamaApiClient::new("test", json!([])).with_openai_compat(true);
        let body = json!({"id": "x", "object": "chat.completion"});
        let result = client.parse_openai_response(&body);
        assert!(result.is_err());
    }

    #[test]
    fn parse_openai_response_missing_id_synthesises_one() {
        let client = OllamaApiClient::new("test", json!([])).with_openai_compat(true);
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "type": "function",
                        "function": {"name": "a", "arguments": "{}"}
                    }]
                }
            }]
        });
        let events = client.parse_openai_response(&body).unwrap();
        match &events[0] {
            AssistantEvent::ToolUse { id, .. } => {
                assert!(id.starts_with("call_"), "expected synthesised id, got {id}");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_openai_response_callback_fires_with_full_text() {
        use std::sync::{Arc, Mutex};
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log_clone = log.clone();
        let cb: TextCallback = Box::new(move |s: &str| {
            log_clone.lock().unwrap().push(s.to_string());
        });
        let client = OllamaApiClient::new("test", json!([]))
            .with_openai_compat(true)
            .with_text_callback(cb);
        let body = json!({
            "choices": [{
                "message": {"role": "assistant", "content": "foo bar"}
            }]
        });
        let _ = client.parse_openai_response(&body).unwrap();
        let entries = log.lock().unwrap();
        assert_eq!(
            *entries,
            vec!["foo bar".to_string(), "\n".to_string()],
            "callback should fire full text + trailing newline (no per-token streaming yet)"
        );
    }

    #[test]
    fn dynamic_registry_budget_shrinks_when_group_is_enabled() {
        // Regression: after Sprint 8 the main client sources tools from a
        // shared Arc<Mutex<ToolRegistry>>. Enabling a group between turns
        // must shrink `history_budget_chars` on the very next call, because
        // the `tools` field has grown and eats into the context budget.
        use crate::tool_groups::{ToolGroup, ToolRegistry};

        let registry = Arc::new(Mutex::new(ToolRegistry::new()));
        let mut client = OllamaApiClient::with_registry("test", registry.clone());
        client.num_ctx = 16384;
        client.num_predict = 1024;

        let request = ApiRequest {
            messages: Vec::new(),
            system_prompt: vec!["sys".to_string()],
        };

        let before = client.history_budget_chars(&request);
        registry.lock().unwrap().enable(ToolGroup::Git);
        let after = client.history_budget_chars(&request);

        assert!(
            after < before,
            "enabling a tool group must shrink the history budget (before={before}, after={after})"
        );
    }
}
