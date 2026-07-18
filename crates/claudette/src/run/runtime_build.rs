//! Runtime assembly (Wave C4 — split out of run.rs).
//!
//! Builds `ConversationRuntime`s for the interactive/single-shot agent and for
//! the forge roles, plus the permission policy that gates every tool. Pure
//! construction — no turn loop. `use super::*` pulls in run.rs's own items
//! (current_model, prompt/persona helpers, the forge mission helpers these call
//! into); the explicit `use`s below are the external crate paths run.rs imports.

use super::*;

use std::sync::{Arc, Mutex};

use crate::{ConversationRuntime, PermissionMode, PermissionPolicy, Session};

use crate::api::{stdout_text_callback, telegram_text_callback, OllamaApiClient};
use crate::executor::AgentToolExecutor;
use crate::forge;
use crate::memory::try_load_memory;
use crate::model_config;
use crate::prompt::{agent_system_prompt_with_memory, faceless_mode_enabled, forge_system_prompt};
use crate::theme;
use crate::tool_groups::{ToolGroup, ToolRegistry};

/// Assemble a `ConversationRuntime` with the secretary's model, tools,
/// executor, prompt, and a permissive policy, around the given session
/// (fresh or restored). Loads `~/.claudette/CLAUDETTE.MD` (if present)
/// and appends it to the system prompt as background memory.
///
/// **No streaming callback installed** — use this from single-shot mode and
/// tests, where the assistant's text is printed via `summary.assistant_messages`
/// after the turn completes. The REPL should call [`build_runtime_streaming`]
/// instead.
///
/// `pub(crate)` so the slash-command dispatcher can rebuild the runtime
/// in-place when the user runs `/reload` (which re-reads the memory file
/// without dropping the conversation history).
pub(crate) fn build_runtime(
    session: Session,
) -> ConversationRuntime<OllamaApiClient, AgentToolExecutor> {
    build_runtime_inner(session, false, false)
}

/// Same as [`build_runtime`] but installs the stdout streaming callback so
/// text deltas appear in the terminal as they arrive. Used by the REPL and
/// by every slash command that rebuilds the runtime in place
/// (`/clear`, `/load`, `/reload`, `/compact`).
pub(crate) fn build_runtime_streaming(
    session: Session,
    telegram: bool,
) -> ConversationRuntime<OllamaApiClient, AgentToolExecutor> {
    build_runtime_inner(session, true, telegram)
}

fn build_runtime_inner(
    session: Session,
    streaming: bool,
    telegram: bool,
) -> ConversationRuntime<OllamaApiClient, AgentToolExecutor> {
    // Sprint 14: pull brain model + limits from the process-global
    // `model_config::active()` snapshot. Slash commands (`/preset`,
    // `/brain`) mutate the active config; the next `build_runtime_*`
    // call (e.g. after `/clear`, `/reload`, or after a fallback turn)
    // picks up the new values.
    let brain = model_config::active().brain;
    build_runtime_with_brain(session, &brain, streaming, telegram)
}

/// Sprint 14: explicit-brain variant of [`build_runtime_streaming`].
/// Used by `brain_selector` to spin up a fallback runtime against a
/// different model (e.g. qwen3.5:9b) while reusing the same session +
/// permission policy + system prompt. `pub(crate)` so it stays internal.
pub(crate) fn build_runtime_with_brain(
    session: Session,
    brain: &crate::model_config::RoleConfig,
    streaming: bool,
    telegram: bool,
) -> ConversationRuntime<OllamaApiClient, AgentToolExecutor> {
    build_runtime_with_brain_inner(session, brain, streaming, telegram, None)
}

/// True when claudette is pointed at a code workspace — i.e. `CLAUDETTE_WORKSPACE`
/// is set to a non-empty value. Gates the pre-enabled coding core in
/// [`build_runtime_with_brain_inner`]. A bare/whitespace value counts as unset.
fn coding_workspace_active() -> bool {
    std::env::var("CLAUDETTE_WORKSPACE").is_ok_and(|s| !s.trim().is_empty())
}

fn build_runtime_with_brain_inner(
    session: Session,
    brain: &crate::model_config::RoleConfig,
    streaming: bool,
    telegram: bool,
    system_override: Option<Vec<String>>,
) -> ConversationRuntime<OllamaApiClient, AgentToolExecutor> {
    // One shared ToolRegistry is the single source of truth for the
    // `tools` field on every request. The API client reads from it (via
    // ToolsProvider::Dynamic) and the executor mutates it when the model
    // calls `enable_tools`. Both halves hold a clone of the Arc so the
    // mutations are immediately visible on the next chat turn.
    //
    // Tool-schema policy is workspace-gated:
    //
    //   • Secretary mode (no CLAUDETTE_WORKSPACE) — minimal core (~210 tok).
    //     Pre-rewrite, Telegram auto-enabled five groups; the cost (~2,500
    //     tokens on every turn, ~15% of a 16K window) dominated one-word
    //     interactions like "hey". So a bare secretary stays lazy and reaches
    //     tools via enable_tools — which is now *forgiving*: a no-group call
    //     enables the coding core instead of erroring (see executor.rs).
    //
    //   • Coding mode (CLAUDETTE_WORKSPACE set) — pre-enable the lean coding
    //     core (files/search/advanced/quality, ~2.2k tok). When the user
    //     points claudette at a repo they intend to read/edit/run code, so
    //     the brain should not have to first win the enable_tools(group)
    //     round-trip — which small local models frequently malform (dropping
    //     the group arg) and then spiral on until timeout. The integration
    //     long-tail (github/gmail/calendar/…) stays lazy and is reached via
    //     enable_tools on demand.
    let mut reg = ToolRegistry::new();
    if coding_workspace_active() {
        reg.enable_coding_core();
    }
    let registry = Arc::new(Mutex::new(reg));

    let mut api_client = OllamaApiClient::with_registry(brain.model.clone(), registry.clone())
        .with_context(brain.num_ctx)
        .with_max_predict(brain.num_predict);
    if streaming {
        let cb = if telegram {
            telegram_text_callback()
        } else {
            stdout_text_callback()
        };
        api_client = api_client.with_text_callback(cb);
    }
    // Clone the registry handle for the unknown-tool hinter before the
    // executor consumes it. The hinter maps a confabulated *group* name
    // (e.g. `facts`, `git`) to that group's actual tools so the brain
    // gets a useful "did you mean?" list instead of an empty array.
    let hinter_registry = Arc::clone(&registry);
    let executor = AgentToolExecutor::with_registry(registry);
    // Daily-driver accept-edits: when CLAUDETTE_AUTO_APPROVE is set, the
    // interactive secretary auto-allows every tool (no [y/N]); otherwise the
    // normal WorkspaceWrite + prompt policy applies. Single chokepoint for
    // REPL, one-shot, and TUI (all build their runtime here).
    let policy = if agent_auto_approve_enabled() {
        build_permission_policy().with_active_mode(crate::PermissionMode::Allow)
    } else {
        build_permission_policy()
    };
    let memory = try_load_memory();

    let system_prompt = system_override
        .unwrap_or_else(|| agent_system_prompt_with_memory(memory.as_deref(), telegram));

    ConversationRuntime::new(session, api_client, executor, policy, system_prompt)
        // Tools in optional groups need 3+ iterations (enable_tools → tool call
        // → respond). With the empty-response retry nudge, 8 was too tight for
        // single-shot search/grep/git chains. The shared default (currently 40)
        // and the `CLAUDETTE_MAX_ITERATIONS` env-var knob live in `max_iterations`.
        .with_max_iterations(max_iterations())
        // Top-level interactive turns land iteration-cap hits gracefully
        // (budget nudge + final state-of-work summary) instead of throwing
        // the whole turn away. Sub-agents and forge roles keep the hard
        // error — their callers rely on it to fail a round.
        .with_graceful_iteration_cap()
        .with_auto_compaction_input_tokens_threshold(u32::MAX)
        .with_unknown_tool_hinter(move |name: &str| {
            ToolGroup::parse(name).map_or_else(Vec::new, |group| {
                // Poisoned-lock recovery: another thread held the lock and
                // panicked. Continue with the inner state — the hinter is a
                // best-effort suggestion, not a correctness path.
                let reg = match hinter_registry.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                reg.group_tool_names(group)
            })
        })
}

/// Forge-mode runtime: same plumbing as [`build_runtime_with_brain`] but with
/// a forge-specific system prompt and the tool groups the brain needs
/// pre-enabled (files, search, git, advanced, github) so it doesn't burn
/// turns on `enable_tools`.
///
/// The mission path is threaded into the system prompt so the model has
/// accurate cwd context; the `tools::active_cwd()` routing primitive ensures
/// tools land in the mission tree regardless.
pub(crate) fn build_forge_runtime(
    session: Session,
    mission: &crate::missions::Mission,
    should_submit: bool,
) -> ConversationRuntime<OllamaApiClient, AgentToolExecutor> {
    // v0b: persona overlay. Auto-load the bundled `codex7` coder persona for
    // forge mode. The persona's voice + backstory get woven into the system
    // prompt via `forge_system_prompt`. Lookup failures fall back to an
    // unpersonified prompt — persona overlay is best-effort, never required.
    //
    // `--faceless` / `CLAUDETTE_FACELESS=1` skips the overlay so CI / API
    // integrations can opt out (added 2026-05-19, Phase 2 of import sweep).
    let persona = if faceless_mode_enabled() {
        None
    } else {
        forge_default_coder_persona()
    };
    let memory = try_load_memory();
    let persona_overlay = persona
        .as_ref()
        .map(|p| (p.voice.as_str(), p.backstory.as_str()));

    let system = forge_system_prompt(
        &mission.path.to_string_lossy(),
        memory.as_deref(),
        persona_overlay,
        should_submit,
    );

    // Coder rounds get the full forge toolset. The Submitter phase (v0c)
    // calls back in with `should_submit=true` and uses the same toolset —
    // restricting it to just github tools is tempting but the brain may
    // still need to look at files (e.g. to compose a PR title from the diff).
    build_forge_role_runtime(
        session,
        mission,
        forge::types::Role::Coder,
        system,
        &[
            ToolGroup::Files,
            ToolGroup::Search,
            ToolGroup::Git,
            ToolGroup::Advanced,
            ToolGroup::Github,
        ],
    )
}

/// v0c: phase-aware forge runtime builder. Used by the Coder runtime (full
/// toolset, `Role::Coder` model from `models.toml`) and by the Planner /
/// Verifier turns (no tool groups, different role-routing). Centralises the
/// `OllamaApiClient` + `AgentToolExecutor` + permission policy + hinter
/// setup that every forge phase needs.
pub(crate) fn build_forge_role_runtime(
    session: Session,
    _mission: &crate::missions::Mission,
    role: forge::types::Role,
    system_prompt: Vec<String>,
    tool_groups: &[ToolGroup],
) -> ConversationRuntime<OllamaApiClient, AgentToolExecutor> {
    let mut brain = model_config::active().brain;

    // v0b/v0c: models.toml role-routing. If the user has the requested role
    // configured in `~/.claudettes-forge/models.toml` (or env-overridden),
    // use it for this phase. num_ctx/num_predict aren't in models.toml so
    // they carry over from claudette's config.
    if let Some(role_model) = forge_role_model(role) {
        brain.model = role_model;
    }

    let mut reg = ToolRegistry::new();
    for group in tool_groups {
        reg.enable(*group);
    }
    let registry = Arc::new(Mutex::new(reg));

    let api_client = OllamaApiClient::with_registry(brain.model.clone(), registry.clone())
        .with_context(brain.num_ctx)
        .with_max_predict(brain.num_predict)
        .with_text_callback(stdout_text_callback());

    let hinter_registry = Arc::clone(&registry);
    let executor = AgentToolExecutor::with_registry(registry);
    // Forge phases auto-approve every tool call when CLAUDETTE_FORGE_AUTO_APPROVE
    // is set (unattended/scripted runs). PermissionMode::Allow short-circuits
    // authorize() so the CliPrompter is never consulted. Forge-only: secretary
    // and TUI go through build_permission_policy() directly, unchanged.
    //
    // ROLE ISOLATION (roast RC-B): the dispatch path authorizes by tool name
    // and never consults the registry's enabled-group set, so advertising a
    // restricted toolset to a role does NOT stop a confabulating model from
    // emitting a tool the role was never granted. Cap each role at a hard
    // tier ceiling so `authorize()` denies any over-tier tool *before* the
    // prompter — and before Allow-mode auto-approval — regardless of which
    // tool name the model invents:
    //   • Planner  — read-only investigation, must never mutate the tree.
    //   • Verifier — toolless grader; ReadOnly denies every write/exec tool.
    //   • Coder/Submitter — legitimately need bash/edit_file/apply_diff/git
    //     (all DangerFullAccess), so they keep the default cap.
    let max_tier = match role {
        forge::types::Role::Planner | forge::types::Role::Verifier => {
            crate::PermissionMode::ReadOnly
        }
        _ => crate::PermissionMode::DangerFullAccess,
    };
    let base_policy = build_permission_policy().with_max_tier(max_tier);
    let policy = if forge_auto_approve_enabled() {
        base_policy.with_active_mode(crate::PermissionMode::Allow)
    } else {
        base_policy
    };

    ConversationRuntime::new(session, api_client, executor, policy, system_prompt)
        .with_max_iterations(max_iterations())
        .with_auto_compaction_input_tokens_threshold(u32::MAX)
        .with_unknown_tool_hinter(move |name: &str| {
            ToolGroup::parse(name).map_or_else(Vec::new, |group| {
                let reg = match hinter_registry.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                reg.group_tool_names(group)
            })
        })
}

/// Build a fresh runtime for one deep-research batch: the active brain with
/// read-only tools (Files/Search/Semantic) and a HARD `ReadOnly` permission
/// tier. The tier cap denies every write/exec/network tool at dispatch —
/// before any prompter — regardless of which tool name the model invents
/// (role-isolation lesson, roast RC-B). No prompter is wired (all permitted
/// tools are ReadOnly-tier, so nothing needs approval); `CLAUDETTE_FORGE_
/// AUTO_APPROVE` deliberately has NO effect here (research never builds an
/// Allow-mode policy).
pub(crate) fn build_research_runtime(
    session: Session,
    system_prompt: Vec<String>,
) -> ConversationRuntime<OllamaApiClient, AgentToolExecutor> {
    let brain = model_config::active().brain;

    let mut reg = ToolRegistry::new();
    for group in [ToolGroup::Files, ToolGroup::Search, ToolGroup::Semantic] {
        reg.enable(group);
    }
    let registry = Arc::new(Mutex::new(reg));

    let api_client = OllamaApiClient::with_registry(brain.model.clone(), registry.clone())
        .with_context(brain.num_ctx)
        .with_max_predict(brain.num_predict)
        .with_text_callback(stdout_text_callback());

    let hinter_registry = Arc::clone(&registry);
    let executor = AgentToolExecutor::with_registry(registry);
    let policy = research_permission_policy();

    ConversationRuntime::new(session, api_client, executor, policy, system_prompt)
        .with_max_iterations(max_iterations())
        .with_auto_compaction_input_tokens_threshold(u32::MAX)
        .with_unknown_tool_hinter(move |name: &str| {
            ToolGroup::parse(name).map_or_else(Vec::new, |group| {
                let reg = match hinter_registry.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                reg.group_tool_names(group)
            })
        })
}

/// The permission policy for a deep-research runtime: the ambient policy,
/// hard-capped at `ReadOnly`. Factored out so the cap is directly testable.
pub(crate) fn research_permission_policy() -> PermissionPolicy {
    build_permission_policy().with_max_tier(crate::PermissionMode::ReadOnly)
}

/// v0b helper: resolve any forge role's model from `~/.claudettes-forge/
/// models.toml` (or env overrides). Returns `None` on any failure — the
/// caller falls back to claudette's active brain model. Best-effort; a
/// missing/malformed config never blocks forge mode from running.
///
/// v0b only consumed this for `Role::Coder`; v0c extends it to the Planner
/// and Verifier role-routed turns.
/// True iff the user has *explicitly* configured forge role-routing — either
/// `~/.claudettes-forge/models.toml` exists or a `CLAUDETTES_FORGE_*` env var
/// is set. When neither holds, [`forge_role_model`] returns `None` so every
/// role uses claudette's active brain (roast RC-G #4 / theater "falls back to
/// the active brain"): previously the built-in defaults (`qwen3.5:14b` etc.)
/// always populated the map and silently shadowed the user's active brain —
/// so running `claudette --forge` on a frontier brain still got qwen for the
/// Planner/Verifier.
fn forge_models_explicitly_configured() -> bool {
    if forge::models_toml::default_toml_path().exists() {
        return true;
    }
    std::env::vars().any(|(k, v)| {
        k.starts_with("CLAUDETTES_FORGE_")
            && (k.ends_with("_MODEL") || k.ends_with("_PROVIDER"))
            && !v.trim().is_empty()
    })
}

fn forge_role_model(role: forge::types::Role) -> Option<String> {
    if !forge_models_explicitly_configured() {
        return None;
    }
    let map = forge::types::ModelMap::load().ok()?;
    let (provider, name) = map.resolve(role)?;
    // The forge runtime is hardcoded to `OllamaApiClient` (which also serves
    // LM Studio via CLAUDETTE_OPENAI_COMPAT). A non-Ollama provider therefore
    // can't be honored — previously the provider was dropped and the model
    // name was sent to Ollama regardless, so `provider="anthropic"
    // model="claude-opus-4-7"` 404'd against the local server (roast RC-G #2).
    // Refuse loudly and fall back to the active brain rather than mis-route.
    if provider != forge::types::ProviderKind::Ollama {
        eprintln!(
            "  {} {}",
            theme::dim("∘"),
            theme::warn(&format!(
                "forge: role {role:?} is configured for provider {provider:?} (model {name:?}), \
                 but forge only supports the Ollama/OpenAI-compat backend — ignoring this \
                 override and using the active brain. Set an Ollama model for this role, or run \
                 the whole pipeline on a frontier model via claudette's active brain config."
            )),
        );
        return None;
    }
    Some(name.to_string())
}

/// v0b helper: load the bundled `codex7` Coder persona, parsed at runtime
/// from content baked in via `include_str!`. Returns `None` if the bundled
/// content fails to parse, which should only happen if the personas file is
/// edited into invalid TOML/markdown — caught by
/// `forge::personas::bundled_personas_all_parse`.
///
/// Bundled rather than disk-resolved because claudette is shipped as a
/// single binary (no `cargo install`-side `personas/` directory).
fn forge_default_coder_persona() -> Option<forge::personas::Persona> {
    const CODEX7: &str = include_str!("../../personas/codex7.md");
    forge::personas::parse_persona_content(CODEX7, "bundled:codex7").ok()
}

// ────────────────────────────────────────────────────────────────────────────
// Permission system
// ────────────────────────────────────────────────────────────────────────────

/// Build the per-tool permission policy. Active mode is `WorkspaceWrite`:
/// read-only and workspace-write tools pass through silently, but tools
/// tagged `DangerFullAccess` trigger the CLI prompter for `[y/N]`
/// confirmation before executing.
pub(crate) fn build_permission_policy() -> PermissionPolicy {
    use PermissionMode::{DangerFullAccess, ReadOnly, WorkspaceWrite};

    PermissionPolicy::new(WorkspaceWrite)
        // ── Read-only (auto-allowed) ────────────────────────────────
        .with_tool_requirement("get_current_time", ReadOnly)
        .with_tool_requirement("note_list", ReadOnly)
        .with_tool_requirement("note_read", ReadOnly)
        .with_tool_requirement("todo_list", ReadOnly)
        // enable_tools: meta-tool, pure in-memory state change, no IO
        .with_tool_requirement("enable_tools", ReadOnly)
        .with_tool_requirement("read_file", ReadOnly)
        .with_tool_requirement("list_dir", ReadOnly)
        .with_tool_requirement("get_capabilities", ReadOnly)
        // load_workspace_rules: reads ~/.claudette/instructions.md on demand
        // (added in the 2026-05-04 token-trim work to lazy-load what used to
        // auto-attach to the system prompt). Read-only.
        .with_tool_requirement("load_workspace_rules", ReadOnly)
        .with_tool_requirement("glob_search", ReadOnly)
        .with_tool_requirement("grep_search", ReadOnly)
        .with_tool_requirement("git_status", ReadOnly)
        .with_tool_requirement("git_diff", ReadOnly)
        .with_tool_requirement("git_log", ReadOnly)
        .with_tool_requirement("git_branch", ReadOnly)
        // ── Workspace-write (auto-allowed) ──────────────────────────
        .with_tool_requirement("note_create", WorkspaceWrite)
        .with_tool_requirement("note_delete", WorkspaceWrite)
        .with_tool_requirement("todo_add", WorkspaceWrite)
        // v0.6.0: todo_complete + todo_uncomplete merged into
        // todo_set_status(done?).
        .with_tool_requirement("todo_set_status", WorkspaceWrite)
        .with_tool_requirement("todo_delete", WorkspaceWrite)
        .with_tool_requirement("write_file", WorkspaceWrite)
        .with_tool_requirement("web_search", WorkspaceWrite)
        // web_fetch is network EGRESS to a model-supplied URL — the exfil sink
        // in the prompt-injection chain (roast 2026-06-02 H2). Gated at
        // DangerFullAccess so it prompts by default; CLAUDETTE_AUTO_APPROVE /
        // forge Allow-mode still pass it through. See the Dangerous block below.
        .with_tool_requirement("open_in_editor", WorkspaceWrite)
        .with_tool_requirement("reveal_in_explorer", WorkspaceWrite)
        .with_tool_requirement("open_url", WorkspaceWrite)
        .with_tool_requirement("add_numbers", WorkspaceWrite)
        // ── Sprint 9 Phase 0a: facts group (read-only REST calls) ───
        // v0.6.0: wikipedia_search + wikipedia_summary merged into
        // wikipedia(mode?); weather_current + weather_forecast merged
        // into weather(days?).
        .with_tool_requirement("wikipedia", ReadOnly)
        .with_tool_requirement("weather", ReadOnly)
        // ── Sprint 9 Phase 0a: registry group (read-only) ────────────
        // crate_search + npm_search were dropped in v0.6.0 — web_search
        // covers the same need with better recall and an already-loaded
        // schema.
        .with_tool_requirement("crate_info", ReadOnly)
        .with_tool_requirement("npm_info", ReadOnly)
        // ── v0.6.0: quality group (project-tests, project-diagnostics) ──
        // Both spawn the project's toolchain as a subprocess, which runs
        // user-provided build/test code — gate at WorkspaceWrite so the
        // user sees the dispatch the first time the brain reaches for
        // each tool. Subsequent calls within the same session are
        // auto-allowed by the policy cache.
        .with_tool_requirement("run_tests", WorkspaceWrite)
        .with_tool_requirement("diagnostics", WorkspaceWrite)
        // apply_patch mutates files under $HOME — same DangerFullAccess
        // gate as edit_file (its long-term replacement). dry_run does no
        // disk writes but the schema doesn't differentiate, so the
        // permission applies uniformly.
        .with_tool_requirement("apply_patch", DangerFullAccess)
        // apply_diff edits arbitrary in-sandbox files (fuzzy before/after
        // replacement) — same disk-write gate as apply_patch/edit_file.
        .with_tool_requirement("apply_diff", DangerFullAccess)
        // ── v0.6.0: bash_background family ──────────────────────────
        // bash_background spawns a long-running subprocess — same gate
        // as `bash`. bash_status + bash_tail are pure reads of files
        // we wrote, so they're ReadOnly.
        .with_tool_requirement("bash_background", DangerFullAccess)
        .with_tool_requirement("bash_status", ReadOnly)
        .with_tool_requirement("bash_tail", ReadOnly)
        // ── v0.6.0 Phase 3.4a: ask_user clarifier ───────────────────
        // ReadOnly — it only reads from stdin; no side effects.
        .with_tool_requirement("ask_user", ReadOnly)
        // ── v0.6.0: semantic search ─────────────────────────────────
        // semantic_grep reads workspace files (capped) and ranks by
        // token-overlap. Pure read — ReadOnly tier is fine.
        .with_tool_requirement("semantic_grep", ReadOnly)
        // repo_map reads workspace source files (capped, gitignore-aware) and
        // returns a ranked symbol outline. Pure read — ReadOnly.
        .with_tool_requirement("repo_map", ReadOnly)
        // ── v0.6.0 Phase 3.4b: clipboard text I/O ───────────────────
        // Both can leak sensitive content (passwords on the clipboard,
        // arbitrary text written into a user-visible buffer) — gate at
        // WorkspaceWrite so the first call shows up in the prompt.
        .with_tool_requirement("clipboard_read", WorkspaceWrite)
        .with_tool_requirement("clipboard_write", WorkspaceWrite)
        // ── v0.6.0: vision tools ────────────────────────────────────
        // screenshot_capture invokes a platform screenshot tool (PowerShell
        // bitmap on Windows, screencapture on macOS, gnome-screenshot/
        // import on Linux). Treated as WorkspaceWrite because it writes
        // a PNG under ~/.claudette/files/. image_describe is a network
        // POST to LM Studio plus a file read — WorkspaceWrite tier.
        .with_tool_requirement("screenshot_capture", WorkspaceWrite)
        .with_tool_requirement("image_describe", WorkspaceWrite)
        // ── Sprint 9 Phase 0a: github group ──────────────────────────
        // Reads: auto-allowed. Writes: WorkspaceWrite (hit the network
        // on the user's behalf but don't touch the filesystem).
        // v0.6.0: gh_list_my_prs + gh_list_assigned_issues merged into
        // gh_inbox(scope?).
        .with_tool_requirement("gh_inbox", ReadOnly)
        .with_tool_requirement("gh_get_issue", ReadOnly)
        .with_tool_requirement("gh_search_code", ReadOnly)
        .with_tool_requirement("gh_list_repo_issues", ReadOnly)
        .with_tool_requirement("gh_pr_status", ReadOnly)
        // v0.6.0 Phase 3.3a — single-shot PR snapshot.
        .with_tool_requirement("gh_pr_view", ReadOnly)
        // v0.6.0 Phase 3.3b — failed-job log extraction.
        .with_tool_requirement("gh_workflow_logs", ReadOnly)
        // v0.6.0 Phase 3.4c — forge mission tail. Pure file read.
        .with_tool_requirement("forge_tail", ReadOnly)
        .with_tool_requirement("gh_create_issue", WorkspaceWrite)
        .with_tool_requirement("gh_comment_issue", WorkspaceWrite)
        .with_tool_requirement("gh_fork", WorkspaceWrite)
        .with_tool_requirement("gh_create_pr", WorkspaceWrite)
        // ── Sprint 10: telegram group ────────────────────────────────
        // tg_send is network EGRESS (posts arbitrary text to an arbitrary
        // chat) — a second exfil sink, so it's gated at DangerFullAccess in
        // the Dangerous block below rather than auto-allowed. v0.6.0 decom:
        // tg_get_updates dropped (prompt-injection footgun); tg_send_photo
        // merged into tg_send via an optional `photo` arg.
        // ── Life Agent (v0.2.0): calendar group ──────────────────────
        // Reads: auto-allowed. Writes/RSVP: WorkspaceWrite. Delete is
        // irreversible from claudette's side, so DangerFullAccess.
        .with_tool_requirement("calendar_list_events", ReadOnly)
        .with_tool_requirement("calendar_create_event", WorkspaceWrite)
        .with_tool_requirement("calendar_update_event", WorkspaceWrite)
        .with_tool_requirement("calendar_delete_event", DangerFullAccess)
        // ── Life Agent: gmail group (gmail.readonly OAuth scope) ─────
        .with_tool_requirement("gmail_list", ReadOnly)
        .with_tool_requirement("gmail_search", ReadOnly)
        .with_tool_requirement("gmail_read", ReadOnly)
        .with_tool_requirement("gmail_list_labels", ReadOnly)
        // ── Life Agent: schedule group ───────────────────────────────
        .with_tool_requirement("schedule_list", ReadOnly)
        .with_tool_requirement("schedule_once", WorkspaceWrite)
        .with_tool_requirement("schedule_recurring", WorkspaceWrite)
        .with_tool_requirement("schedule_cancel", WorkspaceWrite)
        // ── Recall (cross-session memory): pure search ───────────────
        .with_tool_requirement("recall", ReadOnly)
        // ── Dangerous (ALWAYS prompts for [y/N] confirmation) ────��──
        .with_tool_requirement("bash", DangerFullAccess)
        // Network egress to model-supplied destinations — prompt before each
        // call so an injected instruction can't silently exfiltrate (H2).
        .with_tool_requirement("web_fetch", DangerFullAccess)
        .with_tool_requirement("tg_send", DangerFullAccess)
        .with_tool_requirement("edit_file", DangerFullAccess)
        .with_tool_requirement("git_add", DangerFullAccess)
        .with_tool_requirement("git_commit", DangerFullAccess)
        .with_tool_requirement("git_push", DangerFullAccess)
        .with_tool_requirement("git_checkout", DangerFullAccess)
        // Brownfield: git_clone writes a fresh tree under the controlled
        // ~/.claudette/missions/ root. Auto-allowed (WorkspaceWrite).
        .with_tool_requirement("git_clone", WorkspaceWrite)
        // ── T2 brownfield: mission_* tools ──────────────────────────────
        // mission_start clones into ~/.claudette/missions/ (WorkspaceWrite,
        // matching git_clone). mission_state (status/list/attach/exit) only
        // reads or flips in-memory session state with no FS writes, so it
        // sits at the lowest tier (ReadOnly); downstream cwd-routed writes
        // still go through their own gates. mission_submit stages/commits/
        // pushes/opens a PR — DangerFullAccess to match its worst action
        // (`git push -u`).
        .with_tool_requirement("mission_start", WorkspaceWrite)
        .with_tool_requirement("mission_state", ReadOnly)
        .with_tool_requirement("mission_submit", DangerFullAccess)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge;

    /// The bundled `codex7` persona is baked into the binary via
    /// `include_str!`. If the file is edited into invalid TOML or stripped of
    /// its frontmatter, `forge_default_coder_persona` returns `None` and
    /// forge-mode silently runs without a persona. Catch that at build time.
    #[test]
    fn forge_default_coder_persona_parses_bundled_codex7() {
        let p = forge_default_coder_persona().expect("bundled codex7 must parse");
        assert_eq!(p.name, "CodeX-7");
        assert_eq!(p.role, forge::types::Role::Coder);
        assert!(!p.voice.is_empty(), "codex7 should have a voice");
        assert!(!p.backstory.is_empty(), "codex7 should have backstory");
    }

    /// The research permission policy caps at `ReadOnly`: every write/exec
    /// tool is denied at the tier ceiling before any prompter is consulted,
    /// regardless of the ambient mode (role-isolation guard, roast RC-B).
    #[test]
    fn research_policy_is_read_only_capped() {
        let _guard = crate::test_env_lock();
        let policy = research_permission_policy();
        assert_eq!(policy.max_tier(), PermissionMode::ReadOnly);
        for tool in ["write_file", "bash", "apply_diff", "git_commit"] {
            assert!(
                matches!(
                    policy.authorize(tool, "{}", None),
                    crate::PermissionOutcome::Deny { .. }
                ),
                "{tool} must be denied under the ReadOnly cap"
            );
        }
    }
}
