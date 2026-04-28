use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
    Prompt,
    Allow,
}

impl PermissionMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
            Self::Prompt => "prompt",
            Self::Allow => "allow",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub tool_name: String,
    pub input: String,
    pub current_mode: PermissionMode,
    pub required_mode: PermissionMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionPromptDecision {
    Allow,
    Deny { reason: String },
}

pub trait PermissionPrompter {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    Allow,
    Deny { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPolicy {
    active_mode: PermissionMode,
    tool_requirements: BTreeMap<String, PermissionMode>,
}

impl PermissionPolicy {
    #[must_use]
    pub fn new(active_mode: PermissionMode) -> Self {
        Self {
            active_mode,
            tool_requirements: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_tool_requirement(
        mut self,
        tool_name: impl Into<String>,
        required_mode: PermissionMode,
    ) -> Self {
        self.tool_requirements
            .insert(tool_name.into(), required_mode);
        self
    }

    #[must_use]
    pub fn active_mode(&self) -> PermissionMode {
        self.active_mode
    }

    #[must_use]
    pub fn required_mode_for(&self, tool_name: &str) -> PermissionMode {
        self.tool_requirements
            .get(tool_name)
            .copied()
            .unwrap_or(PermissionMode::DangerFullAccess)
    }

    /// True if `tool_name` has an explicit requirement registered. Used by
    /// the conversation loop to short-circuit unknown-tool calls into a
    /// structured "did you mean?" tool_result instead of bubbling a
    /// permission prompt for a name that won't dispatch anyway.
    #[must_use]
    pub fn is_known(&self, tool_name: &str) -> bool {
        self.tool_requirements.contains_key(tool_name)
    }

    /// Up to `max` known tool names ranked by closeness to `unknown_name`.
    /// Heuristic, in order: exact substring matches first (either direction),
    /// then Levenshtein distance ≤ 3. Stable tie-break by lexicographic order
    /// so test output is deterministic.
    ///
    /// Returns an empty vec for names with no nearby matches (e.g. group
    /// names like `facts` that don't share characters with any tool). Caller
    /// is expected to layer additional hints (group-aware suggestions) on top.
    #[must_use]
    pub fn suggest_for(&self, unknown_name: &str, max: usize) -> Vec<String> {
        if max == 0 {
            return Vec::new();
        }
        let needle = unknown_name.to_lowercase();
        // The first underscore-delimited component is the strongest signal —
        // tool names are conventionally `<noun>_<verb>` (e.g. `note_create`),
        // so a confabulated `note_update` should suggest every `note_*` tool.
        let needle_prefix = needle.split('_').next().unwrap_or("").to_string();
        let mut scored: Vec<(u32, String)> = self
            .tool_requirements
            .keys()
            .filter_map(|name| {
                let lower = name.to_lowercase();
                // Score: lower is better. Bands are separated so a prefix
                // match always outranks substring, which outranks Levenshtein.
                if needle_prefix.len() >= 3 && lower.starts_with(&format!("{needle_prefix}_")) {
                    Some((1, name.clone()))
                } else if lower.contains(&needle) || needle.contains(&lower) {
                    Some((2, name.clone()))
                } else {
                    let d = levenshtein(&needle, &lower);
                    if d <= 3 {
                        Some((10 + d, name.clone()))
                    } else {
                        None
                    }
                }
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, n)| n).take(max).collect()
    }

    #[must_use]
    pub fn authorize(
        &self,
        tool_name: &str,
        input: &str,
        mut prompter: Option<&mut dyn PermissionPrompter>,
    ) -> PermissionOutcome {
        let current_mode = self.active_mode();
        let required_mode = self.required_mode_for(tool_name);
        if current_mode == PermissionMode::Allow || current_mode >= required_mode {
            return PermissionOutcome::Allow;
        }

        let request = PermissionRequest {
            tool_name: tool_name.to_string(),
            input: input.to_string(),
            current_mode,
            required_mode,
        };

        if current_mode == PermissionMode::Prompt
            || (current_mode == PermissionMode::WorkspaceWrite
                && required_mode == PermissionMode::DangerFullAccess)
        {
            return match prompter.as_mut() {
                Some(prompter) => match prompter.decide(&request) {
                    PermissionPromptDecision::Allow => PermissionOutcome::Allow,
                    PermissionPromptDecision::Deny { reason } => PermissionOutcome::Deny { reason },
                },
                None => PermissionOutcome::Deny {
                    reason: format!(
                        "tool '{tool_name}' requires approval to escalate from {} to {}",
                        current_mode.as_str(),
                        required_mode.as_str()
                    ),
                },
            };
        }

        PermissionOutcome::Deny {
            reason: format!(
                "tool '{tool_name}' requires {} permission; current mode is {}",
                required_mode.as_str(),
                current_mode.as_str()
            ),
        }
    }
}

/// Iterative Levenshtein distance, two-row variant. `O(m*n)` time, `O(min(m,n))`
/// space. Operates on chars so non-ASCII names aren't penalised by byte length.
fn levenshtein(a: &str, b: &str) -> u32 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return u32::try_from(b.len()).unwrap_or(u32::MAX);
    }
    if b.is_empty() {
        return u32::try_from(a.len()).unwrap_or(u32::MAX);
    }
    let mut prev: Vec<u32> = (0..=u32::try_from(b.len()).unwrap_or(u32::MAX)).collect();
    let mut curr: Vec<u32> = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = u32::try_from(i + 1).unwrap_or(u32::MAX);
        for (j, &cb) in b.iter().enumerate() {
            let cost = u32::from(ca != cb);
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::{
        PermissionMode, PermissionOutcome, PermissionPolicy, PermissionPromptDecision,
        PermissionPrompter, PermissionRequest,
    };

    struct RecordingPrompter {
        seen: Vec<PermissionRequest>,
        allow: bool,
    }

    impl PermissionPrompter for RecordingPrompter {
        fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
            self.seen.push(request.clone());
            if self.allow {
                PermissionPromptDecision::Allow
            } else {
                PermissionPromptDecision::Deny {
                    reason: "not now".to_string(),
                }
            }
        }
    }

    #[test]
    fn allows_tools_when_active_mode_meets_requirement() {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("read_file", PermissionMode::ReadOnly)
            .with_tool_requirement("write_file", PermissionMode::WorkspaceWrite);

        assert_eq!(
            policy.authorize("read_file", "{}", None),
            PermissionOutcome::Allow
        );
        assert_eq!(
            policy.authorize("write_file", "{}", None),
            PermissionOutcome::Allow
        );
    }

    #[test]
    fn denies_read_only_escalations_without_prompt() {
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("write_file", PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);

        assert!(matches!(
            policy.authorize("write_file", "{}", None),
            PermissionOutcome::Deny { reason } if reason.contains("requires workspace-write permission")
        ));
        assert!(matches!(
            policy.authorize("bash", "{}", None),
            PermissionOutcome::Deny { reason } if reason.contains("requires danger-full-access permission")
        ));
    }

    #[test]
    fn prompts_for_workspace_write_to_danger_full_access_escalation() {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);
        let mut prompter = RecordingPrompter {
            seen: Vec::new(),
            allow: true,
        };

        let outcome = policy.authorize("bash", "echo hi", Some(&mut prompter));

        assert_eq!(outcome, PermissionOutcome::Allow);
        assert_eq!(prompter.seen.len(), 1);
        assert_eq!(prompter.seen[0].tool_name, "bash");
        assert_eq!(
            prompter.seen[0].current_mode,
            PermissionMode::WorkspaceWrite
        );
        assert_eq!(
            prompter.seen[0].required_mode,
            PermissionMode::DangerFullAccess
        );
    }

    #[test]
    fn honors_prompt_rejection_reason() {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);
        let mut prompter = RecordingPrompter {
            seen: Vec::new(),
            allow: false,
        };

        assert!(matches!(
            policy.authorize("bash", "echo hi", Some(&mut prompter)),
            PermissionOutcome::Deny { reason } if reason == "not now"
        ));
    }

    fn standard_policy() -> PermissionPolicy {
        PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("note_create", PermissionMode::WorkspaceWrite)
            .with_tool_requirement("note_list", PermissionMode::ReadOnly)
            .with_tool_requirement("note_read", PermissionMode::ReadOnly)
            .with_tool_requirement("note_delete", PermissionMode::WorkspaceWrite)
            .with_tool_requirement("weather_current", PermissionMode::ReadOnly)
            .with_tool_requirement("git_log", PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
    }

    #[test]
    fn is_known_returns_true_for_registered_tool() {
        let policy = standard_policy();
        assert!(policy.is_known("note_create"));
        assert!(policy.is_known("bash"));
    }

    #[test]
    fn is_known_returns_false_for_unregistered_tool() {
        let policy = standard_policy();
        assert!(!policy.is_known("note_update"));
        assert!(!policy.is_known("facts"));
        assert!(!policy.is_known(""));
    }

    #[test]
    fn suggest_for_returns_close_matches_by_substring() {
        // `note_update` shares the `note_` prefix with all four note_* tools;
        // each contains `note_` as a substring, so all four should appear,
        // ordered lexicographically (stable tie-break).
        let policy = standard_policy();
        let suggestions = policy.suggest_for("note_update", 5);
        assert!(suggestions.contains(&"note_create".to_string()));
        assert!(suggestions.contains(&"note_list".to_string()));
        assert!(suggestions.contains(&"note_read".to_string()));
        assert!(suggestions.contains(&"note_delete".to_string()));
    }

    #[test]
    fn suggest_for_respects_max_cap() {
        let policy = standard_policy();
        let suggestions = policy.suggest_for("note_update", 2);
        assert_eq!(suggestions.len(), 2);
    }

    #[test]
    fn suggest_for_returns_empty_for_distant_names() {
        // `facts` shares no characters/substring with any registered tool
        // and Levenshtein distance to all of them exceeds 3. The expected
        // behavior is empty — caller layers a group-aware hinter on top.
        let policy = standard_policy();
        assert!(policy.suggest_for("facts", 5).is_empty());
    }

    #[test]
    fn suggest_for_finds_levenshtein_neighbors() {
        // Single-char typo within distance ≤ 3.
        let policy = standard_policy();
        let suggestions = policy.suggest_for("not_create", 3);
        assert!(suggestions.contains(&"note_create".to_string()));
    }

    #[test]
    fn suggest_for_zero_max_returns_empty() {
        let policy = standard_policy();
        assert!(policy.suggest_for("note_update", 0).is_empty());
    }

    #[test]
    fn levenshtein_basic_distances() {
        assert_eq!(super::levenshtein("", ""), 0);
        assert_eq!(super::levenshtein("abc", "abc"), 0);
        assert_eq!(super::levenshtein("abc", "ab"), 1);
        assert_eq!(super::levenshtein("kitten", "sitting"), 3);
        assert_eq!(super::levenshtein("", "hello"), 5);
    }
}
