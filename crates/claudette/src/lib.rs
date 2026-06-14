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
#[path = "runtime/session.rs"]
pub mod session;
#[path = "runtime/usage.rs"]
pub mod usage;

// ── Claudette secretary layer ────────────────────────────────────────────
pub mod api;
pub mod brain_selector;
pub mod briefing;
pub mod clock;
pub mod codet;
pub mod commands;
pub mod doctor;
pub mod egress;
pub mod executor;
pub mod firstrun;
pub mod forge;
// External-cloud integrations — gated behind the default-on `integrations`
// feature so a `--no-default-features` build is coding-only (no Google/Telegram
// code compiled in). See Cargo.toml `[features]`.
#[cfg(feature = "integrations")]
pub mod google_auth;
pub mod hw;
pub mod image_attach;
pub mod memory;
pub mod missions;
pub mod model_config;
pub mod prompt;
pub mod recall;
pub mod run;
pub mod scheduler;
pub mod secrets;
pub mod security_review;
pub mod status;
#[cfg(feature = "integrations")]
pub mod telegram_mode;
pub mod test_runner;
pub mod theme;
pub mod tool_groups;
pub mod tools;
pub mod transcript;
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
pub use prompt::{
    forge_system_prompt, secretary_system_prompt, secretary_system_prompt_with_memory,
};
pub use run::{
    default_session_path, run_forge_mission, run_secretary, run_secretary_repl, save_session,
    save_session_at, try_load_session, try_load_session_at, SessionOptions,
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

/// Run `f` with `HOME`/`USERPROFILE` swapped to a fresh temp dir, then
/// restore. Shared by every test that touches `~/.claudette` (transcript,
/// notes, todos, file_ops, executor). Holds [`test_env_lock`] for the whole
/// closure so parallel tests can't race the env mutation.
#[cfg(test)]
pub(crate) fn with_temp_home<F, T>(f: F) -> T
where
    F: FnOnce(&std::path::Path) -> T,
{
    let _guard = test_env_lock();
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";
    let prev = std::env::var(key).ok();
    let tmp = std::env::temp_dir().join(format!(
        "claudette-temphome-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_var(key, &tmp);
    let out = f(&tmp);
    match prev {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
    let _ = std::fs::remove_dir_all(&tmp);
    out
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
