//! On-demand tool group registry (Sprint 8).
//!
//! Claudette advertises a small **core** of tools on every request, plus a
//! `enable_tools(group)` meta-tool. When the model calls `enable_tools`, the
//! chosen group's tools are added to the registry and show up on the next
//! `/api/chat` call. This keeps the base schema flat regardless of how many
//! total tools exist — we pay the schema cost only for the groups the model
//! actually asked for.
//!
//! Why it matters: every char of the `tools` field counts against `num_ctx`.
//! Our 16 K window was spending ~7.5 K chars (≈1.9 K tokens) on the full
//! 30-tool schema every turn, even when the user asked a plain question like
//! "what time is it?". With Sprint 8 the baseline drops to ~3.5 K chars
//! (≈900 tokens) and a specialised group adds ~1-2 K chars only when needed.
//!
//! Mutation model: the registry lives behind an `Arc<Mutex<_>>` shared by
//! [`OllamaApiClient`] (which reads it to build the `tools` field) and
//! [`crate::executor::SecretaryToolExecutor`] (which writes it when the
//! model invokes `enable_tools`). Both halves of the runtime see the same
//! registry, so an enable in one turn is visible on the next turn.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

/// Named tool groups that can be enabled on demand. Core tools are always
/// advertised and are NOT represented here — this enum only covers the
/// optional groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ToolGroup {
    /// Git workflow tools: status, diff, log, add, commit, branch, checkout, push.
    Git,
    /// IDE integration: open in editor, reveal in file manager, open URL.
    Ide,
    /// Code/file/web search: glob, grep, `web_fetch`.
    Search,
    /// Power tools: bash, `edit_file`, `spawn_agent` (delegation).
    Advanced,
    /// Reference lookups: Wikipedia, Open-Meteo weather.
    Facts,
    /// Package registries: crates.io, npmjs.
    Registry,
    /// GitHub API (PRs, issues, code search). Requires `GITHUB_TOKEN`.
    Github,
    /// Market data: `TradingView` quotes/ratings/calendar + `vestige.fi` Algorand ASAs.
    Markets,
    /// Telegram bot: send messages, poll updates, send photos.
    Telegram,
    /// Google Calendar: list / create / update / delete events, RSVP.
    /// Requires `claudette --auth-google` one-time setup.
    Calendar,
}

impl ToolGroup {
    /// Canonical lowercase name the model uses in `enable_tools({group:...})`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Ide => "ide",
            Self::Search => "search",
            Self::Advanced => "advanced",
            Self::Facts => "facts",
            Self::Registry => "registry",
            Self::Github => "github",
            Self::Markets => "markets",
            Self::Telegram => "telegram",
            Self::Calendar => "calendar",
        }
    }

    /// One-line human summary. Shown in the `enable_tools` description so the
    /// model knows what each group contains without the full schema being loaded.
    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Self::Git => "git workflows: status, diff, log, add, commit, branch, checkout, push",
            Self::Ide => "IDE integration: open_in_editor, reveal_in_explorer, open_url",
            Self::Search => "code/file/web search: glob_search, grep_search, web_fetch",
            Self::Advanced => "power tools: bash, edit_file, spawn_agent (delegation)",
            Self::Facts => "reference lookups: wikipedia, weather (no API key needed)",
            Self::Registry => "package registries: crates.io and npmjs metadata",
            Self::Github => "github PRs/issues/code search (requires GITHUB_TOKEN)",
            Self::Markets => "market data: TradingView quotes/ratings/economic calendar, vestige.fi Algorand ASAs",
            Self::Telegram => "telegram bot: send messages, poll updates, send photos (requires TELEGRAM_BOT_TOKEN)",
            Self::Calendar => "google calendar: list/create/update/delete events, RSVP (requires claudette --auth-google)",
        }
    }

    /// All groups in a stable order, for schema generation and tests.
    #[must_use]
    pub fn all() -> [ToolGroup; 10] {
        [
            Self::Git,
            Self::Ide,
            Self::Search,
            Self::Advanced,
            Self::Facts,
            Self::Registry,
            Self::Github,
            Self::Markets,
            Self::Telegram,
            Self::Calendar,
        ]
    }

    /// Case-insensitive parse with a couple of common aliases.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "git" => Some(Self::Git),
            "ide" | "editor" => Some(Self::Ide),
            "search" | "grep" | "glob" => Some(Self::Search),
            "advanced" | "shell" | "power" | "bash" => Some(Self::Advanced),
            "facts" | "wikipedia" | "weather" => Some(Self::Facts),
            "registry" | "crates" | "npm" => Some(Self::Registry),
            "github" | "gh" => Some(Self::Github),
            "markets" | "market" | "tradingview" | "tv" | "vestige" | "stocks" | "crypto" => {
                Some(Self::Markets)
            }
            "telegram" | "tg" | "tg_bot" => Some(Self::Telegram),
            "calendar" | "gcal" | "google-calendar" | "google_calendar" => Some(Self::Calendar),
            _ => None,
        }
    }
}

/// Names of the core tools — always advertised, never gated. Keep in sync
/// with the `json!` array in [`crate::tools::secretary_tools_json`] (plus
/// the synthetic `enable_tools` meta-tool that lives only in this registry).
pub const CORE_TOOL_NAMES: &[&str] = &[
    "enable_tools",
    "get_current_time",
    "note_create",
    "note_list",
    "note_read",
    "note_delete",
    "todo_add",
    "todo_list",
    "todo_complete",
    "todo_uncomplete",
    "todo_delete",
    "read_file",
    "write_file",
    "list_dir",
    "get_capabilities",
    "web_search",
    "generate_code",
];

/// Classify a tool name into its group. Returns `None` for core tools, for
/// unknown names, and for `add_numbers` (kept in `dispatch_tool` for
/// backwards-compat but removed from the schema).
#[must_use]
pub fn group_of(tool: &str) -> Option<ToolGroup> {
    match tool {
        "git_status" | "git_diff" | "git_log" | "git_add" | "git_commit" | "git_branch"
        | "git_checkout" | "git_push" => Some(ToolGroup::Git),
        "open_in_editor" | "reveal_in_explorer" | "open_url" => Some(ToolGroup::Ide),
        "glob_search" | "grep_search" | "web_fetch" => Some(ToolGroup::Search),
        "bash" | "edit_file" | "spawn_agent" => Some(ToolGroup::Advanced),
        "wikipedia_search" | "wikipedia_summary" | "weather_current" | "weather_forecast" => {
            Some(ToolGroup::Facts)
        }
        "crate_info" | "crate_search" | "npm_info" | "npm_search" => Some(ToolGroup::Registry),
        "gh_list_my_prs"
        | "gh_list_assigned_issues"
        | "gh_get_issue"
        | "gh_create_issue"
        | "gh_comment_issue"
        | "gh_search_code" => Some(ToolGroup::Github),
        "tv_get_quote"
        | "tv_technical_rating"
        | "tv_search_symbol"
        | "tv_economic_calendar"
        | "vestige_asa_info"
        | "vestige_search_asa"
        | "vestige_top_movers" => Some(ToolGroup::Markets),
        "tg_send" | "tg_get_updates" | "tg_send_photo" => Some(ToolGroup::Telegram),
        "calendar_list_events"
        | "calendar_create_event"
        | "calendar_update_event"
        | "calendar_delete_event"
        | "calendar_respond_to_event" => Some(ToolGroup::Calendar),
        _ => None,
    }
}

/// Mutable tool registry shared between [`crate::api::OllamaApiClient`] and
/// [`crate::executor::SecretaryToolExecutor`]. Holds the core tools (always
/// included) plus the optional groups keyed by [`ToolGroup`], and the set of
/// currently-enabled groups.
pub struct ToolRegistry {
    /// Core tools — always in the output of [`Self::current_tools`].
    /// Includes the synthetic `enable_tools` meta-tool at index 0.
    core: Vec<Value>,
    /// Group → list of tool JSON objects. Populated once at construction.
    groups: BTreeMap<ToolGroup, Vec<Value>>,
    /// Currently enabled groups. Starts empty.
    enabled: BTreeSet<ToolGroup>,
}

impl ToolRegistry {
    /// Build the registry from the full tool list in [`crate::tools`].
    ///
    /// The `enable_tools` meta-tool is **synthesized** here (not loaded from
    /// `secretary_tools_json`) because it's stateful — its implementation
    /// lives in the executor, not in [`crate::tools::dispatch_tool`], so we
    /// don't want it in the stateless tool list.
    #[must_use]
    pub fn new() -> Self {
        let full = crate::tools::secretary_tools_json();
        let arr = full.as_array().cloned().unwrap_or_default();

        let mut core: Vec<Value> = Vec::with_capacity(CORE_TOOL_NAMES.len());
        let mut groups: BTreeMap<ToolGroup, Vec<Value>> = BTreeMap::new();

        // Always-on meta-tool goes first so the model sees it at the top of
        // the schema.
        core.push(enable_tools_schema());

        for tool in arr {
            let Some(name) = tool
                .pointer("/function/name")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            if CORE_TOOL_NAMES.contains(&name.as_str()) {
                core.push(tool);
            } else if let Some(group) = group_of(&name) {
                groups.entry(group).or_default().push(tool);
            }
            // Any tool that is neither core nor in a group (currently none)
            // is dropped from the advertised schema. Dispatch still works if
            // the model somehow calls it by name.
        }

        Self {
            core,
            groups,
            enabled: BTreeSet::new(),
        }
    }

    /// Merge `core` + all currently-enabled groups into the JSON array that
    /// ships to Ollama in the `tools` field.
    #[must_use]
    pub fn current_tools(&self) -> Value {
        let mut out: Vec<Value> = Vec::with_capacity(self.current_len());
        out.extend(self.core.iter().cloned());
        for group in &self.enabled {
            if let Some(tools) = self.groups.get(group) {
                out.extend(tools.iter().cloned());
            }
        }
        Value::Array(out)
    }

    /// How many tools [`current_tools`] would emit right now.
    #[must_use]
    pub fn current_len(&self) -> usize {
        self.core.len()
            + self
                .enabled
                .iter()
                .map(|g| self.groups.get(g).map_or(0, Vec::len))
                .sum::<usize>()
    }

    /// Char-count of the JSON-serialized current tool array. Useful for the
    /// `/tools` slash command to surface the schema size impact of enabling
    /// a group.
    #[must_use]
    pub fn current_schema_chars(&self) -> usize {
        self.current_tools().to_string().len()
    }

    /// Enable `group`. Returns `true` if this is the first time the group has
    /// been enabled in this registry's lifetime (useful for reporting).
    pub fn enable(&mut self, group: ToolGroup) -> bool {
        self.enabled.insert(group)
    }

    /// Whether `group` is currently enabled.
    #[must_use]
    pub fn is_enabled(&self, group: ToolGroup) -> bool {
        self.enabled.contains(&group)
    }

    /// Snapshot of enabled groups in stable order.
    #[must_use]
    pub fn enabled_groups(&self) -> Vec<ToolGroup> {
        self.enabled.iter().copied().collect()
    }

    /// List the tool names in `group`, in schema order. Used by `run_enable_tools`
    /// to tell the model what's newly available.
    #[must_use]
    pub fn group_tool_names(&self, group: ToolGroup) -> Vec<String> {
        self.groups
            .get(&group)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|t| {
                        t.pointer("/function/name")
                            .and_then(Value::as_str)
                            .map(String::from)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// List of core tool names (for `/tools` display). Returns the synthesized
    /// `enable_tools` first, followed by the core tools pulled from
    /// `secretary_tools_json`.
    #[must_use]
    pub fn core_tool_names(&self) -> Vec<String> {
        self.core
            .iter()
            .filter_map(|t| {
                t.pointer("/function/name")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the JSON schema for the `enable_tools` meta-tool. The description
/// lists every group with its one-line summary so the model can pick the
/// right one without the full schema being loaded.
fn enable_tools_schema() -> Value {
    // Description spells out every group + summary so the model can pick
    // without a second tool call. Keep the prose terse to stay within the
    // Qwen tool-description budget.
    let group_lines: Vec<String> = ToolGroup::all()
        .iter()
        .map(|g| format!("{} ({})", g.name(), g.summary()))
        .collect();
    let description = format!(
        "Enable an optional tool group when you need tools beyond the core set. \
         The new tools become available on the next turn. Groups: {}.",
        group_lines.join("; ")
    );

    let enum_values: Vec<Value> = ToolGroup::all()
        .iter()
        .map(|g| Value::String(g.name().to_string()))
        .collect();

    json!({
        "type": "function",
        "function": {
            "name": "enable_tools",
            "description": description,
            "parameters": {
                "type": "object",
                "properties": {
                    "group": {
                        "type": "string",
                        "enum": enum_values,
                        "description": "Group name: git, ide, search, or advanced"
                    }
                },
                "required": ["group"]
            }
        }
    })
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_group_canonical() {
        assert_eq!(ToolGroup::parse("git"), Some(ToolGroup::Git));
        assert_eq!(ToolGroup::parse("ide"), Some(ToolGroup::Ide));
        assert_eq!(ToolGroup::parse("search"), Some(ToolGroup::Search));
        assert_eq!(ToolGroup::parse("advanced"), Some(ToolGroup::Advanced));
        assert_eq!(ToolGroup::parse("facts"), Some(ToolGroup::Facts));
        assert_eq!(ToolGroup::parse("registry"), Some(ToolGroup::Registry));
        assert_eq!(ToolGroup::parse("github"), Some(ToolGroup::Github));
        assert_eq!(ToolGroup::parse("markets"), Some(ToolGroup::Markets));
        assert_eq!(ToolGroup::parse("tradingview"), Some(ToolGroup::Markets));
        assert_eq!(ToolGroup::parse("vestige"), Some(ToolGroup::Markets));
        assert_eq!(ToolGroup::parse("telegram"), Some(ToolGroup::Telegram));
    }

    #[test]
    fn parse_group_aliases() {
        assert_eq!(ToolGroup::parse("GIT"), Some(ToolGroup::Git));
        assert_eq!(ToolGroup::parse("  git "), Some(ToolGroup::Git));
        assert_eq!(ToolGroup::parse("editor"), Some(ToolGroup::Ide));
        assert_eq!(ToolGroup::parse("grep"), Some(ToolGroup::Search));
        assert_eq!(ToolGroup::parse("shell"), Some(ToolGroup::Advanced));
        assert_eq!(ToolGroup::parse("bash"), Some(ToolGroup::Advanced));
        assert_eq!(ToolGroup::parse("wikipedia"), Some(ToolGroup::Facts));
        assert_eq!(ToolGroup::parse("weather"), Some(ToolGroup::Facts));
        assert_eq!(ToolGroup::parse("crates"), Some(ToolGroup::Registry));
        assert_eq!(ToolGroup::parse("npm"), Some(ToolGroup::Registry));
        assert_eq!(ToolGroup::parse("gh"), Some(ToolGroup::Github));
        assert_eq!(ToolGroup::parse("tg"), Some(ToolGroup::Telegram));
    }

    #[test]
    fn parse_group_unknown() {
        assert_eq!(ToolGroup::parse(""), None);
        assert_eq!(ToolGroup::parse("unknown"), None);
        assert_eq!(ToolGroup::parse("core"), None);
    }

    #[test]
    fn group_of_classifies_known_tools() {
        assert_eq!(group_of("git_status"), Some(ToolGroup::Git));
        assert_eq!(group_of("git_push"), Some(ToolGroup::Git));
        assert_eq!(group_of("open_in_editor"), Some(ToolGroup::Ide));
        assert_eq!(group_of("glob_search"), Some(ToolGroup::Search));
        assert_eq!(group_of("web_fetch"), Some(ToolGroup::Search));
        assert_eq!(group_of("bash"), Some(ToolGroup::Advanced));
        assert_eq!(group_of("spawn_agent"), Some(ToolGroup::Advanced));
        // Sprint 9 Phase 0a additions.
        assert_eq!(group_of("wikipedia_search"), Some(ToolGroup::Facts));
        assert_eq!(group_of("weather_forecast"), Some(ToolGroup::Facts));
        assert_eq!(group_of("crate_info"), Some(ToolGroup::Registry));
        assert_eq!(group_of("npm_search"), Some(ToolGroup::Registry));
        assert_eq!(group_of("gh_list_my_prs"), Some(ToolGroup::Github));
        assert_eq!(group_of("gh_create_issue"), Some(ToolGroup::Github));
        assert_eq!(group_of("tv_get_quote"), Some(ToolGroup::Markets));
        assert_eq!(group_of("tv_technical_rating"), Some(ToolGroup::Markets));
        assert_eq!(group_of("vestige_asa_info"), Some(ToolGroup::Markets));
        assert_eq!(group_of("vestige_top_movers"), Some(ToolGroup::Markets));
        assert_eq!(group_of("tg_send"), Some(ToolGroup::Telegram));
        assert_eq!(group_of("tg_get_updates"), Some(ToolGroup::Telegram));
        assert_eq!(group_of("tg_send_photo"), Some(ToolGroup::Telegram));
    }

    #[test]
    fn group_of_returns_none_for_core() {
        for &name in CORE_TOOL_NAMES {
            assert_eq!(
                group_of(name),
                None,
                "core tool {name} should not map to a group"
            );
        }
    }

    #[test]
    fn registry_starts_with_only_core() {
        let reg = ToolRegistry::new();
        assert!(reg.enabled_groups().is_empty());
        // Core should include every CORE_TOOL_NAMES entry that exists in
        // secretary_tools_json plus the synthetic enable_tools.
        let core_names = reg.core_tool_names();
        assert!(core_names.contains(&"enable_tools".to_string()));
        assert!(core_names.contains(&"get_current_time".to_string()));
        assert!(core_names.contains(&"read_file".to_string()));
        assert!(core_names.contains(&"generate_code".to_string()));
    }

    #[test]
    fn registry_current_tools_starts_at_core_size() {
        let reg = ToolRegistry::new();
        let tools = reg.current_tools();
        let arr = tools.as_array().expect("tools should be an array");
        assert_eq!(arr.len(), reg.core.len());
        assert_eq!(arr.len(), reg.current_len());
    }

    #[test]
    fn enable_group_adds_tools_to_current() {
        let mut reg = ToolRegistry::new();
        let base = reg.current_len();

        let newly_enabled = reg.enable(ToolGroup::Git);
        assert!(newly_enabled);
        assert!(reg.is_enabled(ToolGroup::Git));

        let after = reg.current_len();
        assert!(
            after > base,
            "enabling git should add tools (base={base}, after={after})"
        );

        // Contains git_status now.
        let arr = reg.current_tools();
        let names: Vec<&str> = arr
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert!(names.contains(&"git_status"));
        assert!(names.contains(&"enable_tools"));
    }

    #[test]
    fn enable_group_idempotent() {
        let mut reg = ToolRegistry::new();
        let first = reg.enable(ToolGroup::Ide);
        let second = reg.enable(ToolGroup::Ide);
        assert!(first, "first enable reports new");
        assert!(!second, "second enable reports already-on");
        assert_eq!(reg.enabled_groups(), vec![ToolGroup::Ide]);
    }

    #[test]
    fn enable_multiple_groups_combines_tools() {
        let mut reg = ToolRegistry::new();
        reg.enable(ToolGroup::Git);
        reg.enable(ToolGroup::Search);

        let names: Vec<String> = reg
            .current_tools()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| {
                t.pointer("/function/name")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .collect();
        assert!(names.contains(&"git_commit".to_string()));
        assert!(names.contains(&"grep_search".to_string()));
        // Advanced not enabled.
        assert!(!names.contains(&"bash".to_string()));
    }

    #[test]
    fn group_tool_names_returns_schema_order() {
        let reg = ToolRegistry::new();
        let git = reg.group_tool_names(ToolGroup::Git);
        assert!(git.contains(&"git_status".to_string()));
        assert!(git.contains(&"git_push".to_string()));
        // All Git group tools should be non-empty.
        assert_eq!(git.len(), 8);
    }

    #[test]
    fn schema_chars_grows_with_enables() {
        let mut reg = ToolRegistry::new();
        let core_only = reg.current_schema_chars();
        reg.enable(ToolGroup::Git);
        let with_git = reg.current_schema_chars();
        assert!(
            with_git > core_only,
            "enabling git should grow schema (core={core_only}, with_git={with_git})"
        );
    }

    #[test]
    fn enable_tools_schema_mentions_every_group() {
        let schema = enable_tools_schema();
        let desc = schema
            .pointer("/function/description")
            .and_then(Value::as_str)
            .unwrap_or("");
        for g in ToolGroup::all() {
            assert!(
                desc.contains(g.name()),
                "description should mention {}: {desc}",
                g.name()
            );
        }
    }

    #[test]
    fn enable_tools_schema_enum_matches_groups() {
        let schema = enable_tools_schema();
        let enum_arr = schema
            .pointer("/function/parameters/properties/group/enum")
            .and_then(Value::as_array)
            .expect("group enum should be an array");
        let values: Vec<&str> = enum_arr.iter().filter_map(Value::as_str).collect();
        assert_eq!(values.len(), ToolGroup::all().len());
        for g in ToolGroup::all() {
            assert!(values.contains(&g.name()));
        }
    }

    /// Report the concrete schema-size numbers so the author (and anyone
    /// reading the memory doc) can cite them without rerunning by hand.
    /// Run with `cargo test -p claudette schema_size_report -- --nocapture`.
    #[test]
    fn schema_size_report() {
        let old_full = crate::tools::secretary_tools_json().to_string().len();

        let reg = ToolRegistry::new();
        let core_only = reg.current_schema_chars();
        let core_count = reg.core_tool_names().len();

        let mut git_only = ToolRegistry::new();
        git_only.enable(ToolGroup::Git);
        let with_git = git_only.current_schema_chars();

        let mut all = ToolRegistry::new();
        for g in ToolGroup::all() {
            all.enable(g);
        }
        let with_all = all.current_schema_chars();
        let all_count = all.current_len();

        eprintln!("─── schema size report ───");
        eprintln!("old flat registry (30 tools):  {old_full} chars");
        eprintln!("core only ({core_count} tools):                {core_only} chars");
        eprintln!("core + git:                    {with_git} chars");
        eprintln!("core + all groups ({all_count} tools):     {with_all} chars");
        eprintln!(
            "savings vs old, core-only:     {} chars (~{}%)",
            old_full.saturating_sub(core_only),
            100 * (old_full.saturating_sub(core_only)) / old_full.max(1),
        );

        // Core-only should be strictly smaller than the old flat schema.
        assert!(core_only < old_full);
        // And core + all groups should be close to (but not exactly) the
        // old flat schema — it's larger by the enable_tools meta-tool
        // (synthesized, not in the old list).
        assert!(with_all >= old_full);
    }

    #[test]
    fn advanced_group_contains_spawn_agent() {
        let reg = ToolRegistry::new();
        let advanced = reg.group_tool_names(ToolGroup::Advanced);
        assert!(advanced.contains(&"bash".to_string()));
        assert!(advanced.contains(&"edit_file".to_string()));
        assert!(advanced.contains(&"spawn_agent".to_string()));
    }
}
