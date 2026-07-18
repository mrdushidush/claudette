//! Claudette — an air-gapped, local-first AI coding agent that drives one
//! local model (LM Studio or Ollama). The coding agent is the whole product:
//! the default build carries no cloud code. An optional personal-assistant
//! surface (Gmail/Calendar, Telegram, voice/tts, morning briefing) lives behind
//! the off-by-default `integrations` feature — opt in with
//! `cargo install claudette --features integrations`.
//!
//! This crate bundles the agent-loop/session/compaction/permissions kernel
//! (`src/runtime/*.rs`) and the Claudette tool/REPL/TUI layer (tools, REPL,
//! TUI, plus the feature-gated assistant integrations).
//! Single-crate — no path dependencies.
//!
//! Runtime modules are mounted at the crate root via `#[path = "runtime/..."]`
//! attributes so their internal `use crate::session::X` / `use crate::compact::X`
//! paths resolve without rewriting.

#![recursion_limit = "256"]
// Production code must not panic via `.unwrap()` — the binary builds with
// `panic = "abort"`, so a stray unwrap is a hard process crash, not a catchable
// error. `cfg_attr(not(test), …)` keeps the ban off `#[cfg(test)]` code, where
// `.unwrap()` is idiomatic. (Wave F.1 — production unwrap audit.)
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

// ── Embedded runtime kernel ──────────────────────────────────────────────
#[path = "runtime/compact.rs"]
pub mod compact;
#[path = "runtime/config.rs"]
pub mod config;
#[path = "runtime/context_evict.rs"]
pub mod context_evict;
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

// ── Claudette agent layer ─────────────────────────────────────────────────
pub mod api;
pub mod brain_selector;
// Morning-briefing helper — part of the personal-assistant surface, compiled
// only into an `integrations` build (the bot consumes it; `--briefing` sets it
// up). See Cargo.toml `[features]`.
#[cfg(feature = "integrations")]
pub mod briefing;
pub mod clock;
pub mod commands;
pub mod diff_preview;
pub mod doctor;
pub mod egress;
pub mod env_config;
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
pub mod redact;
pub mod run;
pub mod scheduler;
pub mod secrets;
pub mod security_review;
pub mod setup;
pub mod status;
#[cfg(feature = "integrations")]
pub mod telegram_mode;
pub mod test_runner;
pub mod theme;
pub mod tool_groups;
pub mod tools;
pub mod transcript;
// Text-to-speech for Telegram voice replies — only used by `telegram_mode`,
// so it rides the same `integrations` gate.
#[cfg(feature = "integrations")]
pub mod tts;
pub mod tui;
pub mod tui_events;
pub mod tui_executor;
pub mod tui_worker;
// Speech-to-text for inbound Telegram voice notes — only used by
// `telegram_mode`, so it rides the same `integrations` gate.
#[cfg(feature = "integrations")]
pub mod voice;

// ── Public re-exports ────────────────────────────────────────────────────
pub use api::{probe_ollama, resolve_ollama_url, OllamaApiClient};
pub use executor::AgentToolExecutor;
pub use memory::{default_memory_path, try_load_memory, try_load_memory_at, MAX_MEMORY_CHARS};
pub use prompt::{agent_system_prompt, agent_system_prompt_with_memory, forge_system_prompt};
pub use run::{
    default_session_path, run_agent, run_agent_repl, run_deep_research, run_forge_mission,
    save_session, save_session_at, try_load_session, try_load_session_at, SessionOptions,
};
pub use tools::{agent_tools_json, workspace_startup_diagnostics};

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
