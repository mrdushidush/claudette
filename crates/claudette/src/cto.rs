//! CTO chat agent — the strategic layer above the forge fix-loop.
//!
//! The CTO persona makes three decisions per its bundled brief
//! (`crates/claudette/personas/cto.md`):
//! 1. **Decomposition** — given a mission, produce atomic subtasks with
//!    Campbell complexity scores.
//! 2. **Gate review** — given the outputs of Coder + Verifier, judge
//!    whether the mission is ready to ship (score ≥ 7, no critical findings).
//! 3. **Clarification** — when a request is ambiguous, ask the smallest
//!    number of clarifying questions that unblock the work.
//!
//! Phase 5a of `docs/sprint_import_2026_05_19.md` lands the **prompt
//! builders + persona overlay** so the runtime layer (Phase 5b) can be a
//! thin wrapper rather than a redesign. Specifically:
//!
//! - [`cto_decomposition_system_prompt`] builds the Decomposition prompt
//!   with optional active-mission grounding.
//! - [`cto_gate_review_system_prompt`] builds the Gate-Review prompt that
//!   ingests Coder + Verifier outputs.
//! - [`default_cto_persona`] bakes the bundled `cto.md` via `include_str!`
//!   so the binary doesn't depend on a runtime persona file.
//!
//! Phase 5b will:
//! - Refactor [`crate::run::build_runtime_with_brain`] to accept an
//!   explicit `system_prompt: Vec<String>` argument so a CTO turn can
//!   reuse the same plumbing the secretary uses without a parallel
//!   builder.
//! - Add `claudette --cto "<request>"` CLI handler.
//! - Wire `/cto` slash that opens a sub-conversation persisted to
//!   `~/.claudette/cto-sessions/<id>.jsonl`.
//!
//! Distinct from CodeX-7 (the coder persona, wired in
//! [`crate::run`]'s forge loop). CTO frames the *mission* itself;
//! CodeX-7 implements an already-decomposed subtask.

use anyhow::Result;

use crate::permissions::PermissionPrompter;
use crate::prompt::faceless_mode_enabled;
use crate::run::{
    build_runtime_with_brain_and_prompt, extract_assistant_text, CliPrompter, SessionOptions,
};
use crate::session::Session;
use crate::try_load_session;

/// Build the CTO Decomposition system prompt.
///
/// `mission_path` is `Some(path)` when an active brownfield mission gives
/// the CTO a concrete target tree to ground its plan against; `None` when
/// the user is asking for a high-level decomposition without a tree
/// (e.g. before `/brownfield` is run). Both shapes are valid — the
/// prompt adapts.
///
/// The persona overlay is appended verbatim from the bundled `cto.md` so
/// the model adopts the "strategic-authority" voice. Skipped under
/// `--faceless` / `CLAUDETTE_FACELESS=1` the same way Eva is.
#[must_use]
pub fn cto_decomposition_system_prompt(mission_path: Option<&str>) -> Vec<String> {
    let mission_line = match mission_path {
        Some(p) => format!(
            "Active mission tree: {p}. Ground subtasks against this tree's structure. \
             Use flat file paths only — `tasks/<filename>.<ext>`. "
        ),
        None => String::new(),
    };
    let base = format!(
        "You are the CTO at claudette's gate. The user has described a mission; your job \
         is decomposition. {mission_line}\
         Produce a maximum of 5 subtasks (hard cap 7). Each subtask: one file, one \
         validation command, one Campbell complexity score 1-10. Output ONE markdown \
         block in this shape:\n\n\
         ```\n\
         Mission summary: <one sentence>.\n\
         Complexity: C<1-10>\n\
         Clarifying questions: <0 if none, else list as 'Q1: ...'>\n\
         \n\
         Subtasks:\n\
         1. <target/path>  [C<n>]  <one-sentence description>. Validation: `<command>`.\n\
         2. ...\n\
         ```\n\
         \n\
         No preamble before the block, no commentary after it. If the request is so \
         simple it doesn't need decomposition, output one subtask. Honest scoring — \
         inflated complexity wastes routing budget, deflated complexity misses failures."
    );

    vec![append_persona_overlay(base)]
}

/// Build the CTO Gate-Review system prompt. Used when the forge pipeline
/// finishes and the CTO needs to make a ship / no-ship call.
///
/// The Verifier already grades the diff for correctness; the Gate Review
/// is a higher-level pass that weighs the verdict against mission intent,
/// surface-level risk (security, scope creep), and acknowledged tradeoffs.
/// Returns a structured one-line JSON verdict so the caller can parse it
/// the same way [`crate::run::parse_verifier_response`] parses Verifier
/// output.
#[must_use]
pub fn cto_gate_review_system_prompt(mission_path: &str) -> Vec<String> {
    let base = format!(
        "You are the CTO performing the Gate Review on a forge mission at {mission_path}. \
         The user message will contain three blocks: (1) ORIGINAL REQUEST, (2) FINAL DIFF, \
         (3) VERIFIER VERDICT. Read all three. Decide ship / no-ship. Approve if score ≥ 7 \
         and no critical findings. Block on any unhandled critical (security, data-loss, \
         unstated breaking change). Acknowledge tradeoffs explicitly in the summary — \
         'we chose B over A because B ships today and A would need a week' beats silence.\n\n\
         Output ONLY one line of JSON in this exact shape, no preamble, no trailing prose:\n\
         {{\"approved\": <bool>, \"score\": <int 0-10>, \"summary\": <string>, \
         \"findings\": [<string>, ...]}}. \
         You do not have access to tools."
    );

    vec![append_persona_overlay(base)]
}

/// Append the bundled CTO persona's voice + backstory to `base` unless the
/// user opted out via `--faceless` / `CLAUDETTE_FACELESS=1`. Mirrors the
/// Eva overlay pattern in [`crate::prompt::secretary_system_prompt_with_memory`].
fn append_persona_overlay(mut base: String) -> String {
    if faceless_mode_enabled() {
        return base;
    }
    let Some(persona) = default_cto_persona() else {
        return base;
    };
    use std::fmt::Write;
    let voice = persona.voice.trim();
    let backstory = persona.backstory.trim();
    if !voice.is_empty() {
        let _ = write!(base, "\n\nVoice: {voice}");
    }
    if !backstory.is_empty() {
        let _ = write!(base, "\n\nBackstory:\n{backstory}");
    }
    base
}

/// Bundled CTO persona — baked into the binary via `include_str!` so the
/// file isn't required at install time. Falls through silently when
/// parsing fails; the build-time `bundled_personas_all_parse` test guards
/// against shipping a broken CTO markdown.
#[must_use]
pub fn default_cto_persona() -> Option<crate::forge::personas::Persona> {
    const CTO: &str = include_str!("../personas/cto.md");
    crate::forge::personas::parse_persona_content(CTO, "bundled:cto").ok()
}

/// One-shot CTO decomposition turn against `user_input`. Tool-less — the
/// CTO emits a decomposition block as text, no file/git/shell side effects.
///
/// `opts.resume` behaves as in [`crate::run_secretary`]: when set, the
/// turn appends to the saved REPL session so the plan can be referenced
/// in subsequent assistant turns. Without `--resume` a fresh session is
/// used and the stdout print is the canonical output.
///
/// Returns the assistant text — the decomposition block — so the CLI can
/// print it verbatim. Errors only on session-load or model failure.
pub fn run_cto_decomposition(user_input: &str, opts: SessionOptions) -> Result<String> {
    let session = if opts.resume {
        try_load_session()?.ok_or_else(|| anyhow::anyhow!("no saved session to resume"))?
    } else {
        Session::default()
    };

    let mission_path =
        crate::missions::active_mission().map(|m| m.path.to_string_lossy().into_owned());
    let system = cto_decomposition_system_prompt(mission_path.as_deref());

    // Use the configured brain so the CTO turn benefits from `--brain` and
    // model-config overrides the same way the secretary does. Streaming
    // on (CTO output is large enough that incremental render helps); not
    // telegram (CLI-only entrypoint for now).
    let brain = crate::model_config::active().brain;
    let mut runtime = build_runtime_with_brain_and_prompt(session, &brain, true, false, system);

    let mut prompter = CliPrompter;
    let mut prompter_opt: Option<&mut dyn PermissionPrompter> = Some(&mut prompter);
    let summary =
        crate::brain_selector::run_turn_with_fallback(&mut runtime, user_input, &mut prompter_opt)
            .map_err(|e| anyhow::anyhow!("cto decomposition turn failed: {e}"))?;

    if opts.autosave {
        crate::run::save_session(runtime.session())?;
    }

    Ok(extract_assistant_text(&summary))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Decomposition prompt ──────────────────────────────────────────

    #[test]
    fn decomposition_prompt_includes_subtask_format() {
        let p = cto_decomposition_system_prompt(None);
        assert_eq!(p.len(), 1);
        let body = &p[0];
        assert!(body.contains("CTO"));
        assert!(body.contains("Subtasks"));
        assert!(body.contains("Validation"));
        assert!(body.contains("Campbell"));
    }

    #[test]
    fn decomposition_prompt_with_mission_path_declares_tree() {
        let p = cto_decomposition_system_prompt(Some("/tmp/m/sample"));
        assert!(p[0].contains("/tmp/m/sample"));
        assert!(p[0].contains("flat file paths"));
    }

    #[test]
    fn decomposition_prompt_without_mission_omits_path_block() {
        let p = cto_decomposition_system_prompt(None);
        assert!(!p[0].contains("Active mission tree:"));
        assert!(!p[0].contains("flat file paths"));
    }

    #[test]
    fn decomposition_prompt_caps_subtask_count() {
        let p = cto_decomposition_system_prompt(None);
        // The persona brief caps at 5 (hard cap 7); the prompt must echo
        // both numbers so the model doesn't blow past the limit.
        assert!(p[0].contains('5'), "expected '5' in: {}", p[0]);
        assert!(p[0].contains('7'), "expected '7' (hard cap) in: {}", p[0]);
    }

    #[test]
    fn decomposition_prompt_includes_persona_overlay_by_default() {
        let _lock = crate::test_env_lock();
        std::env::remove_var("CLAUDETTE_FACELESS");
        let p = cto_decomposition_system_prompt(None);
        assert!(p[0].contains("Voice:"));
        assert!(p[0].contains("strategic-authority"));
        assert!(p[0].contains("Backstory:"));
    }

    #[test]
    fn decomposition_prompt_skips_persona_under_faceless() {
        let _lock = crate::test_env_lock();
        std::env::set_var("CLAUDETTE_FACELESS", "1");
        let p = cto_decomposition_system_prompt(None);
        std::env::remove_var("CLAUDETTE_FACELESS");
        assert!(!p[0].contains("Voice:"));
        assert!(!p[0].contains("Backstory:"));
    }

    // ─── Gate-review prompt ────────────────────────────────────────────

    #[test]
    fn gate_review_prompt_declares_three_input_blocks() {
        let p = cto_gate_review_system_prompt("/tmp/m/x");
        assert_eq!(p.len(), 1);
        let body = &p[0];
        assert!(body.contains("ORIGINAL REQUEST"));
        assert!(body.contains("FINAL DIFF"));
        assert!(body.contains("VERIFIER VERDICT"));
    }

    #[test]
    fn gate_review_prompt_demands_json_shape() {
        let p = cto_gate_review_system_prompt("/m");
        let body = &p[0];
        assert!(body.contains("approved"));
        assert!(body.contains("score"));
        assert!(body.contains("summary"));
        assert!(body.contains("findings"));
    }

    #[test]
    fn gate_review_prompt_states_critical_block_rule() {
        let p = cto_gate_review_system_prompt("/m");
        // Should articulate the persona's "block on any unhandled critical".
        assert!(p[0].contains("critical"));
        assert!(p[0].contains("Block") || p[0].contains("block"));
    }

    #[test]
    fn gate_review_prompt_threads_mission_path() {
        let p = cto_gate_review_system_prompt("/some/tree");
        assert!(p[0].contains("/some/tree"));
    }

    // ─── Persona bundle ────────────────────────────────────────────────

    #[test]
    fn cto_persona_bundle_parses() {
        let p = default_cto_persona().expect("bundled cto must parse");
        assert_eq!(p.name, "CTO");
        assert_eq!(p.role, crate::forge::types::Role::Cto);
        assert!(!p.voice.is_empty());
        assert!(!p.backstory.is_empty());
    }
}
