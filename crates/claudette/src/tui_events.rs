//! TUI event types — messages from the worker thread to the render loop,
//! and user inputs from the render loop back to the worker thread.

/// Events fired by the worker thread and polled by the render loop.
#[derive(Debug)]
pub enum TuiEvent {
    /// Streaming text delta — one or more tokens from the model.
    Token(String),
    /// A full turn completed.
    TurnComplete {
        text: String,
        iterations: u32,
        in_tok: u32,
        out_tok: u32,
    },
    /// A tool call started (before the tool runs).
    ToolCallStart { name: String, input_preview: String },
    /// A tool call finished.
    ToolCallDone {
        name: String,
        result_preview: String,
        ok: bool,
        elapsed_ms: u64,
    },
    /// Session was auto-compacted; `removed` messages were summarised.
    Compacted { removed: usize },
    /// Session was persisted to disk.
    Saved,
    /// A turn failed with this error message.
    TurnError(String),
    /// Worker is actively running a turn (true) or idle (false).
    Working(bool),
    /// Current estimated session token count + compaction threshold.
    TokensUpdate { estimated: usize, threshold: usize },
    /// Worker rebuilt the runtime from a fresh session (response to /clear).
    SessionReset,
    /// Informational text from a slash command (e.g. /help, /status, /tools).
    /// Rendered as a non-error system message in the chat history.
    Info(String),
    /// The worker thread is blocked inside a permission prompt for a
    /// `DangerFullAccess` tool. The render loop must show the confirmation
    /// modal and answer over `resp_tx` — NOT over `UserInput`, because the
    /// worker owns that receiver and cannot read it while parked inside
    /// `run_turn`.
    ///
    /// Each request carries its own rendezvous channel, so a buffered or
    /// stale answer from an earlier prompt can never satisfy a later one,
    /// and any render-loop exit path that drops the prompt automatically
    /// denies it (the worker's `recv()` sees `Disconnected`).
    PermissionRequest {
        tool_name: String,
        /// The tool's full input — never truncated here. The render side
        /// owns presentation (wrap + scroll), so no payload can hide past
        /// a preview edge (same rationale as `CliPrompter`).
        input: String,
        /// Display string of the tier the tool requires
        /// (e.g. "danger-full-access").
        required_mode: String,
        /// Answer channel: `true` → allow, `false` → deny.
        resp_tx: std::sync::mpsc::SyncSender<bool>,
    },
}

/// One image attached to a user turn — base64-encoded payload paired with
/// its MIME type. Both transports (Ollama `images: [b64,…]` and
/// OpenAI-compat `image_url` data URLs) consume this directly.
#[derive(Debug, Clone)]
pub struct ImageAttachment {
    pub media_type: String,
    pub data_b64: String,
}

/// Commands sent from the TUI render loop to the worker thread.
#[derive(Debug)]
pub enum UserInput {
    /// User submitted a text message to send to the model. `images` is
    /// empty for plain-text turns and non-empty when the user pasted /
    /// drag-dropped image attachments before pressing Enter.
    Message {
        text: String,
        images: Vec<ImageAttachment>,
    },
    /// User typed a slash command (e.g. `clear` for `/clear`).
    SlashCommand(String),
    /// User quit the TUI.
    Quit,
}
