//! Core types shared across forge stages.
//!
//! Originally ported verbatim from `claudettes-forge/crates/core/src/types.rs`
//! at the `rc1-final` tag. The pipeline-vocabulary types (`Mission`, `Subtask`,
//! `MissionId`, `Complexity`, `ToolCall`, `ToolResult`) were duplicates of
//! types claudette's runtime owns elsewhere (`crate::missions::Mission`,
//! `crate::tools::*`) and never reached the live orchestrator in `run.rs`;
//! they were dropped 2026-05-15 after the multi-agent audit. What remains
//! is what `run.rs` and `models_toml.rs` actually use: `Role`, `ModelMap`,
//! `ProviderKind`.

// ─── Role + complexity + providers ───────────────────────────────────

/// Roles the pipeline can assign work to. A single model can fill multiple
/// roles simultaneously — role naming is about *what the model is doing*,
/// not about which weights are loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Conversational assistant loop (default no-subcommand mode).
    Assistant,
    /// Mission decomposition, Campbell-complexity tagging.
    Planner,
    /// Complexity router + model selector.
    Router,
    /// Code generation.
    Coder,
    /// Test generation.
    TestCoder,
    /// Correctness grading.
    Verifier,
    /// Surgical fix-pass — patches specific compile/test failures.
    SurgicalCoder,
    /// Strategic review at Gate — ship / no-ship call.
    Cto,
}

/// Which provider the request dispatches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// Local Ollama via HTTP.
    Ollama,
    /// Anthropic Claude API (v0.2 feature-gated).
    AnthropicClaude,
}

/// Per-role model assignment. Resolved from CLI > TOML > env > preset.
/// Kept opaque at the type level — concrete resolution lives in
/// `providers::ModelConfig`.
#[derive(Debug, Clone, Default)]
pub struct ModelMap {
    /// Internal store: role → `(provider, model-name, options)`.
    entries: Vec<(Role, ProviderKind, String)>,
}

impl ModelMap {
    /// Empty map — every `resolve` call returns `None` until set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Assign a `(provider, model)` pair to a role.
    pub fn set(&mut self, role: Role, provider: ProviderKind, model: impl Into<String>) {
        // Last assignment wins — makes overlay/preset composition easy.
        self.entries.retain(|(r, _, _)| *r != role);
        self.entries.push((role, provider, model.into()));
    }

    /// Look up a role. Returns `None` if the role is unassigned.
    #[must_use]
    pub fn resolve(&self, role: Role) -> Option<(ProviderKind, &str)> {
        self.entries
            .iter()
            .rev() // last assignment wins
            .find(|(r, _, _)| *r == role)
            .map(|(_, p, m)| (*p, m.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_map_resolves_nothing() {
        let map = ModelMap::new();
        assert!(map.resolve(Role::Coder).is_none());
        assert!(map.resolve(Role::Planner).is_none());
    }

    #[test]
    fn set_then_resolve_round_trips() {
        let mut map = ModelMap::new();
        map.set(Role::Coder, ProviderKind::Ollama, "qwen3-coder:30b");
        let (kind, name) = map.resolve(Role::Coder).expect("coder should be set");
        assert_eq!(kind, ProviderKind::Ollama);
        assert_eq!(name, "qwen3-coder:30b");
    }

    #[test]
    fn set_twice_same_role_last_wins() {
        let mut map = ModelMap::new();
        map.set(Role::Planner, ProviderKind::Ollama, "qwen3.5:9b");
        map.set(
            Role::Planner,
            ProviderKind::AnthropicClaude,
            "claude-opus-4-7",
        );
        let (kind, name) = map.resolve(Role::Planner).unwrap();
        assert_eq!(kind, ProviderKind::AnthropicClaude);
        assert_eq!(name, "claude-opus-4-7");
    }

    #[test]
    fn multiple_roles_coexist() {
        let mut map = ModelMap::new();
        map.set(Role::Coder, ProviderKind::Ollama, "qwen3-coder:30b");
        map.set(Role::Planner, ProviderKind::Ollama, "qwen3.5:9b");
        map.set(Role::Cto, ProviderKind::AnthropicClaude, "claude-opus-4-7");

        assert_eq!(map.resolve(Role::Coder).unwrap().1, "qwen3-coder:30b");
        assert_eq!(map.resolve(Role::Planner).unwrap().1, "qwen3.5:9b");
        assert_eq!(map.resolve(Role::Cto).unwrap().1, "claude-opus-4-7");
        assert!(map.resolve(Role::Verifier).is_none());
    }
}
