//! Claudette — local-first AI personal secretary, powered by Ollama.
//!
//! This crate bundles the agent-loop/session/compaction/permissions kernel
//! (`src/runtime/*.rs`) and the Claudette secretary layer (tools, REPL, TUI,
//! Codet sidecar, agents, Telegram bot). Single-crate — no path dependencies.
//!
//! Runtime modules are mounted at the crate root via `#[path = "runtime/..."]`
//! attributes so their internal `use crate::session::X` / `use crate::compact::X`
//! paths resolve without rewriting.

#![recursion_limit = "256"]

// ── Embedded runtime kernel ──────────────────────────────────────────────
#[path = "runtime/compact.rs"]
pub mod compact;
#[path = "runtime/config.rs"]
pub mod config;
#[path = "runtime/conversation.rs"]
pub mod conversation;
#[path = "runtime/hooks.rs"]
pub mod hooks;
#[path = "runtime/json.rs"]
pub mod json;
#[path = "runtime/permissions.rs"]
pub mod permissions;
#[path = "runtime/prompt.rs"]
pub mod prompt_runtime;
#[path = "runtime/sandbox.rs"]
pub mod sandbox;
#[path = "runtime/session.rs"]
pub mod session;
#[path = "runtime/usage.rs"]
pub mod usage;

// ── Claudette secretary layer ────────────────────────────────────────────
pub mod agents;
pub mod api;
pub mod brain_selector;
pub mod briefing;
pub mod clock;
pub mod codet;
pub mod commands;
pub mod executor;
pub mod google_auth;
pub mod memory;
pub mod model_config;
pub mod prompt;
pub mod run;
pub mod scheduler;
pub mod secrets;
pub mod telegram_mode;
pub mod test_runner;
pub mod theme;
pub mod tool_groups;
pub mod tools;
pub mod tts;
pub mod tui;
pub mod tui_events;
pub mod tui_executor;
pub mod tui_worker;
pub mod voice;

// ── Public re-exports ────────────────────────────────────────────────────
pub use api::{probe_ollama, resolve_ollama_url, OllamaApiClient};
pub use executor::SecretaryToolExecutor;
pub use memory::{default_memory_path, try_load_memory, try_load_memory_at, MAX_MEMORY_CHARS};
pub use prompt::{secretary_system_prompt, secretary_system_prompt_with_memory};
pub use run::{
    default_session_path, run_secretary, run_secretary_repl, save_session, save_session_at,
    try_load_session, try_load_session_at, SessionOptions,
};
pub use tools::{secretary_tools_json, workspace_startup_diagnostics};

/// Process-wide lock for tests that mutate environment variables. Several
/// runtime tests call `crate::test_env_lock()` to serialise env-var mutation
/// across parallel test threads.
#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Bridge re-exports: claudette code imports these from `crate::`.
// In this crate they live at the root of the runtime sibling modules.
pub use compact::{compact_session, estimate_session_tokens, CompactionConfig};
pub use conversation::{
    ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, RuntimeError, ToolError,
    ToolExecutor, TurnSummary,
};
pub use permissions::{
    PermissionMode, PermissionOutcome, PermissionPolicy, PermissionPromptDecision,
    PermissionPrompter, PermissionRequest,
};
pub use prompt_runtime::ProjectContext;
pub use session::{ContentBlock, ConversationMessage, MessageRole, Session};
pub use usage::TokenUsage;
