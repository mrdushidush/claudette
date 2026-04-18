//! Multi-agent team for the claudette secretary.
//!
//! Sprint 6 adds specialised agents that Claudette can delegate to via the
//! `spawn_agent` tool. Each agent is a synchronous, isolated
//! `ConversationRuntime<OllamaApiClient, FilteredToolExecutor>` with its own
//! tool subset, system prompt, and permission policy. Agents share the same
//! Ollama model as Claudette by default (no VRAM swap needed).
//!
//! Architecture mirrors Codet's isolation principle: Claudette sees only the
//! agent's final text output, never its internal tool calls or reasoning.

use std::collections::BTreeSet;
use std::fmt;

use crate::{
    ContentBlock, ConversationRuntime, PermissionMode, PermissionPolicy, Session, ToolError,
    ToolExecutor, TurnSummary,
};
use serde_json::Value;

use crate::api::OllamaApiClient;
use crate::run::{current_model, CliPrompter};
use crate::tools::secretary_tools_json;

// ────────────────────────────────────────────────────────────────────────────
// Agent types
// ────────────────────────────────────────────────────────────────────────────

/// Available agent types that Claudette can delegate to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentType {
    /// Deep web research, file reading, code search.
    Researcher,
    /// Git workflows: status, diff, add, commit, push, branch management.
    GitOps,
    /// Code review: reads files, finds bugs, security issues, quality problems.
    CodeReviewer,
}

impl AgentType {
    /// Parse a string into an agent type. Case-insensitive.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "researcher" | "research" => Some(Self::Researcher),
            "gitops" | "git" => Some(Self::GitOps),
            "code_reviewer" | "reviewer" | "review" | "code-review" => Some(Self::CodeReviewer),
            _ => None,
        }
    }

    /// Build the full configuration for this agent type.
    #[must_use]
    pub fn config(self) -> AgentConfig {
        let base_prompt = match self {
            Self::Researcher => {
                "You are a research agent. Investigate the given topic \
                thoroughly. Use multiple web searches with different queries, \
                fetch pages for detail, and cross-reference sources. \
                Do NOT stop after one search — dig deeper, look for recent news, \
                primary sources, and specific facts. \
                Only write your final summary after at least 2-3 searches."
            }
            Self::GitOps => {
                "You are a git operations agent. Execute the requested \
                git workflow safely. Always check status before destructive \
                operations."
            }
            Self::CodeReviewer => {
                "You are a code review agent. Read the specified code files \
                and provide a thorough review covering: bugs, logic errors, security \
                vulnerabilities, performance issues, error handling gaps, code quality, \
                and adherence to best practices. Be specific — cite line numbers and \
                suggest concrete fixes. Rate severity: critical, warning, or suggestion."
            }
        };

        let system_prompt = build_agent_prompt(base_prompt);

        match self {
            Self::Researcher => AgentConfig {
                agent_type: self,
                system_prompt,
                allowed_tools: researcher_tools(),
                max_iterations: researcher_max_iter(),
                model: researcher_model(),
                num_ctx: crate::api::current_num_ctx(),
            },
            Self::GitOps => AgentConfig {
                agent_type: self,
                system_prompt,
                allowed_tools: gitops_tools(),
                max_iterations: gitops_max_iter(),
                model: gitops_model(),
                num_ctx: crate::api::current_num_ctx(),
            },
            Self::CodeReviewer => AgentConfig {
                agent_type: self,
                system_prompt,
                allowed_tools: code_reviewer_tools(),
                max_iterations: DEFAULT_CODE_REVIEWER_MAX_ITER,
                model: current_model(),
                num_ctx: crate::api::current_num_ctx(),
            },
        }
    }
}

impl fmt::Display for AgentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Researcher => write!(f, "researcher"),
            Self::GitOps => write!(f, "gitops"),
            Self::CodeReviewer => write!(f, "code_reviewer"),
        }
    }
}

/// Full configuration for a spawned agent.
pub struct AgentConfig {
    pub agent_type: AgentType,
    pub system_prompt: String,
    pub allowed_tools: BTreeSet<String>,
    pub max_iterations: usize,
    pub model: String,
    pub num_ctx: u32,
}

// ────────────────────────────────────────────────────────────────────────────
// Tool allowlists
// ────────────────────────────────────────────────────────────────────────────

fn researcher_tools() -> BTreeSet<String> {
    [
        "web_search",
        "web_fetch",
        "read_file",
        "list_dir",
        "glob_search",
        "grep_search",
        "get_current_time",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn code_reviewer_tools() -> BTreeSet<String> {
    [
        "read_file",
        "list_dir",
        "glob_search",
        "grep_search",
        "get_current_time",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn gitops_tools() -> BTreeSet<String> {
    [
        "git_status",
        "git_diff",
        "git_log",
        "git_add",
        "git_commit",
        "git_branch",
        "git_checkout",
        "git_push",
        "bash",
        "read_file",
        "list_dir",
        "glob_search",
        "grep_search",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

// ────────────────────────────────────────────────────────────────────────────
// Env-var overrides
// ────────────────────────────────────────────────────────────────────────────

const DEFAULT_RESEARCHER_MAX_ITER: usize = 10;
const DEFAULT_GITOPS_MAX_ITER: usize = 8;
const DEFAULT_CODE_REVIEWER_MAX_ITER: usize = 5;

fn researcher_model() -> String {
    std::env::var("CLAUDETTE_RESEARCHER_MODEL").unwrap_or_else(|_| current_model())
}

fn gitops_model() -> String {
    std::env::var("CLAUDETTE_GITOPS_MODEL").unwrap_or_else(|_| current_model())
}

fn researcher_max_iter() -> usize {
    std::env::var("CLAUDETTE_RESEARCHER_MAX_ITER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_RESEARCHER_MAX_ITER)
}

fn gitops_max_iter() -> usize {
    std::env::var("CLAUDETTE_GITOPS_MAX_ITER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_GITOPS_MAX_ITER)
}

// ────────────────────────────────────────────────────────────────────────────
// Agent prompt construction
// ────────────────────────────────────────────────────────────────────────────

/// Build a system prompt for an agent: base role prompt + environment context
/// (cwd, date, OS, git status, CLAUDETTE.md project instructions). Keeps the
/// prompt terse per qwen constraint — environment is appended, not inflated.
fn build_agent_prompt(base: &str) -> String {
    let mut prompt = base.to_string();
    if let Some(env) = crate::prompt::build_environment_block() {
        use std::fmt::Write;
        let _ = write!(prompt, "\n\n{env}");
    }
    prompt
}

// ────────────────────────────────────────────────────────────────────────────
// FilteredToolExecutor
// ────────────────────────────────────────────────────────────────────────────

/// A `ToolExecutor` that only permits a subset of tools. Rejects disallowed
/// tool calls with a clear error so the model can adjust. Dispatches allowed
/// calls through the same `tools::dispatch_tool` as the main secretary.
pub struct FilteredToolExecutor {
    allowed: BTreeSet<String>,
}

impl FilteredToolExecutor {
    #[must_use]
    pub fn new(allowed: BTreeSet<String>) -> Self {
        Self { allowed }
    }
}

impl ToolExecutor for FilteredToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        if !self.allowed.contains(tool_name) {
            return Err(ToolError::new(format!(
                "tool `{tool_name}` is not available for this agent"
            )));
        }
        crate::tools::dispatch_tool(tool_name, input).map_err(ToolError::new)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tool JSON subsetting
// ────────────────────────────────────────────────────────────────────────────

/// Filter the full tool registry JSON to only include tools whose
/// `function.name` appears in `allowed`. Used to build the Ollama
/// `/api/chat` `tools` parameter for a spawned agent.
#[must_use]
pub fn filter_tools_json(full_tools: &Value, allowed: &BTreeSet<String>) -> Value {
    let empty = vec![];
    let arr = full_tools.as_array().unwrap_or(&empty);
    Value::Array(
        arr.iter()
            .filter(|tool| {
                tool["function"]["name"]
                    .as_str()
                    .is_some_and(|n| allowed.contains(n))
            })
            .cloned()
            .collect(),
    )
}

// ────────────────────────────────────────────────────────────────────────────
// Permission policy for agents
// ────────────────────────────────────────────────────────────────────────────

/// Build a permission policy scoped to an agent's tool set. Read-only tools
/// auto-pass; dangerous tools (bash, git write ops) require `[y/N]`
/// confirmation via the `CliPrompter` — same as Claudette's main policy.
fn build_agent_permission_policy(allowed: &BTreeSet<String>) -> PermissionPolicy {
    use PermissionMode::{DangerFullAccess, ReadOnly, WorkspaceWrite};

    let mut policy = PermissionPolicy::new(WorkspaceWrite);

    // Read-only tools — auto-allowed.
    for name in [
        "get_current_time",
        "read_file",
        "list_dir",
        "glob_search",
        "grep_search",
        "git_status",
        "git_diff",
        "git_log",
        "git_branch",
    ] {
        if allowed.contains(name) {
            policy = policy.with_tool_requirement(name, ReadOnly);
        }
    }

    // Workspace-write tools — auto-allowed.
    for name in ["web_search", "web_fetch"] {
        if allowed.contains(name) {
            policy = policy.with_tool_requirement(name, WorkspaceWrite);
        }
    }

    // Dangerous tools — always prompt [y/N].
    for name in ["bash", "git_add", "git_commit", "git_push", "git_checkout"] {
        if allowed.contains(name) {
            policy = policy.with_tool_requirement(name, DangerFullAccess);
        }
    }

    policy
}

// ────────────────────────────────────────────────────────────────────────────
// Spawn orchestrator
// ────────────────────────────────────────────────────────────────────────────

/// Spawn an agent synchronously. Blocks until the agent completes all
/// iterations (or hits its max). Returns the agent's final text output.
///
/// In normal mode, dangerous tools prompt the user via `CliPrompter`.
/// In auto mode, all tools are auto-approved (user opted in).
pub fn spawn_agent(agent_type: AgentType, task: &str, auto_mode: bool) -> Result<String, String> {
    let config = agent_type.config();

    eprintln!(
        "{} {} {}",
        crate::theme::ROBOT,
        crate::theme::accent(&format!("spawning {} agent", config.agent_type)),
        crate::theme::dim(&format!(
            "(model={}, tools={}, max_iter={})",
            config.model,
            config.allowed_tools.len(),
            config.max_iterations,
        ))
    );

    // Build filtered tool JSON for the Ollama API.
    let full_tools = secretary_tools_json();
    let tools_json = filter_tools_json(&full_tools, &config.allowed_tools);

    // Build API client — no streaming callback (agent runs silently).
    let api_client = OllamaApiClient::new(config.model, tools_json).with_context(config.num_ctx);

    // Build filtered executor.
    let executor = FilteredToolExecutor::new(config.allowed_tools.clone());

    // Build permission policy.
    let policy = if auto_mode {
        PermissionPolicy::new(PermissionMode::Allow)
    } else {
        build_agent_permission_policy(&config.allowed_tools)
    };

    // Build runtime with fresh session.
    let mut runtime = ConversationRuntime::new(
        Session::default(),
        api_client,
        executor,
        policy,
        vec![config.system_prompt],
    )
    .with_max_iterations(config.max_iterations)
    .with_auto_compaction_input_tokens_threshold(u32::MAX);

    // Run the agent. Pass CliPrompter if not auto_mode.
    let summary = if auto_mode {
        runtime
            .run_turn(task, None)
            .map_err(|e| format!("{} agent failed: {e}", config.agent_type))?
    } else {
        let mut prompter = CliPrompter;
        runtime
            .run_turn(task, Some(&mut prompter))
            .map_err(|e| format!("{} agent failed: {e}", config.agent_type))?
    };

    let result = extract_final_text(&summary);

    eprintln!(
        "{} {} {}",
        crate::theme::OK_GLYPH,
        crate::theme::ok(&format!("{} agent done", config.agent_type)),
        crate::theme::dim(&format!(
            "(iter={}, in={}, out={})",
            summary.iterations, summary.usage.input_tokens, summary.usage.output_tokens,
        ))
    );

    Ok(result)
}

/// Extract the final text from the assistant's last message in a turn summary.
fn extract_final_text(summary: &TurnSummary) -> String {
    let mut texts = Vec::new();
    for msg in &summary.assistant_messages {
        for block in &msg.blocks {
            if let ContentBlock::Text { text } = block {
                if !text.trim().is_empty() {
                    texts.push(text.trim().to_string());
                }
            }
        }
    }
    if texts.is_empty() {
        "(agent produced no text output)".to_string()
    } else {
        texts.join("\n\n")
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_type_from_str_researcher() {
        assert_eq!(AgentType::parse("researcher"), Some(AgentType::Researcher));
        assert_eq!(AgentType::parse("Research"), Some(AgentType::Researcher));
        assert_eq!(AgentType::parse("RESEARCHER"), Some(AgentType::Researcher));
    }

    #[test]
    fn agent_type_from_str_gitops() {
        assert_eq!(AgentType::parse("gitops"), Some(AgentType::GitOps));
        assert_eq!(AgentType::parse("git"), Some(AgentType::GitOps));
        assert_eq!(AgentType::parse("GitOps"), Some(AgentType::GitOps));
    }

    #[test]
    fn agent_type_from_str_code_reviewer() {
        assert_eq!(AgentType::parse("reviewer"), Some(AgentType::CodeReviewer));
        assert_eq!(
            AgentType::parse("code_reviewer"),
            Some(AgentType::CodeReviewer)
        );
        assert_eq!(
            AgentType::parse("code-review"),
            Some(AgentType::CodeReviewer)
        );
        assert_eq!(AgentType::parse("review"), Some(AgentType::CodeReviewer));
    }

    #[test]
    fn agent_type_from_str_unknown() {
        assert_eq!(AgentType::parse("unknown"), None);
        assert_eq!(AgentType::parse(""), None);
        assert_eq!(AgentType::parse("codet"), None);
    }

    #[test]
    fn agent_type_display() {
        assert_eq!(AgentType::Researcher.to_string(), "researcher");
        assert_eq!(AgentType::GitOps.to_string(), "gitops");
        assert_eq!(AgentType::CodeReviewer.to_string(), "code_reviewer");
    }

    #[test]
    fn researcher_config_has_correct_tools() {
        let config = AgentType::Researcher.config();
        assert!(config.allowed_tools.contains("web_search"));
        assert!(config.allowed_tools.contains("web_fetch"));
        assert!(config.allowed_tools.contains("read_file"));
        assert!(config.allowed_tools.contains("glob_search"));
        assert!(config.allowed_tools.contains("grep_search"));
        assert!(!config.allowed_tools.contains("bash"));
        assert!(!config.allowed_tools.contains("git_add"));
        assert!(!config.allowed_tools.contains("spawn_agent"));
    }

    #[test]
    fn gitops_config_has_correct_tools() {
        let config = AgentType::GitOps.config();
        assert!(config.allowed_tools.contains("git_status"));
        assert!(config.allowed_tools.contains("git_add"));
        assert!(config.allowed_tools.contains("git_commit"));
        assert!(config.allowed_tools.contains("git_push"));
        assert!(config.allowed_tools.contains("bash"));
        assert!(config.allowed_tools.contains("read_file"));
        assert!(!config.allowed_tools.contains("web_search"));
        assert!(!config.allowed_tools.contains("spawn_agent"));
    }

    #[test]
    fn code_reviewer_config_has_correct_tools() {
        let config = AgentType::CodeReviewer.config();
        assert!(config.allowed_tools.contains("read_file"));
        assert!(config.allowed_tools.contains("glob_search"));
        assert!(config.allowed_tools.contains("grep_search"));
        // Code reviewer is read-only — no write tools.
        assert!(!config.allowed_tools.contains("bash"));
        assert!(!config.allowed_tools.contains("git_add"));
        assert!(!config.allowed_tools.contains("web_search"));
        assert!(!config.allowed_tools.contains("spawn_agent"));
    }

    #[test]
    fn code_reviewer_default_max_iter() {
        let config = AgentType::CodeReviewer.config();
        assert_eq!(config.max_iterations, DEFAULT_CODE_REVIEWER_MAX_ITER);
    }

    #[test]
    fn researcher_default_max_iter() {
        let config = AgentType::Researcher.config();
        assert_eq!(config.max_iterations, DEFAULT_RESEARCHER_MAX_ITER);
    }

    #[test]
    fn gitops_default_max_iter() {
        let config = AgentType::GitOps.config();
        assert_eq!(config.max_iterations, DEFAULT_GITOPS_MAX_ITER);
    }

    #[test]
    fn filter_tools_json_keeps_allowed_only() {
        let full = serde_json::json!([
            { "type": "function", "function": { "name": "read_file" } },
            { "type": "function", "function": { "name": "bash" } },
            { "type": "function", "function": { "name": "web_search" } },
        ]);
        let allowed: BTreeSet<String> = ["read_file", "web_search"]
            .into_iter()
            .map(String::from)
            .collect();

        let filtered = filter_tools_json(&full, &allowed);
        let arr = filtered.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        let names: Vec<&str> = arr
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"web_search"));
        assert!(!names.contains(&"bash"));
    }

    #[test]
    fn filter_tools_json_empty_allowed_returns_empty() {
        let full = serde_json::json!([
            { "type": "function", "function": { "name": "read_file" } },
        ]);
        let allowed: BTreeSet<String> = BTreeSet::new();
        let filtered = filter_tools_json(&full, &allowed);
        assert_eq!(filtered.as_array().unwrap().len(), 0);
    }

    #[test]
    fn filtered_executor_rejects_disallowed_tool() {
        let allowed: BTreeSet<String> = ["read_file"].into_iter().map(String::from).collect();
        let mut exec = FilteredToolExecutor::new(allowed);
        let result = exec.execute("bash", r#"{"command":"ls"}"#);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not available"));
    }

    #[test]
    fn filtered_executor_passes_allowed_tool() {
        let allowed: BTreeSet<String> =
            ["get_current_time"].into_iter().map(String::from).collect();
        let mut exec = FilteredToolExecutor::new(allowed);
        let result = exec.execute("get_current_time", "{}");
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("iso8601"));
    }

    #[test]
    fn auto_mode_policy_allows_everything() {
        let policy = PermissionPolicy::new(PermissionMode::Allow);
        let outcome = policy.authorize("bash", r#"{"command":"rm -rf /"}"#, None);
        assert!(matches!(outcome, crate::PermissionOutcome::Allow));
    }

    #[test]
    fn normal_mode_policy_read_only_auto_allowed() {
        let allowed = researcher_tools();
        let policy = build_agent_permission_policy(&allowed);
        let outcome = policy.authorize("read_file", r#"{"path":"~/test"}"#, None);
        assert!(matches!(outcome, crate::PermissionOutcome::Allow));
    }

    #[test]
    fn extract_final_text_from_empty_summary() {
        let summary = TurnSummary {
            assistant_messages: vec![],
            tool_results: vec![],
            iterations: 0,
            usage: crate::TokenUsage::default(),
            auto_compaction: None,
        };
        let text = extract_final_text(&summary);
        assert_eq!(text, "(agent produced no text output)");
    }
}
