//! Forge — dormant plumbing for forge-mode (NL mission → planner → coder → PR).
//!
//! Originally lived as a workspace-internal `forge` crate at
//! `crates/forge/`; folded into claudette in v0.5.1 so claudette can publish
//! to crates.io without a path-only workspace dependency (cargo rejects
//! path-only deps at publish time). Three modules survived the fold:
//!
//! 1. [`personas`] — persona loader (markdown + TOML frontmatter).
//! 2. [`models_toml`] — role → (model, provider) mapping with env-var overrides.
//! 3. [`types`] — Role / ProviderKind / ModelMap. The pipeline-vocabulary
//!    siblings (Mission/Subtask/MissionId/Complexity/ToolCall/ToolResult)
//!    were duplicates of types claudette's runtime owns elsewhere and were
//!    dropped 2026-05-15 after the multi-agent audit.
//!
//! The standalone-crate's `pipeline` module (`pub mod` stubs only) did not
//! carry over — it was 36 LoC of empty placeholders.

pub mod models_toml;
pub mod personas;
pub mod types;
