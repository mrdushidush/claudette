//! Core types shared across forge stages.
//!
//! Ported verbatim from `claudettes-forge/crates/core/src/types.rs` at the
//! `rc1-final` tag. These are the domain vocabulary — `Mission`, `Task`,
//! `Role`, `Complexity`, `ProviderKind`, etc. No behaviour, just data.

use std::path::PathBuf;

// ─── Mission / task hierarchy ────────────────────────────────────────

/// A natural-language request from the user that may decompose into one or
/// more `Subtask`s when run through forge mode.
#[derive(Debug, Clone)]
pub struct Mission {
    /// Unique identifier for this mission run.
    pub id: MissionId,
    /// The user's original prompt, verbatim.
    pub prompt: String,
    /// Per-role model assignment, resolved from CLI/TOML/env/preset at mission
    /// start. See `providers::ModelMap`.
    pub model_map: ModelMap,
    /// Complexity score assigned by the Router (forge mode only).
    pub complexity: Option<Complexity>,
    /// Isolation dir for mission artifacts, typically
    /// `generated/<mission-id>/`.
    pub workspace: PathBuf,
}

/// A mission-unique identifier. Used as a branch name, a directory name, and
/// a pipeline-report key — kept short and filesystem-safe.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MissionId(pub String);

/// A subtask produced by the Planner stage. Each subtask targets one file or
/// one unit of work; the Coder stage handles each independently.
#[derive(Debug, Clone)]
pub struct Subtask {
    /// Stable ordinal within the mission (1-indexed).
    pub index: usize,
    /// Target path relative to the mission workspace.
    pub target: PathBuf,
    /// Natural-language description passed to the Coder.
    pub description: String,
    /// Per-subtask complexity (may differ from mission-level).
    pub complexity: Complexity,
}

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

/// Campbell Complexity scale (1-10). Routes to context-window sizing and
/// model tier. C1-C6 → local small models; C7-C9 → local large or cloud;
/// C10 → cloud-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Complexity {
    /// Trivial tasks — single-line changes, constants.
    C1,
    /// Simple tasks — one small function, no branching.
    C2,
    /// Low complexity — small module, single file, no concurrency.
    C3,
    /// Moderate — multi-function file, well-known patterns.
    C4,
    /// Medium — small feature across 2-3 files.
    C5,
    /// Above medium — cross-module feature, some state.
    C6,
    /// High — new subsystem, concurrency, non-obvious interactions.
    C7,
    /// Hard — architectural decision required, tradeoff analysis.
    C8,
    /// Very hard — novel algorithm, performance-critical, unclear requirements.
    C9,
    /// Expert — research-grade problem, requires domain knowledge the model
    /// doesn't have out of the box.
    C10,
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

// ─── Tool call + result vocabulary ───────────────────────────────────

/// A tool invocation as emitted by the model.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// Tool name (must match a registered `Tool`).
    pub name: String,
    /// Arguments as JSON (every provider is coerced to this format before
    /// it reaches the dispatcher).
    pub arguments: serde_json::Value,
    /// Unique ID for matching the response (important for providers that
    /// support parallel tool calling).
    pub call_id: String,
}

/// A tool's response back to the model.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// ID from the matching `ToolCall`.
    pub call_id: String,
    /// Serialized output (JSON string or plain text).
    pub content: String,
    /// Whether the tool ran to completion; `false` means the content is an
    /// error string and the model should adapt.
    pub success: bool,
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
