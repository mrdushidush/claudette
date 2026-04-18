//! Slash-command dispatcher for the claudette REPL.
//!
//! Slash commands are how the user controls the REPL itself (sessions,
//! memory, status, exit) without sending text to the model. They are matched
//! in [`run::run_secretary_repl`] BEFORE the line is dispatched to the LLM:
//! any line starting with `/` is parsed via [`parse_slash_command`], and a
//! non-`None` return value is handled inside the REPL loop without ever
//! reaching the model.
//!
//! Adding a new command is a 5-step touch:
//! 1. Add a variant to [`SlashCommand`].
//! 2. Add the keyword (and any aliases) to [`parse_slash_command`].
//! 3. Add a handler arm to [`dispatch_slash_command`].
//! 4. Add the one-liner to [`print_help`] so `/help` lists it.
//! 5. Add a parser test in `mod tests`.
//!
//! All output goes to stderr — never stdout — so piping the assistant's
//! actual replies into a file still works cleanly.

use std::io::{self, Write};

use crate::{compact_session, CompactionConfig, ConversationRuntime, Session};

use crate::api::{current_num_ctx, OllamaApiClient};
use crate::executor::SecretaryToolExecutor;
use crate::memory::{default_memory_path, try_load_memory, MAX_MEMORY_CHARS};
use crate::model_config::{self, ModelConfig, Preset};
use crate::run::{
    build_runtime_streaming, compact_threshold, current_model, default_session_path, sessions_dir,
};
use crate::theme;
use crate::tool_groups::{ToolGroup, ToolRegistry};
use crate::tools::secretary_tools_json;

/// Type alias for the concrete runtime the dispatcher mutates. Saves a lot of
/// horizontal space in the handler signatures.
type SecretaryRuntime = ConversationRuntime<OllamaApiClient, SecretaryToolExecutor>;

// === Public types ============================================================

/// One slash command parsed from a REPL line. Carries any string arg directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Help,
    Clear,
    Compact,
    Sessions,
    Save(String),
    Load(String),
    Status,
    Cost,
    Tools,
    Model,
    Memory,
    Reload,
    Capabilities,
    Exit,
    Validate(String),
    Agents,
    /// Sprint 14: swap the whole preset bundle (brain + fallback) in one
    /// command. `fast` / `auto` / `smart`.
    PresetSwitch(Preset),
    /// Sprint 14: pin the brain model. `auto` re-enables the current
    /// preset's fallback; any other value pins the brain and disables
    /// auto-fallback. Lives for the process (no persistence).
    Brain(String),
    /// Sprint 14: pin the coder model. Out of the fallback scope — just
    /// a convenience wrapper over `CLAUDETTE_CODER_MODEL`.
    Coder(String),
    /// Sprint 14: dump the current model config (brain, fallback, coder,
    /// last fallback timestamp from `fallback.jsonl`).
    Models,
    /// Recognised as starting with `/` but unusable: unknown name, missing
    /// required argument, etc. Carries a human-readable error.
    Invalid(String),
}

/// What the REPL should do after a slash command finishes running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashOutcome {
    /// Command handled, REPL continues to the next line.
    Continue,
    /// User asked to leave the REPL.
    Exit,
}

/// Per-REPL accumulators that the dispatcher reads for `/status` and `/cost`.
/// Lives in the REPL loop, not in the runtime, because the runtime's own
/// `UsageTracker` resets when we rebuild on `/clear` / `/load` / `/reload` —
/// but we want lifetime-of-the-process stats too.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReplState {
    pub cumulative_input_tokens: u64,
    pub cumulative_output_tokens: u64,
    pub turn_count: u32,
}

impl ReplState {
    /// Add one turn's token usage to the running totals.
    pub fn record_turn(&mut self, input: u32, output: u32) {
        self.cumulative_input_tokens = self
            .cumulative_input_tokens
            .saturating_add(u64::from(input));
        self.cumulative_output_tokens = self
            .cumulative_output_tokens
            .saturating_add(u64::from(output));
        self.turn_count = self.turn_count.saturating_add(1);
    }
}

// === Parser ==================================================================

/// Parse a REPL line into a `SlashCommand`. Returns `None` if the line is not
/// a slash command (so the caller can fall through to `runtime.run_turn`).
/// Returns `Some(Invalid(...))` if it IS a slash command but unusable.
#[must_use]
pub fn parse_slash_command(line: &str) -> Option<SlashCommand> {
    let trimmed = line.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let body = &trimmed[1..];
    if body.is_empty() {
        return Some(SlashCommand::Invalid(
            "empty command — try /help".to_string(),
        ));
    }

    let mut parts = body.splitn(2, char::is_whitespace);
    let cmd = parts.next()?.to_lowercase();
    let arg = parts.next().map(|s| s.trim().to_string());

    let parsed = match cmd.as_str() {
        "help" | "h" | "?" => SlashCommand::Help,
        "clear" | "cl" => SlashCommand::Clear,
        "compact" => SlashCommand::Compact,
        "sessions" | "ls" => SlashCommand::Sessions,
        "save" => match arg.filter(|s| !s.is_empty()) {
            Some(name) => SlashCommand::Save(name),
            None => SlashCommand::Invalid("/save requires a name. Try: /save my-work".to_string()),
        },
        "load" => match arg.filter(|s| !s.is_empty()) {
            Some(name) => SlashCommand::Load(name),
            None => SlashCommand::Invalid("/load requires a name. Try: /load my-work".to_string()),
        },
        "status" | "st" => SlashCommand::Status,
        "cost" => SlashCommand::Cost,
        "tools" => SlashCommand::Tools,
        "model" => SlashCommand::Model,
        "memory" | "mem" => SlashCommand::Memory,
        "reload" => SlashCommand::Reload,
        "capabilities" | "cap" => SlashCommand::Capabilities,
        "validate" | "val" => match arg.filter(|s| !s.is_empty()) {
            Some(path) => SlashCommand::Validate(path),
            None => SlashCommand::Invalid(
                "/validate requires a path. Try: /validate ~/.claudette/files/userClass.py"
                    .to_string(),
            ),
        },
        "agents" => SlashCommand::Agents,
        "preset" => match arg.filter(|s| !s.is_empty()) {
            Some(p) => match p.parse::<Preset>() {
                Ok(preset) => SlashCommand::PresetSwitch(preset),
                Err(e) => SlashCommand::Invalid(e),
            },
            None => SlashCommand::Invalid("/preset requires one of: fast, auto, smart".to_string()),
        },
        "brain" => match arg.filter(|s| !s.is_empty()) {
            Some(m) => SlashCommand::Brain(m),
            None => SlashCommand::Invalid("/brain requires a model name or 'auto'".to_string()),
        },
        "coder" => match arg.filter(|s| !s.is_empty()) {
            Some(m) => SlashCommand::Coder(m),
            None => SlashCommand::Invalid("/coder requires a model name".to_string()),
        },
        "models" => SlashCommand::Models,
        "exit" | "quit" | "q" | "x" => SlashCommand::Exit,
        other => SlashCommand::Invalid(format!("unknown command: /{other} — try /help")),
    };
    Some(parsed)
}

// === Dispatcher ==============================================================

/// Run a parsed slash command. Mutates `runtime` for `/clear`, `/compact`,
/// `/load`, `/reload` (which all rebuild the runtime around a different
/// session). Returns whether the REPL should continue or exit.
pub fn dispatch_slash_command(
    cmd: SlashCommand,
    runtime: &mut SecretaryRuntime,
    state: &ReplState,
) -> SlashOutcome {
    let stderr = io::stderr();
    let mut err = stderr.lock();

    let outcome = match cmd {
        SlashCommand::Help => {
            print_help(&mut err);
            SlashOutcome::Continue
        }
        SlashCommand::Clear => {
            *runtime = build_runtime_streaming(Session::default(), false);
            let _ = writeln!(
                err,
                "{} {}",
                theme::ok(theme::OK_GLYPH),
                theme::ok("session cleared (saved files on disk untouched)")
            );
            SlashOutcome::Continue
        }
        SlashCommand::Compact => {
            handle_compact(&mut err, runtime);
            SlashOutcome::Continue
        }
        SlashCommand::Sessions => {
            handle_sessions(&mut err);
            SlashOutcome::Continue
        }
        SlashCommand::Save(name) => {
            handle_save(&mut err, runtime, &name);
            SlashOutcome::Continue
        }
        SlashCommand::Load(name) => {
            handle_load(&mut err, runtime, &name);
            SlashOutcome::Continue
        }
        SlashCommand::Status => {
            handle_status(&mut err, runtime, state);
            SlashOutcome::Continue
        }
        SlashCommand::Cost => {
            handle_cost(&mut err, runtime, state);
            SlashOutcome::Continue
        }
        SlashCommand::Tools => {
            handle_tools(&mut err);
            SlashOutcome::Continue
        }
        SlashCommand::Model => {
            handle_model(&mut err);
            SlashOutcome::Continue
        }
        SlashCommand::Memory => {
            handle_memory(&mut err);
            SlashOutcome::Continue
        }
        SlashCommand::Reload => {
            handle_reload(&mut err, runtime);
            SlashOutcome::Continue
        }
        SlashCommand::Capabilities => {
            handle_capabilities(&mut err);
            SlashOutcome::Continue
        }
        SlashCommand::Validate(path) => {
            handle_validate(&mut err, &path);
            SlashOutcome::Continue
        }
        SlashCommand::Agents => {
            handle_agents(&mut err);
            SlashOutcome::Continue
        }
        SlashCommand::PresetSwitch(preset) => {
            handle_preset(&mut err, runtime, preset);
            SlashOutcome::Continue
        }
        SlashCommand::Brain(model) => {
            handle_brain(&mut err, runtime, &model);
            SlashOutcome::Continue
        }
        SlashCommand::Coder(model) => {
            handle_coder(&mut err, &model);
            SlashOutcome::Continue
        }
        SlashCommand::Models => {
            handle_models(&mut err);
            SlashOutcome::Continue
        }
        SlashCommand::Exit => SlashOutcome::Exit,
        SlashCommand::Invalid(msg) => {
            let _ = writeln!(
                err,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&msg)
            );
            SlashOutcome::Continue
        }
    };

    let _ = err.flush();
    outcome
}

// === Handlers ================================================================

fn print_help(out: &mut impl Write) {
    let lines: &[(&str, &str)] = &[
        ("/help (h, ?)", "Show this list"),
        (
            "/clear (cl)",
            "Wipe in-memory session — saved files untouched",
        ),
        ("/compact", "Force a context compaction now"),
        (
            "/sessions (ls)",
            "List saved sessions in ~/.claudette/sessions/",
        ),
        ("/save <name>", "Snapshot the current session under a name"),
        ("/load <name>", "Replace current session with a saved one"),
        ("/status (st)", "Turns, tokens, model, context window"),
        ("/cost", "Cumulative token usage for this REPL"),
        ("/tools", "List the secretary's tools"),
        ("/model", "Show the active Ollama model"),
        ("/memory (mem)", "Show CLAUDETTE.MD memory in use"),
        ("/reload", "Re-read CLAUDETTE.MD without losing history"),
        ("/capabilities (cap)", "Full configuration dump"),
        (
            "/validate (val) <path>",
            "Run Codet code validator on a file",
        ),
        ("/agents", "List available agent types"),
        (
            "/preset <fast|auto|smart>",
            "Switch brain preset (swap 4b/9b/fallback)",
        ),
        (
            "/brain <model|auto>",
            "Pin brain model (or 'auto' to restore preset fallback)",
        ),
        ("/coder <model>", "Pin coder model"),
        ("/models", "Show current model config"),
        ("/exit (quit, q, x)", "Leave the REPL"),
    ];
    let _ = writeln!(
        out,
        "{} {}",
        theme::SPARKLES,
        theme::accent("claudette slash commands")
    );
    for (cmd, desc) in lines {
        let _ = writeln!(out, "  {}  {}", theme::accent(cmd), theme::dim(desc));
    }
}

fn handle_compact(out: &mut impl Write, runtime: &mut SecretaryRuntime) {
    // Force compaction by setting `max_estimated_tokens = 0` so the
    // should-compact gate is satisfied as long as the session has more than
    // `preserve_recent_messages` (=4) entries.
    let result = compact_session(
        runtime.session(),
        CompactionConfig {
            preserve_recent_messages: 4,
            max_estimated_tokens: 0,
        },
    );
    if result.removed_message_count == 0 {
        let _ = writeln!(
            out,
            "{} {}",
            theme::dim("○"),
            theme::dim("nothing to compact (session has 4 or fewer messages)")
        );
        return;
    }
    let removed = result.removed_message_count;
    *runtime = build_runtime_streaming(result.compacted_session, false);
    let _ = writeln!(
        out,
        "{} {}",
        theme::SAVE,
        theme::ok(&format!("compacted {removed} message(s) into a summary"))
    );
}

fn handle_sessions(out: &mut impl Write) {
    let dir = sessions_dir();
    let _ = writeln!(
        out,
        "{} {} {}",
        theme::FILE,
        theme::accent("sessions"),
        theme::dim(&dir.display().to_string())
    );
    let names = list_session_names(&dir);
    if names.is_empty() {
        let _ = writeln!(out, "  {}", theme::dim("(none)"));
        return;
    }
    for name in names {
        let _ = writeln!(out, "  {} {}", theme::ok(theme::OK_GLYPH), name);
    }
}

fn list_session_names(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str())? != "json" {
                return None;
            }
            p.file_stem().and_then(|s| s.to_str()).map(String::from)
        })
        .collect();
    names.sort();
    names
}

fn handle_save(out: &mut impl Write, runtime: &SecretaryRuntime, name: &str) {
    let safe_name = match sanitize_session_name(name) {
        Ok(n) => n,
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&e)
            );
            return;
        }
    };
    let path = sessions_dir().join(format!("{safe_name}.json"));
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("create_dir_all failed: {e}"))
            );
            return;
        }
    }
    match runtime.session().save_to_path(&path) {
        Ok(()) => {
            let _ = writeln!(
                out,
                "{} {} {}",
                theme::SAVE,
                theme::ok("session saved →"),
                theme::dim(&path.display().to_string())
            );
        }
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("save failed: {e}"))
            );
        }
    }
}

fn handle_load(out: &mut impl Write, runtime: &mut SecretaryRuntime, name: &str) {
    let safe_name = match sanitize_session_name(name) {
        Ok(n) => n,
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&e)
            );
            return;
        }
    };
    let path = sessions_dir().join(format!("{safe_name}.json"));
    if !path.exists() {
        let _ = writeln!(
            out,
            "{} {}",
            theme::error(theme::ERR_GLYPH),
            theme::error(&format!("no session at {}", path.display()))
        );
        return;
    }
    match Session::load_from_path(&path) {
        Ok(session) => {
            let count = session.messages.len();
            *runtime = build_runtime_streaming(session, false);
            let _ = writeln!(
                out,
                "{} {} {}",
                theme::ok(theme::OK_GLYPH),
                theme::ok(&format!("loaded {safe_name}")),
                theme::dim(&format!("({count} messages)"))
            );
        }
        Err(e) => {
            let _ = writeln!(
                out,
                "{} {}",
                theme::error(theme::ERR_GLYPH),
                theme::error(&format!("load failed: {e}"))
            );
        }
    }
}

fn handle_status(out: &mut impl Write, runtime: &SecretaryRuntime, state: &ReplState) {
    let session = runtime.session();
    let memory_marker = if try_load_memory().is_some() {
        theme::ok("loaded")
    } else {
        theme::dim("none")
    };

    let _ = writeln!(out, "{} {}", theme::GEAR, theme::accent("status"));
    let _ = writeln!(
        out,
        "  {} model: {}",
        theme::dim("•"),
        theme::ok(&current_model())
    );
    let _ = writeln!(
        out,
        "  {} session messages: {}",
        theme::dim("•"),
        session.messages.len()
    );
    let _ = writeln!(
        out,
        "  {} REPL turns: {}",
        theme::dim("•"),
        state.turn_count
    );
    let _ = writeln!(
        out,
        "  {} cumulative tokens: in={} out={}",
        theme::dim("•"),
        state.cumulative_input_tokens,
        state.cumulative_output_tokens
    );
    let _ = writeln!(out, "  {} num_ctx: {}", theme::dim("•"), current_num_ctx());
    let _ = writeln!(
        out,
        "  {} compaction threshold: {} cumulative input tokens",
        theme::dim("•"),
        compact_threshold()
    );
    let _ = writeln!(out, "  {} memory: {}", theme::dim("•"), memory_marker);
    let _ = writeln!(
        out,
        "  {} session file: {}",
        theme::dim("•"),
        theme::dim(&default_session_path().display().to_string())
    );
}

fn handle_cost(out: &mut impl Write, runtime: &SecretaryRuntime, state: &ReplState) {
    let usage = runtime.usage().cumulative_usage();
    let avg_in = if state.turn_count > 0 {
        state.cumulative_input_tokens / u64::from(state.turn_count)
    } else {
        0
    };
    let avg_out = if state.turn_count > 0 {
        state.cumulative_output_tokens / u64::from(state.turn_count)
    } else {
        0
    };

    let _ = writeln!(out, "{} {}", theme::BOLT, theme::accent("token usage"));
    let _ = writeln!(
        out,
        "  {} REPL turns: {}",
        theme::dim("•"),
        state.turn_count
    );
    let _ = writeln!(
        out,
        "  {} REPL cumulative — in: {}  out: {}",
        theme::dim("•"),
        state.cumulative_input_tokens,
        state.cumulative_output_tokens
    );
    let _ = writeln!(
        out,
        "  {} REPL average / turn — in: {}  out: {}",
        theme::dim("•"),
        avg_in,
        avg_out
    );
    let _ = writeln!(
        out,
        "  {} runtime cumulative — in: {}  out: {}",
        theme::dim("•"),
        usage.input_tokens,
        usage.output_tokens
    );
    let _ = writeln!(
        out,
        "  {} {}",
        theme::dim("•"),
        theme::dim("(Ollama is free; numbers are for tuning, not billing)")
    );
}

fn handle_tools(out: &mut impl Write) {
    // Sprint 8: show tools grouped into core + optional groups. We can't
    // reach into the live runtime's registry from a slash command (the
    // borrow checker would fight us), so we build a fresh ToolRegistry for
    // display purposes. It uses the same construction logic so the groups
    // it shows are exactly what the live registry has.
    let registry = ToolRegistry::new();

    let _ = writeln!(
        out,
        "{} {} {}",
        theme::SPARKLES,
        theme::accent("secretary tools"),
        theme::dim(&format!(
            "(core {} + {} optional groups)",
            registry.core_tool_names().len(),
            ToolGroup::all().len()
        ))
    );

    let _ = writeln!(out, "  {} core (always loaded)", theme::BOLT);
    describe_tool_group(out, &registry.core_tool_names());

    for group in ToolGroup::all() {
        let names = registry.group_tool_names(group);
        let _ = writeln!(
            out,
            "\n  {} {} {}",
            theme::BOLT,
            theme::accent(group.name()),
            theme::dim(&format!(
                "— {} tool(s), enable with enable_tools({{group: {:?}}})",
                names.len(),
                group.name()
            ))
        );
        describe_tool_group(out, &names);
    }

    let _ = writeln!(
        out,
        "\n  {}",
        theme::dim(&format!(
            "core schema: {} chars — enabling a group grows this temporarily",
            registry.current_schema_chars(),
        ))
    );
}

/// Print a short "• name: description" line for each tool in `names`,
/// looking up the description in the full `secretary_tools_json` registry.
fn describe_tool_group(out: &mut impl Write, names: &[String]) {
    let full = secretary_tools_json();
    let arr: &[serde_json::Value] = full.as_array().map_or(&[], Vec::as_slice);
    for name in names {
        // enable_tools is synthesized by ToolRegistry and doesn't live in
        // secretary_tools_json — give it a hard-coded description.
        let desc_owned;
        let desc: &str = if name == "enable_tools" {
            "Load an optional tool group (git, ide, search, advanced)."
        } else {
            desc_owned = arr
                .iter()
                .find(|t| t.pointer("/function/name").and_then(|v| v.as_str()) == Some(name))
                .and_then(|t| {
                    t.pointer("/function/description")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .unwrap_or_else(|| "(no description)".to_string());
            &desc_owned
        };
        let short = first_sentence(desc, 80);
        let _ = writeln!(
            out,
            "    {} {}: {}",
            theme::ok(theme::OK_GLYPH),
            theme::accent(name),
            theme::dim(&short)
        );
    }
}

fn handle_model(out: &mut impl Write) {
    let _ = writeln!(out, "{} {}", theme::ROBOT, theme::accent("model"));
    let _ = writeln!(
        out,
        "  {} active: {}",
        theme::dim("•"),
        theme::ok(&current_model())
    );
    let _ = writeln!(
        out,
        "  {} {}",
        theme::dim("•"),
        theme::dim("override with: CLAUDETTE_MODEL=<name>")
    );
}

fn handle_memory(out: &mut impl Write) {
    let path = default_memory_path();
    let _ = writeln!(
        out,
        "{} {} {}",
        theme::BRAIN,
        theme::accent("memory"),
        theme::dim(&path.display().to_string())
    );
    match try_load_memory() {
        Some(content) => {
            let len = content.chars().count();
            let _ = writeln!(
                out,
                "  {} {} chars (cap {MAX_MEMORY_CHARS})",
                theme::dim("•"),
                len
            );
            let _ = writeln!(out);
            for line in content.lines() {
                let _ = writeln!(out, "    {line}");
            }
        }
        None => {
            let _ = writeln!(
                out,
                "  {}",
                theme::dim("(no memory file — create one to give the secretary background)")
            );
        }
    }
}

fn handle_reload(out: &mut impl Write, runtime: &mut SecretaryRuntime) {
    let session = runtime.session().clone();
    *runtime = build_runtime_streaming(session, false);
    if try_load_memory().is_some() {
        let _ = writeln!(
            out,
            "{} {}",
            theme::BRAIN,
            theme::ok("memory reloaded into the system prompt")
        );
    } else {
        let _ = writeln!(
            out,
            "{} {}",
            theme::BRAIN,
            theme::dim("no memory file found — continuing without")
        );
    }
}

fn handle_agents(out: &mut impl Write) {
    let _ = writeln!(
        out,
        "{} {}",
        theme::ROBOT,
        theme::accent("available agents")
    );
    let agents: &[(&str, &str)] = &[
        (
            "researcher",
            "web search, file reading, code search (max 10 iter)",
        ),
        ("gitops", "git workflows, bash, file reading (max 8 iter)"),
        (
            "reviewer",
            "code review: bugs, security, quality (max 5 iter, read-only)",
        ),
        (
            "codet",
            "code validation sidecar (automatic on write_file, supports py/rs/js/ts)",
        ),
    ];
    for (name, desc) in agents {
        let _ = writeln!(out, "  {}  {}", theme::accent(name), theme::dim(desc));
    }
    let _ = writeln!(
        out,
        "\n  {}",
        theme::dim("Trigger via the spawn_agent tool or ask Claudette to delegate.")
    );
}

fn handle_preset(out: &mut impl Write, runtime: &mut SecretaryRuntime, preset: Preset) {
    // Start from a fresh preset-defaults config, then reapply the TOML +
    // env overlays so we don't silently lose per-role customisations the
    // user had in `~/.claudette/models.toml` or env vars.
    let new_cfg = ModelConfig::resolve(preset);
    model_config::set_active(new_cfg);
    rebuild_after_model_swap(runtime);
    print_models(out, &model_config::active(), Some(preset));
}

fn handle_brain(out: &mut impl Write, runtime: &mut SecretaryRuntime, model: &str) {
    // Special case: `/brain auto` is the inverse of a pin — restores the
    // current preset's fallback policy. Any other value pins the brain
    // and clears the fallback so the next turn doesn't silently swap.
    if model.eq_ignore_ascii_case("auto") {
        let preset = model_config::active().preset;
        let new_cfg = ModelConfig::resolve(preset);
        model_config::set_active(new_cfg);
        rebuild_after_model_swap(runtime);
        let _ = writeln!(
            out,
            "{} {}",
            theme::ROBOT,
            theme::ok(&format!(
                "restored preset {}: {} (fallback: {})",
                model_config::active().preset,
                model_config::active().brain.model,
                model_config::active()
                    .fallback_brain
                    .as_ref()
                    .map_or("none", |f| f.model.as_str()),
            ))
        );
        return;
    }

    let cfg = model_config::update_active(|c| {
        c.brain.model = model.to_string();
        c.fallback_brain = None;
    });
    rebuild_after_model_swap(runtime);
    let _ = writeln!(
        out,
        "{} {}",
        theme::ROBOT,
        theme::ok(&format!(
            "brain pinned → {} (fallback disabled)",
            cfg.brain.model
        ))
    );
}

fn handle_coder(out: &mut impl Write, model: &str) {
    let cfg = model_config::update_active(|c| c.coder.model = model.to_string());
    // No runtime rebuild — the coder is a sidecar invoked per-call by
    // Codet via `coder_model()`, which re-reads `model_config::active()`
    // on every use.
    let _ = writeln!(
        out,
        "{} {}",
        theme::ROBOT,
        theme::ok(&format!("coder pinned → {}", cfg.coder.model))
    );
}

fn handle_models(out: &mut impl Write) {
    print_models(out, &model_config::active(), None);
}

fn print_models(out: &mut impl Write, cfg: &ModelConfig, just_switched: Option<Preset>) {
    let _ = writeln!(out, "{} {}", theme::ROBOT, theme::accent("models"));
    if let Some(p) = just_switched {
        let _ = writeln!(
            out,
            "  {} {}",
            theme::ok(theme::OK_GLYPH),
            theme::ok(&format!("preset switched to {p}"))
        );
    } else {
        let _ = writeln!(
            out,
            "  {} preset: {}",
            theme::dim("•"),
            theme::ok(&cfg.preset.to_string())
        );
    }
    let _ = writeln!(
        out,
        "  {} brain: {} {}",
        theme::dim("•"),
        theme::ok(&cfg.brain.model),
        theme::dim(&format!(
            "(num_ctx={}, num_predict={})",
            cfg.brain.num_ctx, cfg.brain.num_predict
        ))
    );
    match &cfg.fallback_brain {
        Some(fb) => {
            let _ = writeln!(
                out,
                "  {} fallback: {} {}",
                theme::dim("•"),
                theme::ok(&fb.model),
                theme::dim("(used when primary is stuck; reverts after success)")
            );
        }
        None => {
            let _ = writeln!(
                out,
                "  {} fallback: {}",
                theme::dim("•"),
                theme::dim("none (no auto-escalation)")
            );
        }
    }
    let _ = writeln!(
        out,
        "  {} coder: {} {}",
        theme::dim("•"),
        theme::ok(&cfg.coder.model),
        theme::dim(&format!(
            "(num_ctx={}, num_predict={})",
            cfg.coder.num_ctx, cfg.coder.num_predict
        ))
    );
    if let Some(ts) = last_fallback_event() {
        let _ = writeln!(
            out,
            "  {} last fallback: {}",
            theme::dim("•"),
            theme::dim(&ts)
        );
    }
}

/// Read the newest timestamp from `~/.claudette/fallback.jsonl`, if any.
/// Silent on missing/empty file — fallback logging is best-effort.
fn last_fallback_event() -> Option<String> {
    let path = crate::model_config::default_toml_path()
        .parent()?
        .join("fallback.jsonl");
    let content = std::fs::read_to_string(&path).ok()?;
    let last = content.lines().rfind(|l| !l.trim().is_empty())?;
    // Pull the "ts" field without a full JSON parser if possible; fall back
    // to serde_json for anything weirder.
    serde_json::from_str::<serde_json::Value>(last)
        .ok()
        .and_then(|v| v.get("ts").and_then(|t| t.as_str()).map(String::from))
}

/// Rebuild the runtime in place so the next turn uses the updated brain
/// model. Preserves the full message history. Matches the pattern
/// `/clear` / `/reload` / `/load` already use.
fn rebuild_after_model_swap(runtime: &mut SecretaryRuntime) {
    let session = runtime.session().clone();
    *runtime = build_runtime_streaming(session, false);
}

fn handle_validate(out: &mut impl Write, path_str: &str) {
    let path = std::path::Path::new(path_str);
    // Expand tilde for convenience.
    let resolved = if path_str.starts_with("~/") || path_str.starts_with("~\\") {
        crate::tools::expand_tilde(path_str)
    } else {
        path.to_path_buf()
    };
    if !resolved.exists() {
        let _ = writeln!(
            out,
            "{} {}",
            theme::error(theme::ERR_GLYPH),
            theme::error(&format!("file not found: {}", resolved.display()))
        );
        return;
    }

    let _ = writeln!(
        out,
        "{} {} {}",
        theme::GEAR,
        theme::accent("validating"),
        theme::dim(&resolved.display().to_string())
    );

    match crate::codet::validate_code_file(&resolved, &[]) {
        None => {
            let _ = writeln!(
                out,
                "  {}",
                theme::dim("(not a known code file type — nothing to validate)")
            );
        }
        Some(result) => {
            let _ = writeln!(
                out,
                "  {} syntax: {}",
                theme::dim("•"),
                if result.syntax_ok {
                    theme::ok("ok")
                } else {
                    theme::error("failed")
                }
            );
            if result.tests_found {
                let _ = writeln!(
                    out,
                    "  {} tests: {} passed, {} failed, {} errors",
                    theme::dim("•"),
                    result.tests_passed,
                    result.tests_failed,
                    result.tests_errors
                );
            } else {
                let _ = writeln!(
                    out,
                    "  {} tests: {}",
                    theme::dim("•"),
                    theme::dim("none found")
                );
            }
            if result.fixes_applied > 0 {
                let _ = writeln!(
                    out,
                    "  {} fixes applied: {} — {}",
                    theme::dim("•"),
                    result.fixes_applied,
                    result.fix_summary
                );
            }
            match &result.status {
                crate::codet::CodetStatus::AllPassed => {
                    let _ = writeln!(
                        out,
                        "  {} {}",
                        theme::ok(theme::OK_GLYPH),
                        theme::ok("all checks passed")
                    );
                }
                crate::codet::CodetStatus::FixedAll => {
                    let _ = writeln!(
                        out,
                        "  {} {}",
                        theme::ok(theme::OK_GLYPH),
                        theme::ok("all checks passed (after Codet fixes)")
                    );
                }
                crate::codet::CodetStatus::CouldNotFix { last_error } => {
                    let short: String = last_error.lines().take(3).collect::<Vec<_>>().join(" | ");
                    let _ = writeln!(
                        out,
                        "  {} {}",
                        theme::error(theme::ERR_GLYPH),
                        theme::error(&format!("could not fix: {short}"))
                    );
                }
                crate::codet::CodetStatus::Skipped => {
                    let _ = writeln!(out, "  {}", theme::dim("(validation skipped)"));
                }
            }
        }
    }
}

fn handle_capabilities(out: &mut impl Write) {
    let _ = writeln!(out, "{} {}", theme::GEAR, theme::accent("capabilities"));
    let _ = writeln!(
        out,
        "  {} model: {}",
        theme::dim("•"),
        theme::ok(&current_model())
    );
    let _ = writeln!(out, "  {} num_ctx: {}", theme::dim("•"), current_num_ctx());
    let _ = writeln!(
        out,
        "  {} compact threshold: {}",
        theme::dim("•"),
        compact_threshold()
    );
    let _ = writeln!(
        out,
        "  {} memory file: {}",
        theme::dim("•"),
        theme::dim(&default_memory_path().display().to_string())
    );
    let _ = writeln!(
        out,
        "  {} session file: {}",
        theme::dim("•"),
        theme::dim(&default_session_path().display().to_string())
    );
    let registry = ToolRegistry::new();
    let core_count = registry.core_tool_names().len();
    let optional_count: usize = ToolGroup::all()
        .iter()
        .map(|g| registry.group_tool_names(*g).len())
        .sum();
    let _ = writeln!(
        out,
        "  {} tools: {} core + {} in {} groups (on-demand)",
        theme::dim("•"),
        core_count,
        optional_count,
        ToolGroup::all().len(),
    );
    let _ = writeln!(
        out,
        "  {} version: {}",
        theme::dim("•"),
        env!("CARGO_PKG_VERSION")
    );
}

// === Helpers =================================================================

/// Validate a session name for `/save` and `/load`. Restricts to ASCII
/// alphanumerics, `-`, `_` so we never construct a path that escapes the
/// `sessions/` directory or hits Windows reserved names like `CON`/`PRN`.
fn sanitize_session_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("session name is empty".to_string());
    }
    if n.len() > 64 {
        return Err(format!("session name too long ({} > 64 chars)", n.len()));
    }
    if n.starts_with('.') {
        return Err("session name cannot start with '.'".to_string());
    }
    for c in n.chars() {
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!(
                "session name contains illegal character {c:?} (allowed: alphanumerics, '-', '_')"
            ));
        }
    }
    Ok(n.to_string())
}

/// Take the first sentence of `s` — that is, everything up to the first
/// "period followed by whitespace" (`. `, `.\n`, `.\t`). Falls back to the
/// whole string if there's no sentence-end. Then char-truncates to
/// `max_chars` if still too long. Char-based truncation, never byte
/// slicing, so multibyte glyphs don't blow up the formatter.
///
/// Splitting on `". "` (instead of bare `.`) is the fix for a `/tools` bug
/// where descriptions like `(~/.claudette/files/)` were being cut at the
/// path's first dot, producing output like `Write text content to a file
/// in the secretary's scratch directory (~/`. Real prose ends sentences
/// with a period followed by a space, so this gives us the right answer
/// for both natural text and inline path strings.
fn first_sentence(s: &str, max_chars: usize) -> String {
    let head = sentence_head(s).unwrap_or(s);
    if head.chars().count() <= max_chars {
        return head.to_string();
    }
    head.chars().take(max_chars).collect()
}

/// Find the first sentence-end in `s` and return everything before it.
/// A sentence-end is `.`, `!`, or `?` followed by ASCII whitespace.
/// Returns `None` if there's no sentence boundary in the input.
fn sentence_head(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        let here = bytes[i];
        if matches!(here, b'.' | b'!' | b'?') {
            let next = bytes[i + 1];
            if next.is_ascii_whitespace() {
                return Some(&s[..i]);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_returns_none_for_non_slash_lines() {
        assert!(parse_slash_command("hello").is_none());
        assert!(parse_slash_command("").is_none());
        assert!(parse_slash_command("  ").is_none());
        assert!(parse_slash_command("what time is it?").is_none());
    }

    #[test]
    fn parse_help_aliases() {
        assert_eq!(parse_slash_command("/help"), Some(SlashCommand::Help));
        assert_eq!(parse_slash_command("/h"), Some(SlashCommand::Help));
        assert_eq!(parse_slash_command("/?"), Some(SlashCommand::Help));
    }

    #[test]
    fn parse_clear_aliases() {
        assert_eq!(parse_slash_command("/clear"), Some(SlashCommand::Clear));
        assert_eq!(parse_slash_command("/cl"), Some(SlashCommand::Clear));
    }

    #[test]
    fn parse_simple_commands() {
        assert_eq!(parse_slash_command("/compact"), Some(SlashCommand::Compact));
        assert_eq!(
            parse_slash_command("/sessions"),
            Some(SlashCommand::Sessions)
        );
        assert_eq!(parse_slash_command("/ls"), Some(SlashCommand::Sessions));
        assert_eq!(parse_slash_command("/status"), Some(SlashCommand::Status));
        assert_eq!(parse_slash_command("/st"), Some(SlashCommand::Status));
        assert_eq!(parse_slash_command("/cost"), Some(SlashCommand::Cost));
        assert_eq!(parse_slash_command("/tools"), Some(SlashCommand::Tools));
        assert_eq!(parse_slash_command("/model"), Some(SlashCommand::Model));
        assert_eq!(parse_slash_command("/memory"), Some(SlashCommand::Memory));
        assert_eq!(parse_slash_command("/mem"), Some(SlashCommand::Memory));
        assert_eq!(parse_slash_command("/reload"), Some(SlashCommand::Reload));
        assert_eq!(
            parse_slash_command("/capabilities"),
            Some(SlashCommand::Capabilities)
        );
        assert_eq!(
            parse_slash_command("/cap"),
            Some(SlashCommand::Capabilities)
        );
    }

    #[test]
    fn parse_exit_aliases() {
        for alias in ["/exit", "/quit", "/q", "/x"] {
            assert_eq!(parse_slash_command(alias), Some(SlashCommand::Exit));
        }
    }

    #[test]
    fn parse_save_with_name() {
        assert_eq!(
            parse_slash_command("/save my-work"),
            Some(SlashCommand::Save("my-work".to_string()))
        );
        assert_eq!(
            parse_slash_command("/save   spaces  "),
            Some(SlashCommand::Save("spaces".to_string()))
        );
    }

    #[test]
    fn parse_save_without_name_is_invalid() {
        let parsed = parse_slash_command("/save");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
        let parsed = parse_slash_command("/save   ");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_load_with_name() {
        assert_eq!(
            parse_slash_command("/load deep-research"),
            Some(SlashCommand::Load("deep-research".to_string()))
        );
    }

    #[test]
    fn parse_load_without_name_is_invalid() {
        let parsed = parse_slash_command("/load");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_validate_with_path() {
        assert_eq!(
            parse_slash_command("/validate ~/foo.py"),
            Some(SlashCommand::Validate("~/foo.py".to_string()))
        );
        assert_eq!(
            parse_slash_command("/val test.py"),
            Some(SlashCommand::Validate("test.py".to_string()))
        );
    }

    #[test]
    fn parse_validate_without_path_is_invalid() {
        let parsed = parse_slash_command("/validate");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_agents() {
        assert_eq!(parse_slash_command("/agents"), Some(SlashCommand::Agents));
        assert_eq!(parse_slash_command("/AGENTS"), Some(SlashCommand::Agents));
    }

    #[test]
    fn parse_preset_variants() {
        assert_eq!(
            parse_slash_command("/preset fast"),
            Some(SlashCommand::PresetSwitch(Preset::Fast))
        );
        assert_eq!(
            parse_slash_command("/preset auto"),
            Some(SlashCommand::PresetSwitch(Preset::Auto))
        );
        assert_eq!(
            parse_slash_command("/preset SMART"),
            Some(SlashCommand::PresetSwitch(Preset::Smart))
        );
    }

    #[test]
    fn parse_preset_without_arg_is_invalid() {
        let parsed = parse_slash_command("/preset");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_preset_with_unknown_arg_is_invalid() {
        let parsed = parse_slash_command("/preset balanced");
        match parsed {
            Some(SlashCommand::Invalid(msg)) => assert!(msg.contains("balanced")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn parse_brain_with_model_name() {
        assert_eq!(
            parse_slash_command("/brain qwen3.5:9b"),
            Some(SlashCommand::Brain("qwen3.5:9b".to_string()))
        );
        assert_eq!(
            parse_slash_command("/brain auto"),
            Some(SlashCommand::Brain("auto".to_string()))
        );
    }

    #[test]
    fn parse_brain_without_arg_is_invalid() {
        let parsed = parse_slash_command("/brain");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_coder_with_model_name() {
        assert_eq!(
            parse_slash_command("/coder qwen3-coder:30b"),
            Some(SlashCommand::Coder("qwen3-coder:30b".to_string()))
        );
    }

    #[test]
    fn parse_coder_without_arg_is_invalid() {
        let parsed = parse_slash_command("/coder");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn parse_models_alias() {
        assert_eq!(parse_slash_command("/models"), Some(SlashCommand::Models));
    }

    #[test]
    fn parse_unknown_command_is_invalid() {
        let parsed = parse_slash_command("/whatever");
        match parsed {
            Some(SlashCommand::Invalid(msg)) => assert!(msg.contains("/whatever")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(parse_slash_command("/HELP"), Some(SlashCommand::Help));
        assert_eq!(parse_slash_command("/Status"), Some(SlashCommand::Status));
    }

    #[test]
    fn parse_empty_slash_is_invalid() {
        let parsed = parse_slash_command("/");
        assert!(matches!(parsed, Some(SlashCommand::Invalid(_))));
    }

    #[test]
    fn sanitize_accepts_normal_names() {
        assert_eq!(
            sanitize_session_name("my-work_2026"),
            Ok("my-work_2026".to_string())
        );
        assert_eq!(
            sanitize_session_name("  trimmed  "),
            Ok("trimmed".to_string())
        );
    }

    #[test]
    fn sanitize_rejects_path_chars() {
        for bad in [
            "../escape",
            "with/slash",
            "with\\backslash",
            "with:colon",
            "with space",
            "with.dot",
        ] {
            assert!(sanitize_session_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn sanitize_rejects_empty_and_dotted() {
        assert!(sanitize_session_name("").is_err());
        assert!(sanitize_session_name("   ").is_err());
        assert!(sanitize_session_name(".hidden").is_err());
    }

    #[test]
    fn sanitize_rejects_oversized_names() {
        let long = "a".repeat(65);
        assert!(sanitize_session_name(&long).is_err());
    }

    #[test]
    fn first_sentence_takes_up_to_period_space() {
        assert_eq!(first_sentence("hello. world.", 100), "hello");
    }

    #[test]
    fn first_sentence_does_not_split_on_period_inside_path() {
        // Regression for the `/tools` bug: descriptions like "scratch
        // directory (~/.claudette/files/)" were being cut at the dot
        // inside the path. The split must require a sentence boundary
        // (period followed by whitespace), not just any dot.
        let desc = "Write to scratch (~/.claudette/files/) safely. \
                    Sandboxed always.";
        let head = first_sentence(desc, 200);
        assert_eq!(head, "Write to scratch (~/.claudette/files/) safely");
    }

    #[test]
    fn first_sentence_handles_question_and_exclamation() {
        assert_eq!(first_sentence("hi! there", 100), "hi");
        assert_eq!(first_sentence("really? maybe", 100), "really");
    }

    #[test]
    fn first_sentence_falls_back_to_whole_string_when_no_boundary() {
        let s = "no sentence boundary at all";
        assert_eq!(first_sentence(s, 100), s);
    }

    #[test]
    fn first_sentence_falls_back_to_max_chars_for_runaway_text() {
        let s = "no sentence boundary at all";
        assert_eq!(first_sentence(s, 7), "no sent");
    }

    #[test]
    fn first_sentence_handles_multibyte_safely() {
        // 4 robots, no sentence boundary; cap at 3 = "🤖🤖🤖" (3 chars).
        assert_eq!(first_sentence("🤖🤖🤖🤖", 3), "🤖🤖🤖");
    }

    #[test]
    fn repl_state_records_turns() {
        let mut s = ReplState::default();
        s.record_turn(100, 50);
        s.record_turn(200, 80);
        assert_eq!(s.cumulative_input_tokens, 300);
        assert_eq!(s.cumulative_output_tokens, 130);
        assert_eq!(s.turn_count, 2);
    }
}
