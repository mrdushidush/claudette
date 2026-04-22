//! `ToolExecutor` implementation that dispatches secretary tools.
//!
//! Sprint 8 wired a shared [`ToolRegistry`] through this executor so the
//! `enable_tools` meta-tool can mutate the registry in place. Every other
//! tool still routes through the stateless [`crate::tools::dispatch_tool`].

use std::sync::{Arc, Mutex};

use crate::{ToolError, ToolExecutor};
use serde_json::{json, Value};

use crate::tool_groups::{ToolGroup, ToolRegistry};
use crate::tools::dispatch_tool;

/// Executor for the main Claudette runtime. Holds an `Arc<Mutex<ToolRegistry>>`
/// shared with the `OllamaApiClient` so that `enable_tools` calls mutate the
/// same registry the client reads from on the next request.
///
/// `Default::default()` is intentionally **not** implemented — the registry
/// must be built once per runtime and shared, so callers always go through
/// [`Self::with_registry`] (or [`Self::stateless`] for tests and agents).
pub struct SecretaryToolExecutor {
    /// Shared registry. `None` = agents / tests that don't want the
    /// `enable_tools` feature; in that mode `enable_tools` calls return an
    /// error so the model can adapt.
    registry: Option<Arc<Mutex<ToolRegistry>>>,
}

impl SecretaryToolExecutor {
    /// Build an executor wired to a shared tool registry. `enable_tools`
    /// calls will mutate `registry` and be visible to subsequent
    /// `OllamaApiClient::build_chat_body` calls that read from the same
    /// `Arc<Mutex<_>>`.
    #[must_use]
    pub fn with_registry(registry: Arc<Mutex<ToolRegistry>>) -> Self {
        Self {
            registry: Some(registry),
        }
    }

    /// Build a stateless executor. `enable_tools` is not wired up — useful
    /// for tests, for agents (who have a fixed tool allowlist), and for the
    /// old single-shot path that pre-dates Sprint 8.
    #[must_use]
    pub fn stateless() -> Self {
        Self { registry: None }
    }

    /// Back-compat shim for the old API. Now returns the stateless variant;
    /// the main runtime always goes through [`Self::with_registry`].
    #[must_use]
    pub fn new() -> Self {
        Self::stateless()
    }
}

impl Default for SecretaryToolExecutor {
    fn default() -> Self {
        Self::stateless()
    }
}

impl ToolExecutor for SecretaryToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        if tool_name == "enable_tools" {
            return run_enable_tools(self.registry.as_ref(), input).map_err(ToolError::new);
        }
        dispatch_tool(tool_name, input)
            .map(|result| format!("[tool:{tool_name}] {result}"))
            .map_err(ToolError::new)
    }
}

/// Handle a call to the synthetic `enable_tools` meta-tool. Parses the
/// `group` argument, flips the enabled bit on the shared registry, and
/// returns a JSON result listing the tools that are now available so the
/// model knows what to call on the next turn.
fn run_enable_tools(
    registry: Option<&Arc<Mutex<ToolRegistry>>>,
    input: &str,
) -> Result<String, String> {
    let Some(registry) = registry else {
        return Err(
            "enable_tools is not available in this runtime (stateless executor)".to_string(),
        );
    };

    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("enable_tools: invalid JSON input ({e}): {input}"))?;
    let group_name = v
        .get("group")
        .and_then(Value::as_str)
        .ok_or("enable_tools: missing 'group' parameter")?;
    let group = ToolGroup::parse(group_name).ok_or_else(|| {
        // Dynamically enumerate all groups so adding a new one in
        // `tool_groups.rs` doesn't silently leave this error message
        // pointing at a stale list. A hard-coded subset was wrong
        // for months; never again.
        let available: Vec<&str> = ToolGroup::all().iter().map(|g| g.name()).collect();
        format!(
            "enable_tools: unknown group '{group_name}' — available: {}",
            available.join(", ")
        )
    })?;

    // Poisoned lock means another thread panicked with the registry held;
    // the mutation model here is single-threaded (the runtime drives calls
    // serially) so poisoning in practice means a test panicked. Fall back
    // to the inner payload so we stay operational.
    let mut reg = match registry.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    let newly_enabled = reg.enable(group);
    let tool_names = reg.group_tool_names(group);
    let current_count = reg.current_len();

    Ok(json!({
        "ok": true,
        "group": group.name(),
        "already_enabled": !newly_enabled,
        "tools_now_available": tool_names,
        "total_advertised_tools": current_count,
        "note": "The new tools take effect on the next model call — call them directly on your next turn.",
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolExecutor;

    #[test]
    fn stateless_executor_rejects_enable_tools() {
        let mut exec = SecretaryToolExecutor::stateless();
        let result = exec.execute("enable_tools", r#"{"group":"git"}"#);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not available"),
            "expected 'not available', got: {err}"
        );
    }

    #[test]
    fn stateless_executor_dispatches_core_tool() {
        let mut exec = SecretaryToolExecutor::stateless();
        let result = exec.execute("get_current_time", "{}");
        assert!(result.is_ok(), "core tools should still work: {result:?}");
        assert!(result.unwrap().contains("iso8601"));
    }

    #[test]
    fn wired_executor_enables_git_group() {
        let registry = Arc::new(Mutex::new(ToolRegistry::new()));
        let mut exec = SecretaryToolExecutor::with_registry(registry.clone());

        assert!(!registry.lock().unwrap().is_enabled(ToolGroup::Git));

        let result = exec.execute("enable_tools", r#"{"group":"git"}"#).unwrap();
        assert!(result.contains("\"ok\":true"));
        assert!(result.contains("git_status"));

        assert!(registry.lock().unwrap().is_enabled(ToolGroup::Git));
    }

    #[test]
    fn wired_executor_reports_already_enabled_on_second_call() {
        let registry = Arc::new(Mutex::new(ToolRegistry::new()));
        let mut exec = SecretaryToolExecutor::with_registry(registry);

        let first = exec.execute("enable_tools", r#"{"group":"ide"}"#).unwrap();
        assert!(first.contains("\"already_enabled\":false"));

        let second = exec.execute("enable_tools", r#"{"group":"ide"}"#).unwrap();
        assert!(second.contains("\"already_enabled\":true"));
    }

    #[test]
    fn wired_executor_unknown_group_errors_clearly() {
        let registry = Arc::new(Mutex::new(ToolRegistry::new()));
        let mut exec = SecretaryToolExecutor::with_registry(registry);
        // Use a name that is not a valid group or alias.
        let err = exec
            .execute("enable_tools", r#"{"group":"does-not-exist-xyz"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown group"), "got: {err}");
        // Every registered group must appear in the "available" list — guards
        // against the hardcoded-subset regression the dynamic formatter was
        // added to fix.
        for group in ToolGroup::all() {
            assert!(
                err.contains(group.name()),
                "error should list group '{}': {err}",
                group.name()
            );
        }
    }

    #[test]
    fn wired_executor_missing_group_param_errors() {
        let registry = Arc::new(Mutex::new(ToolRegistry::new()));
        let mut exec = SecretaryToolExecutor::with_registry(registry);
        let err = exec.execute("enable_tools", "{}").unwrap_err().to_string();
        assert!(err.contains("missing 'group'"), "got: {err}");
    }

    #[test]
    fn wired_executor_bad_json_errors() {
        let registry = Arc::new(Mutex::new(ToolRegistry::new()));
        let mut exec = SecretaryToolExecutor::with_registry(registry);
        let err = exec
            .execute("enable_tools", "not json at all")
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid JSON"), "got: {err}");
    }

    #[test]
    fn wired_executor_still_dispatches_non_meta_tools() {
        let registry = Arc::new(Mutex::new(ToolRegistry::new()));
        let mut exec = SecretaryToolExecutor::with_registry(registry);
        let result = exec.execute("get_current_time", "{}").unwrap();
        assert!(result.contains("iso8601"));
    }
}
