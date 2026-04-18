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
}

/// Commands sent from the TUI render loop to the worker thread.
#[derive(Debug)]
pub enum UserInput {
    /// User submitted a text message to send to the model.
    Message(String),
    /// User typed a slash command (e.g. `clear` for `/clear`).
    SlashCommand(String),
    /// User quit the TUI.
    Quit,
}
