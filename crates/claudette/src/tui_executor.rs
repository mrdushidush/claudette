//! `TuiToolExecutor` — wraps `SecretaryToolExecutor` and fires `TuiEvent`s
//! before and after every tool call so the TUI can show live tool activity.

use std::sync::mpsc::SyncSender;
use std::time::Instant;

use crate::{ToolError, ToolExecutor};

use crate::executor::SecretaryToolExecutor;
use crate::tui_events::TuiEvent;

/// Executor wired to the TUI. Delegates every call to the inner secretary
/// executor and emits `ToolCallStart` / `ToolCallDone` events on the channel.
pub struct TuiToolExecutor {
    inner: SecretaryToolExecutor,
    tx: SyncSender<TuiEvent>,
}

impl TuiToolExecutor {
    #[must_use]
    pub fn new(inner: SecretaryToolExecutor, tx: SyncSender<TuiEvent>) -> Self {
        Self { inner, tx }
    }
}

impl ToolExecutor for TuiToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        let input_preview: String = input.chars().take(60).collect();
        let _ = self.tx.send(TuiEvent::ToolCallStart {
            name: tool_name.to_string(),
            input_preview,
        });

        let start = Instant::now();
        let result = self.inner.execute(tool_name, input);
        let elapsed_ms = start.elapsed().as_millis() as u64;

        let (result_preview, ok): (String, bool) = match &result {
            Ok(s) => (s.chars().take(80).collect(), true),
            Err(e) => (e.to_string().chars().take(80).collect(), false),
        };
        let _ = self.tx.send(TuiEvent::ToolCallDone {
            name: tool_name.to_string(),
            result_preview,
            ok,
            elapsed_ms,
        });

        result
    }
}
