//! Forge — dormant plumbing for forge-mode (NL mission → planner → coder → PR).
//!
//! Folded into the claudette workspace 2026-05-09 from the standalone
//! `claudettes-forge` repo (frozen at `rc1-final`). Three modules carried
//! over as dormant primitives:
//!
//! 1. [`personas`] — persona loader (markdown + TOML frontmatter).
//! 2. [`models_toml`] — role → (model, provider) mapping with env-var overrides.
//! 3. [`pipeline`] — Router → Planner → Coder → TestCoder → Verifier →
//!    SurgicalCoder → Gate skeleton. All stages are `pub mod` placeholders.
//!
//! **Status:** None of this is wired into claudette in 0.4.1. The crate
//! exists so the modules compile under workspace lints and are ready for
//! Theme D (forge-mode-as-brownfield) in a future sprint. There is no
//! `--persona` flag, no router invocation, no published `forge` crate.

#![forbid(unsafe_code)]

pub mod models_toml;
pub mod personas;
pub mod pipeline;
pub mod types;
