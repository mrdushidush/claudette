use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use crate::compact::{
    compact_session, estimate_session_tokens, CompactionConfig, CompactionResult,
};
use crate::config::RuntimeFeatureConfig;
use crate::hooks::{HookRunResult, HookRunner};
use crate::permissions::{PermissionOutcome, PermissionPolicy, PermissionPrompter};
use crate::session::{ContentBlock, ConversationMessage, Session};
use crate::usage::{TokenUsage, UsageTracker};

const DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD: u32 = 200_000;
const AUTO_COMPACTION_THRESHOLD_ENV_VAR: &str = "CLAUDE_CODE_AUTO_COMPACT_INPUT_TOKENS";

/// Read-loop breaker knobs (plans/read-loop-breaker-2026-06-17.md). Setting
/// `CLAUDETTE_NO_READ_LOOP_BREAKER` (to anything) disables both the unchanged-
/// re-read suppression and the no-progress nudge; `CLAUDETTE_READ_LOOP_LIMIT`
/// overrides how many identical reads are allowed before suppression.
const READ_LOOP_DISABLE_ENV_VAR: &str = "CLAUDETTE_NO_READ_LOOP_BREAKER";
const READ_LOOP_LIMIT_ENV_VAR: &str = "CLAUDETTE_READ_LOOP_LIMIT";
/// Suppress from the 2nd identical re-read (1st returns the body, 2nd+ the
/// pointer-up notice).
const READ_LOOP_LIMIT_DEFAULT: usize = 2;
/// Tool calls since the last SUCCESSFUL mutation before the no-progress nudge
/// fires once. Counts a DIFFERENT spiral than [`SEARCH_NUDGE_AT`]: a
/// read↔failed-edit churn, where each failed edit resets the consecutive-nav
/// streak (so [`search_budget_nudge`] never trips) yet no file ever changes.
const NO_PROGRESS_NUDGE_AT: usize = 8;

/// One-sentence discipline reminder appended to the system prompt for any
/// turn that carries an image attachment. The 35B brain on
/// `unsloth/qwen3.6-35b-a3b` was observed citing prior-turn tool output
/// (Technical Rating 0.4/1.0, RSI 56.37, …) as if those values came from
/// the attached chart. Tracks P2 in the 2026-05-04 optimization queue.
///
/// Kept short on purpose: longer prompts suppress tool-calling on small
/// brains (qwen3.5:9b notably). Only takes effect when an image is
/// actually attached; image-less turns see the unmodified system prompt.
const VISION_DISCIPLINE_HINT: &str = "User attached an image this turn. \
    Cite features visible in the image (price level, candle pattern, \
    chart indicators, text in screenshots) before referencing tool data \
    from prior turns.";

/// How many tool-call rounds before the iteration cap the budget warning
/// starts appearing in the system prompt. Only active when the graceful
/// cap is enabled (top-level daily-driver turns, cap 40); sub-agents with
/// caps of 5-10 would see the warning almost immediately, which suppresses
/// tool-calling on small brains.
const ITERATION_NUDGE_WINDOW: usize = 5;

/// System-prompt line for the last [`ITERATION_NUDGE_WINDOW`] rounds before
/// the cap. Two dogfood sessions (2026-06-11) were hard-killed within sight
/// of the finish line — one at `git checkout -b` after the full test gate
/// had passed — with no warning that the budget was running out.
fn iteration_budget_nudge(remaining: usize) -> String {
    format!(
        "Iteration budget alert: at most {remaining} tool-call round(s) remain \
         this turn. Prioritize finishing the task now. If you cannot finish, \
         stop calling tools and reply with a summary of what is done, the \
         current state, and the exact next steps."
    )
}

/// System-prompt line for the one extra text-only request made when the cap
/// is hit (graceful landing). Tool calls in the reply are refused, never
/// executed.
const ITERATION_CAP_LANDING_PROMPT: &str = "The tool-call iteration limit for \
    this turn is exhausted. Tools are no longer available; any tool call will \
    be rejected, not executed. Reply with TEXT ONLY: state what was \
    accomplished, the current state of files/branches/commands, what remains \
    unfinished, and the exact next step for whoever continues.";

/// Tool-result body for tool calls the model emits anyway during the
/// landing request. Keeps the tool_use/tool_result protocol consistent for
/// the next turn without executing anything.
const ITERATION_CAP_TOOL_REFUSAL: &str =
    "iteration limit reached — this tool call was NOT executed";

/// Append the vision-discipline hint to `base` when this turn has an image
/// attached, otherwise return `base` unchanged. Pure function — separates
/// the policy from the conversation loop so it can be unit-tested.
fn build_turn_system_prompt(base: &[String], has_images: bool) -> Vec<String> {
    if has_images {
        let mut out = Vec::with_capacity(base.len() + 1);
        out.extend_from_slice(base);
        out.push(VISION_DISCIPLINE_HINT.to_string());
        out
    } else {
        base.to_vec()
    }
}

/// One API request to the brain. Carries the system prompt and the
/// conversation history.
///
/// `messages` is a `Cow` so the hot path in `run_turn` can borrow
/// `Session::messages` instead of cloning the full history on every loop
/// iteration — that clone was O(n) in message count and held megabyte-scale
/// image blocks. Test code and one-shot callers (e.g. `codet`) construct
/// the owned variant; the runtime constructs the borrowed variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest<'a> {
    pub system_prompt: Vec<String>,
    pub messages: Cow<'a, [ConversationMessage]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistantEvent {
    TextDelta(String),
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    Usage(TokenUsage),
    MessageStop,
}

pub trait ApiClient {
    fn stream(&mut self, request: &ApiRequest<'_>) -> Result<Vec<AssistantEvent>, RuntimeError>;
}

pub trait ToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    message: String,
}

impl ToolError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for ToolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    message: String,
}

impl RuntimeError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSummary {
    pub assistant_messages: Vec<ConversationMessage>,
    pub tool_results: Vec<ConversationMessage>,
    pub iterations: usize,
    pub usage: TokenUsage,
    pub auto_compaction: Option<AutoCompactionEvent>,
    /// True when the turn hit the iteration cap and ended with the forced
    /// text-only landing (see `with_graceful_iteration_cap`) instead of a
    /// hard error. The last assistant message is then a state-of-work
    /// summary, and the turn's work may be unfinished — callers should
    /// surface that to the user.
    pub hit_iteration_cap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoCompactionEvent {
    pub removed_message_count: usize,
}

/// Callback that maps an unknown tool name to a list of suggested real
/// tool names. See [`ConversationRuntime::with_unknown_tool_hinter`].
pub type UnknownToolHinter = Box<dyn Fn(&str) -> Vec<String>>;

pub struct ConversationRuntime<C, T> {
    session: Session,
    api_client: C,
    tool_executor: T,
    permission_policy: PermissionPolicy,
    system_prompt: Vec<String>,
    max_iterations: usize,
    graceful_iteration_cap: bool,
    usage_tracker: UsageTracker,
    hook_runner: HookRunner,
    auto_compaction_input_tokens_threshold: u32,
    /// Optional fallback for unknown-tool suggestions. Invoked only when
    /// `PermissionPolicy::suggest_for` returns no candidates — claudette
    /// wires this to map a confabulated group name (e.g. `facts`) to that
    /// group's actual tools (e.g. `weather_current`, `wikipedia_search`).
    unknown_tool_hinter: Option<UnknownToolHinter>,
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    #[must_use]
    pub fn new(
        session: Session,
        api_client: C,
        tool_executor: T,
        permission_policy: PermissionPolicy,
        system_prompt: Vec<String>,
    ) -> Self {
        Self::new_with_features(
            session,
            api_client,
            tool_executor,
            permission_policy,
            system_prompt,
            RuntimeFeatureConfig::default(),
        )
    }

    #[must_use]
    pub fn new_with_features(
        session: Session,
        api_client: C,
        tool_executor: T,
        permission_policy: PermissionPolicy,
        system_prompt: Vec<String>,
        feature_config: RuntimeFeatureConfig,
    ) -> Self {
        let usage_tracker = UsageTracker::from_session(&session);
        Self {
            session,
            api_client,
            tool_executor,
            permission_policy,
            system_prompt,
            max_iterations: usize::MAX,
            graceful_iteration_cap: false,
            usage_tracker,
            hook_runner: HookRunner::from_feature_config(&feature_config),
            auto_compaction_input_tokens_threshold: auto_compaction_threshold_from_env(),
            unknown_tool_hinter: None,
        }
    }

    #[must_use]
    pub fn with_max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations;
        self
    }

    /// Land iteration-cap turns gracefully instead of hard-failing.
    ///
    /// With this enabled, the last [`ITERATION_NUDGE_WINDOW`] rounds before
    /// the cap carry a budget warning in the system prompt, and hitting the
    /// cap makes ONE extra text-only request asking the model to summarize
    /// the state of its work — returned as a normal `Ok` summary with
    /// [`TurnSummary::hit_iteration_cap`] set — rather than returning the
    /// hard "exceeded the maximum number of iterations" error and throwing
    /// the turn away. Tool calls in that final reply are refused, never
    /// executed.
    ///
    /// Intended for top-level interactive turns (REPL/TUI, cap ~40). Leave
    /// it off for sub-agents and forge roles: their callers consume results
    /// programmatically and rely on the hard error to fail a round.
    #[must_use]
    pub fn with_graceful_iteration_cap(mut self) -> Self {
        self.graceful_iteration_cap = true;
        self
    }

    #[must_use]
    pub fn with_auto_compaction_input_tokens_threshold(mut self, threshold: u32) -> Self {
        self.auto_compaction_input_tokens_threshold = threshold;
        self
    }

    /// Register a fallback that maps an unknown tool name to a list of
    /// real tool names to suggest. Used by claudette to turn the brain's
    /// confabulated *group* names (`facts`, `markets`, `notes`) into the
    /// group's actual tools — `PermissionPolicy::suggest_for` is generic
    /// and can't know which tool group a name refers to.
    #[must_use]
    pub fn with_unknown_tool_hinter<F>(mut self, hinter: F) -> Self
    where
        F: Fn(&str) -> Vec<String> + 'static,
    {
        self.unknown_tool_hinter = Some(Box::new(hinter));
        self
    }

    pub fn run_turn(
        &mut self,
        user_input: impl Into<String>,
        prompter: Option<&mut dyn PermissionPrompter>,
    ) -> Result<TurnSummary, RuntimeError> {
        self.run_turn_with_images(user_input, Vec::new(), prompter)
    }

    /// Same as [`Self::run_turn`] but the user message also carries N image
    /// attachments (`(media_type, base64_data)` pairs). Used by the TUI's
    /// clipboard-paste / `@path` flow.
    pub fn run_turn_with_images(
        &mut self,
        user_input: impl Into<String>,
        images: Vec<(String, String)>,
        mut prompter: Option<&mut dyn PermissionPrompter>,
    ) -> Result<TurnSummary, RuntimeError> {
        let has_images = !images.is_empty();
        let user_message = if has_images {
            ConversationMessage::user_with_images(user_input.into(), images)
        } else {
            ConversationMessage::user_text(user_input.into())
        };
        self.session.messages.push(user_message);

        let mut assistant_messages = Vec::new();
        let mut tool_results = Vec::new();
        let mut iterations = 0;
        // Loop-breaker state: exact (name, args) of read-only navigation calls
        // already executed this turn. Small local brains routinely re-issue the
        // identical read_file/grep when they fail to converge, spiraling into
        // the iteration cap (and, on slow models, multi-minute hangs). A repeat
        // is suppressed with a pointer to the earlier result. Any non-navigation
        // tool (an edit, bash, git op, …) may change state, so it clears the set
        // and subsequent re-reads are allowed against the new state.
        let mut seen_nav: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Count of consecutive read-only navigation calls with no intervening
        // mutation or final answer. A small brain that has searched many times
        // in a row is usually stuck refining queries instead of committing to
        // an answer; past SEARCH_NUDGE_AT we append a "stop and answer" hint to
        // its search results (see the Allow arm below).
        let mut consecutive_nav = 0usize;
        // Loop-breaker (block edits): exact (name, args) of block-edit calls
        // already attempted this turn. An identical retry is always a spiral
        // (see is_block_edit_tool), so it is suppressed with a pointer to
        // re-read and change tactic. Kept separate from seen_nav: edits are not
        // navigation, and a suppressed edit must not perturb the nav dedup.
        let mut seen_edits: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Read-loop breaker (plans/read-loop-breaker-2026-06-17.md): per-path
        // content hash + how many times a read_file returned those exact bytes
        // this turn. Unlike `seen_nav` this is NOT cleared by a mutating tool —
        // it is keyed on CONTENT, so a FAILED edit (file unchanged) still lets a
        // re-read be suppressed (the Task-16 spiral that defeated `seen_nav`,
        // because a failed edit hits the `seen_nav.clear()` branch), while a
        // SUCCESSFUL edit changes the hash so the real content is returned.
        let read_loop_enabled = std::env::var_os(READ_LOOP_DISABLE_ENV_VAR).is_none();
        let read_loop_limit =
            read_loop_limit_from(std::env::var(READ_LOOP_LIMIT_ENV_VAR).ok().as_deref());
        let mut read_seen: std::collections::HashMap<String, (u64, usize)> =
            std::collections::HashMap::new();
        // No-progress breaker: tool calls since the last SUCCESSFUL mutation,
        // plus a once-per-turn flag for the steering line.
        let mut iters_since_mutation = 0usize;
        let mut nudged_no_progress = false;
        let mut hit_iteration_cap = false;

        loop {
            iterations += 1;
            if iterations > self.max_iterations {
                if !self.graceful_iteration_cap {
                    return Err(RuntimeError::new(
                        "conversation loop exceeded the maximum number of iterations",
                    ));
                }
                // Graceful landing (dogfood F2, 2026-06-11): two sessions in
                // a row lost their endgame to the hard kill — one at
                // `git checkout -b` after the full test gate had passed. One
                // extra text-only call turns the dead turn into a
                // state-of-work handoff the user can act on.
                let (landing, refusals) = self.iteration_cap_landing(has_images)?;
                self.session.messages.push(landing.clone());
                assistant_messages.push(landing);
                for refusal in refusals {
                    self.session.messages.push(refusal.clone());
                    tool_results.push(refusal);
                }
                hit_iteration_cap = true;
                break;
            }

            // P2 (2026-05-04): when an image is attached this turn, append a
            // discipline reminder to the system prompt. Without it, real
            // sessions on 35B brains showed the model echoing prior-turn
            // tool data (Technical Rating 0.4/1.0, RSI 56.37, …) and
            // pretending those came from the chart.
            let mut system_prompt = build_turn_system_prompt(&self.system_prompt, has_images);
            if self.graceful_iteration_cap {
                let remaining = self.max_iterations.saturating_sub(iterations) + 1;
                if remaining <= ITERATION_NUDGE_WINDOW {
                    system_prompt.push(iteration_budget_nudge(remaining));
                }
            }
            let request = ApiRequest {
                system_prompt,
                messages: Cow::Borrowed(&self.session.messages),
            };
            let events = self.api_client.stream(&request)?;
            let (assistant_message, usage) = build_assistant_message(events)?;
            if let Some(usage) = usage {
                self.usage_tracker.record(usage);
            }
            let pending_tool_uses = assistant_message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();

            self.session.messages.push(assistant_message.clone());
            assistant_messages.push(assistant_message);

            if pending_tool_uses.is_empty() {
                break;
            }

            for (tool_use_id, tool_name, input) in pending_tool_uses {
                // Short-circuit unknown tools BEFORE the permission gate.
                // The brain confabulating a tool name (e.g. calling the
                // *group* `facts` instead of the actual tool
                // `weather_current`) should get a structured "did you mean?"
                // tool_result, not a `[y/N]` prompt for a name that won't
                // dispatch anyway. The next iteration sees the suggestion
                // list and can self-correct without bothering the user.
                if !self.permission_policy.is_known(&tool_name) {
                    let mut suggestions = self.permission_policy.suggest_for(&tool_name, 5);
                    if suggestions.is_empty() {
                        if let Some(hinter) = self.unknown_tool_hinter.as_ref() {
                            suggestions = hinter(&tool_name);
                        }
                    }
                    let body = build_unknown_tool_body(&tool_name, &suggestions);
                    let result_message =
                        ConversationMessage::tool_result(tool_use_id, tool_name, body, true);
                    self.session.messages.push(result_message.clone());
                    tool_results.push(result_message);
                    continue;
                }

                // Loop-breaker (block edits): suppress an exact repeat of a
                // block edit (apply_diff/edit_file/apply_patch). The first
                // attempt runs; a byte-identical repeat this turn cannot help
                // (it failed before, or already applied), so return a "change
                // tactic" tool_result instead of re-executing. Before the nav
                // block so the suppressed edit leaves seen_nav untouched.
                if is_block_edit_tool(&tool_name) {
                    let key = format!("{tool_name}\u{1f}{input}");
                    if !seen_edits.insert(key) {
                        let body = duplicate_edit_body(&tool_name);
                        let result_message =
                            ConversationMessage::tool_result(tool_use_id, tool_name, body, true);
                        self.session.messages.push(result_message.clone());
                        tool_results.push(result_message);
                        continue;
                    }
                }

                // Loop-breaker: suppress an exact repeat of a read-only
                // navigation call (read_file/grep/glob/list/semantic_grep) —
                // its result is already in the context above. A non-navigation
                // tool may have changed state, so it resets the dedup set.
                let is_nav = is_navigation_tool(&tool_name);
                if is_nav {
                    let key = format!("{tool_name}\u{1f}{input}");
                    if !seen_nav.insert(key) {
                        let body = duplicate_call_body(&tool_name);
                        let result_message =
                            ConversationMessage::tool_result(tool_use_id, tool_name, body, true);
                        self.session.messages.push(result_message.clone());
                        tool_results.push(result_message);
                        continue;
                    }
                    consecutive_nav += 1;
                } else {
                    seen_nav.clear();
                    consecutive_nav = 0;
                }

                let permission_outcome = if let Some(prompt) = prompter.as_mut() {
                    self.permission_policy
                        .authorize(&tool_name, &input, Some(*prompt))
                } else {
                    self.permission_policy.authorize(&tool_name, &input, None)
                };

                let result_message = match permission_outcome {
                    PermissionOutcome::Allow => {
                        let pre_hook_result = self.hook_runner.run_pre_tool_use(&tool_name, &input);
                        if pre_hook_result.is_denied() {
                            let deny_message = format!("PreToolUse hook denied tool `{tool_name}`");
                            ConversationMessage::tool_result(
                                tool_use_id,
                                tool_name,
                                format_hook_message(&pre_hook_result, &deny_message),
                                true,
                            )
                        } else {
                            // Surface live tool activity to the REPL spinner
                            // (no-op unless the interactive REPL enabled it).
                            crate::status::global().on_tool_start(&tool_name);
                            let exec_result = self.tool_executor.execute(&tool_name, &input);
                            crate::status::global().on_tool_end();
                            let (mut output, mut is_error) = match exec_result {
                                Ok(output) => (output, false),
                                Err(error) => (error.to_string(), true),
                            };
                            output = merge_hook_feedback(pre_hook_result.messages(), output, false);

                            let post_hook_result = self
                                .hook_runner
                                .run_post_tool_use(&tool_name, &input, &output, is_error);
                            if post_hook_result.is_denied() {
                                is_error = true;
                            }
                            output = merge_hook_feedback(
                                post_hook_result.messages(),
                                output,
                                post_hook_result.is_denied(),
                            );

                            // Read-loop breaker (A): when read_file returns the
                            // SAME bytes as an earlier read this turn, replace
                            // the re-injected full body with a pointer-up notice
                            // — the big context saver. Keyed on content hash, so
                            // a genuine change (after a successful edit) is NOT
                            // suppressed. Post-exec by design: we need the bytes
                            // to know they are unchanged, and the cost it cuts is
                            // CONTEXT, not the (cheap) disk read.
                            if read_loop_enabled && tool_name == "read_file" && !is_error {
                                let path = read_file_path(&input);
                                let hash = content_hash(&output);
                                let reads = {
                                    let entry = read_seen.entry(path.clone()).or_insert((hash, 0));
                                    if entry.0 == hash {
                                        entry.1 += 1;
                                    } else {
                                        *entry = (hash, 1);
                                    }
                                    entry.1
                                };
                                if reads >= read_loop_limit {
                                    output = unchanged_read_notice(&path, reads);
                                    is_error = true;
                                }
                            }

                            // No-progress counter: reset on a SUCCESSFUL
                            // mutation, otherwise grow. Distinct from
                            // consecutive_nav, which a failed edit resets — so
                            // this keeps climbing through a read↔failed-edit
                            // churn that search_budget_nudge never catches.
                            if read_loop_enabled {
                                if is_mutation_tool(&tool_name) && !is_error {
                                    iters_since_mutation = 0;
                                } else {
                                    iters_since_mutation += 1;
                                }
                                // Fire once, on a clean nav result, only when a
                                // non-nav tool (a failed edit) broke the nav
                                // streak — i.e. consecutive_nav <
                                // iters_since_mutation. Pure read churn is left
                                // to search_budget_nudge so the two never double.
                                // A read suppressed by (A) above is is_error and
                                // already carries its own scroll-up/narrow/edit
                                // steer, so the `!is_error` guard skips it: in a
                                // single-file churn (A) does the steering; this
                                // nudge covers the multi-file churn (A) can't
                                // suppress.
                                if !nudged_no_progress
                                    && is_nav
                                    && !is_error
                                    && iters_since_mutation >= NO_PROGRESS_NUDGE_AT
                                    && consecutive_nav < iters_since_mutation
                                {
                                    output.push_str(&no_progress_nudge());
                                    nudged_no_progress = true;
                                }
                            }

                            // Search-budget nudge: too many consecutive
                            // searches/reads → push the brain to answer from
                            // what it already has instead of refining forever.
                            if is_nav && !is_error && consecutive_nav >= SEARCH_NUDGE_AT {
                                output.push_str(&search_budget_nudge(consecutive_nav));
                            }
                            ConversationMessage::tool_result(
                                tool_use_id,
                                tool_name,
                                output,
                                is_error,
                            )
                        }
                    }
                    PermissionOutcome::Deny { reason } => {
                        ConversationMessage::tool_result(tool_use_id, tool_name, reason, true)
                    }
                };
                self.session.messages.push(result_message.clone());
                tool_results.push(result_message);
            }
        }

        let auto_compaction = self.maybe_auto_compact();

        Ok(TurnSummary {
            assistant_messages,
            tool_results,
            iterations,
            usage: self.usage_tracker.cumulative_usage(),
            auto_compaction,
            hit_iteration_cap,
        })
    }

    /// One final text-only request after the iteration cap (graceful
    /// landing): the system prompt tells the model tools are gone and asks
    /// for a state-of-work summary. Tool calls in the reply are converted to
    /// error tool_results — never executed — so the tool_use/tool_result
    /// protocol stays consistent for the next turn. If the landing request
    /// itself fails, the classic hard-cap error is returned so callers see
    /// the same failure they did before graceful landing existed.
    fn iteration_cap_landing(
        &mut self,
        has_images: bool,
    ) -> Result<(ConversationMessage, Vec<ConversationMessage>), RuntimeError> {
        let hard_cap_error =
            || RuntimeError::new("conversation loop exceeded the maximum number of iterations");

        let mut system_prompt = build_turn_system_prompt(&self.system_prompt, has_images);
        system_prompt.push(ITERATION_CAP_LANDING_PROMPT.to_string());
        let request = ApiRequest {
            system_prompt,
            messages: Cow::Borrowed(&self.session.messages),
        };
        let events = self
            .api_client
            .stream(&request)
            .map_err(|_| hard_cap_error())?;
        let (assistant_message, usage) =
            build_assistant_message(events).map_err(|_| hard_cap_error())?;
        if let Some(usage) = usage {
            self.usage_tracker.record(usage);
        }

        let refusals = assistant_message
            .blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, name, .. } => Some(ConversationMessage::tool_result(
                    id.clone(),
                    name.clone(),
                    ITERATION_CAP_TOOL_REFUSAL.to_string(),
                    true,
                )),
                _ => None,
            })
            .collect();

        Ok((assistant_message, refusals))
    }

    #[must_use]
    pub fn compact(&self, config: CompactionConfig) -> CompactionResult {
        compact_session(&self.session, config)
    }

    #[must_use]
    pub fn estimated_tokens(&self) -> usize {
        estimate_session_tokens(&self.session)
    }

    #[must_use]
    pub fn usage(&self) -> &UsageTracker {
        &self.usage_tracker
    }

    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    #[must_use]
    pub fn into_session(self) -> Session {
        self.session
    }

    fn maybe_auto_compact(&mut self) -> Option<AutoCompactionEvent> {
        if self.usage_tracker.cumulative_usage().input_tokens
            < self.auto_compaction_input_tokens_threshold
        {
            return None;
        }

        let result = compact_session(
            &self.session,
            CompactionConfig {
                max_estimated_tokens: 0,
                ..CompactionConfig::default()
            },
        );

        if result.removed_message_count == 0 {
            return None;
        }

        self.session = result.compacted_session;
        Some(AutoCompactionEvent {
            removed_message_count: result.removed_message_count,
        })
    }
}

#[must_use]
pub fn auto_compaction_threshold_from_env() -> u32 {
    parse_auto_compaction_threshold(
        std::env::var(AUTO_COMPACTION_THRESHOLD_ENV_VAR)
            .ok()
            .as_deref(),
    )
}

#[must_use]
fn parse_auto_compaction_threshold(value: Option<&str>) -> u32 {
    value
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .filter(|threshold| *threshold > 0)
        .unwrap_or(DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD)
}

fn build_assistant_message(
    events: Vec<AssistantEvent>,
) -> Result<(ConversationMessage, Option<TokenUsage>), RuntimeError> {
    let mut text = String::new();
    let mut blocks = Vec::new();
    let mut finished = false;
    let mut usage = None;

    for event in events {
        match event {
            AssistantEvent::TextDelta(delta) => text.push_str(&delta),
            AssistantEvent::ToolUse { id, name, input } => {
                flush_text_block(&mut text, &mut blocks);
                blocks.push(ContentBlock::ToolUse { id, name, input });
            }
            AssistantEvent::Usage(value) => usage = Some(value),
            AssistantEvent::MessageStop => {
                finished = true;
            }
        }
    }

    flush_text_block(&mut text, &mut blocks);

    if !finished {
        return Err(RuntimeError::new(
            "assistant stream ended without a message stop event",
        ));
    }
    if blocks.is_empty() {
        return Err(RuntimeError::new("assistant stream produced no content"));
    }

    Ok((
        ConversationMessage::assistant_with_usage(blocks, usage),
        usage,
    ))
}

fn flush_text_block(text: &mut String, blocks: &mut Vec<ContentBlock>) {
    if !text.is_empty() {
        blocks.push(ContentBlock::Text {
            text: std::mem::take(text),
        });
    }
}

/// Build the JSON tool_result body returned to the brain when it calls a
/// tool that isn't registered. The structure (`error` + `did_you_mean` +
/// `hint`) is intentionally machine-readable so the next iteration can
/// pluck the right name out without natural-language parsing.
fn build_unknown_tool_body(tool_name: &str, suggestions: &[String]) -> String {
    let suggestions_json = if suggestions.is_empty() {
        "[]".to_string()
    } else {
        let quoted: Vec<String> = suggestions
            .iter()
            .map(|s| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
            .collect();
        format!("[{}]", quoted.join(","))
    };
    let escaped_name = tool_name.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "{{\"error\":\"unknown tool: {escaped_name}\",\"did_you_mean\":{suggestions_json},\"hint\":\"Use one of the listed tools, or call enable_tools to activate a tool group.\"}}"
    )
}

/// After this many consecutive read-only navigation calls with no answer, the
/// loop starts appending [`search_budget_nudge`] to each search result. Tuned
/// for a small local brain (qwen3.6-35b q3) that over-searches a large repo:
/// high enough to allow genuine multi-step work (repo_map + a few greps + a
/// read, or assembling a list) before nudging, low enough to break the "refine
/// the query forever" spiral before it burns the iteration budget.
const SEARCH_NUDGE_AT: usize = 9;

/// Hint appended to a search/read result once the consecutive-navigation count
/// crosses [`SEARCH_NUDGE_AT`]. Pushes the brain to commit — while still
/// allowing exhaustive enumeration tasks to finish gathering first.
fn search_budget_nudge(n: usize) -> String {
    format!(
        "\n\n[search-budget: {n} consecutive searches/reads this turn. You very \
         likely have what you need above. If you can answer, do so now (cite \
         file:line). If you are assembling a COMPLETE list, run ONE broad search \
         (e.g. grep the shared prefix) and read ALL its matches at once instead \
         of many narrow searches.]"
    )
}

/// Read-only "navigation" tools whose output bloats context and whose exact
/// repeat within a turn is (almost) always a non-converging spiral rather than
/// useful work. These are the calls the loop-breaker dedups; a repeat returns
/// [`duplicate_call_body`] instead of re-running the tool. Mutating tools are
/// deliberately excluded — re-reading after an edit is legitimate.
fn is_navigation_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file" | "grep_search" | "glob_search" | "list_dir" | "semantic_grep" | "repo_map"
    )
}

/// Tool result returned when a navigation call is an exact duplicate of one
/// already executed this turn. Phrased to push the brain off the spiral:
/// answer from what it already has, or change tactic.
fn duplicate_call_body(tool_name: &str) -> String {
    format!(
        "{{\"error\":\"duplicate call suppressed\",\"tool\":\"{tool_name}\",\"hint\":\"You \
         already ran this exact call earlier in this turn — its result is in the conversation \
         above. Re-reading it changes nothing. Either answer now from what you have, or take a \
         DIFFERENT action: grep_search for the specific symbol, or read_file with an offset/limit \
         to page to a new region.\"}}"
    )
}

/// Block-edit tools whose *exact repeat* within a turn is a spiral, never
/// useful work. Unlike a re-read (handled by [`is_navigation_tool`]), an
/// identical block edit re-issued in the same turn can only do one of two
/// things: the first attempt failed, so a byte-identical retry fails the same
/// way (the dogfood 2026-06-13 doubled-backslash no-op loop); or the first
/// succeeded, so the target block is gone and the retry can't match. Either
/// way it changes nothing. `write_file` is excluded — a whole-file overwrite
/// is idempotent, not a find-and-replace, and isn't part of the spiral.
fn is_block_edit_tool(name: &str) -> bool {
    matches!(name, "apply_diff" | "edit_file" | "apply_patch")
}

/// Tool result for a block-edit call that exactly repeats one already
/// attempted this turn. Pushes the brain off the retry spiral toward
/// re-reading the file and changing tactic.
fn duplicate_edit_body(tool_name: &str) -> String {
    format!(
        "{{\"error\":\"duplicate edit suppressed\",\"tool\":\"{tool_name}\",\"hint\":\"You \
         already attempted this EXACT edit earlier in this turn. A byte-identical retry \
         changes nothing — the first attempt either failed (it will fail the same way) or \
         already applied (the target is gone). Re-read the file with read_file to see its \
         CURRENT contents, then send a DIFFERENT edit that matches what is actually there.\"}}"
    )
}

/// Parse [`READ_LOOP_LIMIT_ENV_VAR`]; falls back to [`READ_LOOP_LIMIT_DEFAULT`]
/// for an absent / unparseable / out-of-range value. The floor is 2, not 1:
/// suppression triggers at `reads >= limit`, and the FIRST read must always
/// return the real body, so a limit of 0 or 1 (which would suppress — and lie
/// "unchanged since you read it earlier" about — the first read) is rejected
/// back to the default. Pure so it can be unit-tested without touching the
/// (process-global, parallel-test-shared) environment.
fn read_loop_limit_from(value: Option<&str>) -> usize {
    value
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|n| *n >= 2)
        .unwrap_or(READ_LOOP_LIMIT_DEFAULT)
}

/// Mutating tools whose SUCCESS counts as progress and resets the no-progress
/// counter. `write_file` is included here (unlike [`is_block_edit_tool`]) — a
/// successful whole-file write is real progress, even though its idempotent
/// *repeat* is not a spiral worth suppressing.
fn is_mutation_tool(name: &str) -> bool {
    matches!(
        name,
        "edit_file" | "apply_diff" | "apply_patch" | "write_file"
    )
}

/// Stable-within-a-turn hash of a `read_file` body. Only ever compared against
/// another hash from the same turn to tell "same bytes" from "changed" — not
/// persisted, so the default hasher is fine.
fn content_hash(body: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut hasher);
    hasher.finish()
}

/// Best-effort `path` pulled from a `read_file` tool input, for the unchanged-
/// re-read notice. Falls back to the raw input when the JSON has no `path`.
fn read_file_path(input: &str) -> String {
    serde_json::from_str::<serde_json::Value>(input)
        .ok()
        .and_then(|v| {
            v.get("path")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| input.to_string())
}

/// Notice returned in place of a `read_file` body when the file is UNCHANGED
/// since an earlier read this turn (identical content hash). Points UP to the
/// copy already in context. It must NEVER invite a re-fetch — the #61
/// re-fetch-loop trap came from a "call it again" placeholder — so it says
/// scroll up and narrow, never "read it again".
fn unchanged_read_notice(path: &str, reads: usize) -> String {
    serde_json::json!({
        "error": "unchanged re-read suppressed",
        "path": path,
        "reads": reads,
        "hint": "This file is UNCHANGED since you read it earlier this turn — its full \
                 contents are already in the conversation above; scroll up to them. Do NOT \
                 read it again. To act on a specific part, narrow with grep_search or \
                 read_file with an offset/limit, then send your edit.",
    })
    .to_string()
}

/// One-shot steering line for the read↔failed-edit churn ([`NO_PROGRESS_NUDGE_AT`]).
/// Appended to a clean navigation result; pushes the brain to commit an edit or
/// stop, rather than keep re-reading around an edit that never lands.
fn no_progress_nudge() -> String {
    "\n\n[no-progress: several reads/searches and no file has actually changed. If you are \
     locating the change, narrow with grep_search; if an edit keeps failing, re-read ONLY the \
     failing region (read_file offset/limit) and send a DIFFERENT edit that matches what is \
     there. Make the edit now, or stop and summarize what you found.]"
        .to_string()
}

fn format_hook_message(result: &HookRunResult, fallback: &str) -> String {
    if result.messages().is_empty() {
        fallback.to_string()
    } else {
        result.messages().join("\n")
    }
}

fn merge_hook_feedback(messages: &[String], output: String, denied: bool) -> String {
    if messages.is_empty() {
        return output;
    }

    let mut sections = Vec::new();
    if !output.trim().is_empty() {
        sections.push(output);
    }
    let label = if denied {
        "Hook feedback (denied)"
    } else {
        "Hook feedback"
    };
    sections.push(format!("{label}:\n{}", messages.join("\n")));
    sections.join("\n\n")
}

type ToolHandler = Box<dyn FnMut(&str) -> Result<String, ToolError>>;

#[derive(Default)]
pub struct StaticToolExecutor {
    handlers: BTreeMap<String, ToolHandler>,
}

impl StaticToolExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn register(
        mut self,
        tool_name: impl Into<String>,
        handler: impl FnMut(&str) -> Result<String, ToolError> + 'static,
    ) -> Self {
        self.handlers.insert(tool_name.into(), Box::new(handler));
        self
    }
}

impl ToolExecutor for StaticToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        self.handlers
            .get_mut(tool_name)
            .ok_or_else(|| ToolError::new(format!("unknown tool: {tool_name}")))?(input)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_turn_system_prompt, parse_auto_compaction_threshold, ApiClient, ApiRequest,
        AssistantEvent, AutoCompactionEvent, ConversationRuntime, RuntimeError, StaticToolExecutor,
        DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD, VISION_DISCIPLINE_HINT,
    };
    use crate::compact::CompactionConfig;
    use crate::config::{RuntimeFeatureConfig, RuntimeHookConfig};
    use crate::permissions::{
        PermissionMode, PermissionPolicy, PermissionPromptDecision, PermissionPrompter,
        PermissionRequest,
    };
    use crate::prompt_runtime::{ProjectContext, SystemPromptBuilder};
    use crate::session::{ContentBlock, MessageRole, Session};
    use crate::usage::TokenUsage;
    use std::path::PathBuf;

    struct ScriptedApiClient {
        call_count: usize,
    }

    impl ApiClient for ScriptedApiClient {
        fn stream(
            &mut self,
            request: &ApiRequest<'_>,
        ) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.call_count += 1;
            match self.call_count {
                1 => {
                    assert!(request
                        .messages
                        .iter()
                        .any(|message| message.role == MessageRole::User));
                    Ok(vec![
                        AssistantEvent::TextDelta("Let me calculate that.".to_string()),
                        AssistantEvent::ToolUse {
                            id: "tool-1".to_string(),
                            name: "add".to_string(),
                            input: "2,2".to_string(),
                        },
                        AssistantEvent::Usage(TokenUsage {
                            input_tokens: 20,
                            output_tokens: 6,
                            cache_creation_input_tokens: 1,
                            cache_read_input_tokens: 2,
                        }),
                        AssistantEvent::MessageStop,
                    ])
                }
                2 => {
                    let last_message = request
                        .messages
                        .last()
                        .expect("tool result should be present");
                    assert_eq!(last_message.role, MessageRole::Tool);
                    Ok(vec![
                        AssistantEvent::TextDelta("The answer is 4.".to_string()),
                        AssistantEvent::Usage(TokenUsage {
                            input_tokens: 24,
                            output_tokens: 4,
                            cache_creation_input_tokens: 1,
                            cache_read_input_tokens: 3,
                        }),
                        AssistantEvent::MessageStop,
                    ])
                }
                _ => Err(RuntimeError::new("unexpected extra API call")),
            }
        }
    }

    struct PromptAllowOnce;

    impl PermissionPrompter for PromptAllowOnce {
        fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
            assert_eq!(request.tool_name, "add");
            PermissionPromptDecision::Allow
        }
    }

    #[test]
    fn runs_user_to_tool_to_result_loop_end_to_end_and_tracks_usage() {
        let api_client = ScriptedApiClient { call_count: 0 };
        let tool_executor = StaticToolExecutor::new().register("add", |input| {
            let total = input
                .split(',')
                .map(|part| part.parse::<i32>().expect("input must be valid integer"))
                .sum::<i32>();
            Ok(total.to_string())
        });
        let permission_policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("add", PermissionMode::DangerFullAccess);
        let system_prompt = SystemPromptBuilder::new()
            .with_project_context(ProjectContext {
                cwd: PathBuf::from("/tmp/project"),
                current_date: "2026-03-31".to_string(),
                git_status: None,
                git_diff: None,
                instruction_files: Vec::new(),
            })
            .with_os("linux", "6.8")
            .build();
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            api_client,
            tool_executor,
            permission_policy,
            system_prompt,
        );

        let summary = runtime
            .run_turn("what is 2 + 2?", Some(&mut PromptAllowOnce))
            .expect("conversation loop should succeed");

        assert_eq!(summary.iterations, 2);
        assert_eq!(summary.assistant_messages.len(), 2);
        assert_eq!(summary.tool_results.len(), 1);
        assert_eq!(runtime.session().messages.len(), 4);
        assert_eq!(summary.usage.output_tokens, 10);
        assert_eq!(summary.auto_compaction, None);
        assert!(matches!(
            runtime.session().messages[1].blocks[1],
            ContentBlock::ToolUse { .. }
        ));
        assert!(matches!(
            runtime.session().messages[2].blocks[0],
            ContentBlock::ToolResult {
                is_error: false,
                ..
            }
        ));
    }

    /// Api client that calls the `step` tool forever — until it sees the
    /// graceful-landing system prompt, at which point it returns the
    /// scripted landing events. Drives every iteration-cap test below.
    struct CapSpiralApi {
        calls: usize,
        /// Events to return for the landing request.
        landing: Vec<AssistantEvent>,
        /// Iteration numbers (1-based) whose request carried the budget nudge.
        nudged_calls: Vec<usize>,
    }

    impl CapSpiralApi {
        fn new(landing: Vec<AssistantEvent>) -> Self {
            Self {
                calls: 0,
                landing,
                nudged_calls: Vec::new(),
            }
        }
    }

    impl ApiClient for CapSpiralApi {
        fn stream(
            &mut self,
            request: &ApiRequest<'_>,
        ) -> Result<Vec<AssistantEvent>, RuntimeError> {
            if request
                .system_prompt
                .iter()
                .any(|s| s.contains("iteration limit for"))
            {
                return Ok(self.landing.clone());
            }
            self.calls += 1;
            if request
                .system_prompt
                .iter()
                .any(|s| s.contains("Iteration budget alert"))
            {
                self.nudged_calls.push(self.calls);
            }
            Ok(vec![
                AssistantEvent::ToolUse {
                    id: format!("t{}", self.calls),
                    name: "step".to_string(),
                    input: format!("{{\"n\":{}}}", self.calls),
                },
                AssistantEvent::MessageStop,
            ])
        }
    }

    fn cap_test_runtime(
        api: CapSpiralApi,
        max_iterations: usize,
    ) -> (
        ConversationRuntime<CapSpiralApi, StaticToolExecutor>,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let executed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = executed.clone();
        let tool_executor = StaticToolExecutor::new().register("step", move |_| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("ok".to_string())
        });
        let permission_policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("step", PermissionMode::ReadOnly);
        let runtime = ConversationRuntime::new(
            Session::new(),
            api,
            tool_executor,
            permission_policy,
            vec!["test system prompt".to_string()],
        )
        .with_max_iterations(max_iterations);
        (runtime, executed)
    }

    #[test]
    fn iteration_cap_without_graceful_flag_hard_fails() {
        let api = CapSpiralApi::new(vec![AssistantEvent::MessageStop]);
        let (mut runtime, _executed) = cap_test_runtime(api, 2);
        let err = runtime
            .run_turn("spiral", None)
            .expect_err("cap must hard-fail without the graceful flag");
        assert!(
            err.to_string().contains("maximum number of iterations"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn graceful_cap_lands_with_text_summary() {
        let api = CapSpiralApi::new(vec![
            AssistantEvent::TextDelta("state-of-work: branch ready, push remains".to_string()),
            AssistantEvent::MessageStop,
        ]);
        let (mut runtime, executed) = cap_test_runtime(api, 2);
        runtime = runtime.with_graceful_iteration_cap();

        let summary = runtime
            .run_turn("spiral", None)
            .expect("graceful cap must return Ok");

        assert!(summary.hit_iteration_cap);
        // Two normal rounds executed the tool; the landing did not.
        assert_eq!(executed.load(std::sync::atomic::Ordering::SeqCst), 2);
        let last_text = summary
            .assistant_messages
            .last()
            .and_then(|m| {
                m.blocks.iter().find_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
            })
            .expect("landing message must carry text");
        assert!(last_text.contains("state-of-work"));
    }

    #[test]
    fn graceful_cap_refuses_landing_tool_calls() {
        // The model ignores the text-only instruction and calls a tool in
        // the landing reply — it must be refused, never executed.
        let api = CapSpiralApi::new(vec![
            AssistantEvent::ToolUse {
                id: "landing-tool".to_string(),
                name: "step".to_string(),
                input: "{}".to_string(),
            },
            AssistantEvent::MessageStop,
        ]);
        let (mut runtime, executed) = cap_test_runtime(api, 2);
        runtime = runtime.with_graceful_iteration_cap();

        let summary = runtime
            .run_turn("spiral", None)
            .expect("graceful cap must return Ok");

        assert!(summary.hit_iteration_cap);
        assert_eq!(
            executed.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "the landing tool call must NOT execute"
        );
        let refusal = summary
            .tool_results
            .last()
            .expect("landing tool call must get a tool_result");
        assert!(matches!(
            &refusal.blocks[0],
            ContentBlock::ToolResult { is_error: true, output, .. }
                if output.contains("NOT executed")
        ));
    }

    #[test]
    fn budget_nudge_fires_only_in_final_window() {
        // max=8, window=5 → remaining = 8-i+1: iterations 1-3 are clean,
        // 4-8 carry the nudge. The turn ends at the cap (graceful landing),
        // so all 8 normal calls happen.
        let api = CapSpiralApi::new(vec![
            AssistantEvent::TextDelta("summary".to_string()),
            AssistantEvent::MessageStop,
        ]);
        let (mut runtime, _executed) = cap_test_runtime(api, 8);
        runtime = runtime.with_graceful_iteration_cap();

        let summary = runtime.run_turn("spiral", None).expect("turn should land");
        assert!(summary.hit_iteration_cap);
        assert_eq!(runtime.api_client.nudged_calls, vec![4, 5, 6, 7, 8]);
    }

    #[test]
    fn budget_nudge_absent_without_graceful_flag() {
        let api = CapSpiralApi::new(vec![AssistantEvent::MessageStop]);
        let (mut runtime, _executed) = cap_test_runtime(api, 8);
        let _ = runtime.run_turn("spiral", None);
        assert!(runtime.api_client.nudged_calls.is_empty());
    }

    #[test]
    fn navigation_tool_classifier_matches_readers_only() {
        for nav in [
            "read_file",
            "grep_search",
            "glob_search",
            "list_dir",
            "semantic_grep",
        ] {
            assert!(super::is_navigation_tool(nav), "{nav} should be navigation");
        }
        for other in [
            "edit_file",
            "apply_diff",
            "bash",
            "git_commit",
            "write_file",
            "add",
        ] {
            assert!(
                !super::is_navigation_tool(other),
                "{other} must NOT be navigation (re-running it can be legitimate)"
            );
        }
    }

    #[test]
    fn duplicate_navigation_call_is_suppressed_not_re_executed() {
        use std::cell::Cell;
        use std::rc::Rc;

        // The brain re-issues the IDENTICAL read_file twice (the observed q3
        // spiral), then answers. The loop must execute it once, suppress the
        // exact repeat with a "duplicate" tool_result, and still finish.
        struct SpiralApiClient {
            calls: usize,
        }
        impl ApiClient for SpiralApiClient {
            fn stream(
                &mut self,
                _request: &ApiRequest<'_>,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                self.calls += 1;
                match self.calls {
                    1 | 2 => Ok(vec![
                        AssistantEvent::ToolUse {
                            id: format!("rf-{}", self.calls),
                            name: "read_file".to_string(),
                            input: "{\"path\":\"x.rs\"}".to_string(),
                        },
                        AssistantEvent::MessageStop,
                    ]),
                    _ => Ok(vec![
                        AssistantEvent::TextDelta("done".to_string()),
                        AssistantEvent::MessageStop,
                    ]),
                }
            }
        }

        let exec_count = Rc::new(Cell::new(0usize));
        let ec = Rc::clone(&exec_count);
        let tool_executor = StaticToolExecutor::new().register("read_file", move |_input| {
            ec.set(ec.get() + 1);
            Ok("FILE BODY".to_string())
        });
        let permission_policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("read_file", PermissionMode::ReadOnly);

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            SpiralApiClient { calls: 0 },
            tool_executor,
            permission_policy,
            vec!["sys".to_string()],
        );

        let summary = runtime
            .run_turn("where is x configured?", None)
            .expect("loop should finish");

        assert_eq!(
            exec_count.get(),
            1,
            "read_file must execute once; the identical repeat is suppressed"
        );
        assert_eq!(summary.iterations, 3);
        // Second tool_result is the suppressed duplicate (is_error = true).
        let dup = runtime
            .session()
            .messages
            .iter()
            .find_map(|m| {
                m.blocks.iter().find_map(|b| match b {
                    ContentBlock::ToolResult {
                        output, is_error, ..
                    } if *is_error => Some(output.clone()),
                    _ => None,
                })
            })
            .expect("a suppressed duplicate tool_result should exist");
        assert!(dup.contains("duplicate call suppressed"), "got: {dup}");
    }

    #[test]
    fn duplicate_block_edit_call_is_suppressed_not_re_executed() {
        use std::cell::Cell;
        use std::rc::Rc;

        // The brain re-issues the IDENTICAL apply_diff twice (the dogfood
        // 2026-06-13 no-op spiral), then answers. The loop must execute it
        // once, suppress the exact repeat with a "duplicate edit" tool_result,
        // and still finish.
        struct EditSpiralApi {
            calls: usize,
        }
        impl ApiClient for EditSpiralApi {
            fn stream(
                &mut self,
                _request: &ApiRequest<'_>,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                self.calls += 1;
                match self.calls {
                    1 | 2 => Ok(vec![
                        AssistantEvent::ToolUse {
                            id: format!("ad-{}", self.calls),
                            name: "apply_diff".to_string(),
                            input: "{\"path\":\"x.rs\",\"before\":\"a\",\"after\":\"b\"}"
                                .to_string(),
                        },
                        AssistantEvent::MessageStop,
                    ]),
                    _ => Ok(vec![
                        AssistantEvent::TextDelta("done".to_string()),
                        AssistantEvent::MessageStop,
                    ]),
                }
            }
        }

        let exec_count = Rc::new(Cell::new(0usize));
        let ec = Rc::clone(&exec_count);
        let tool_executor = StaticToolExecutor::new().register("apply_diff", move |_input| {
            ec.set(ec.get() + 1);
            Ok("{\"ok\":true}".to_string())
        });
        let permission_policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("apply_diff", PermissionMode::WorkspaceWrite);

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            EditSpiralApi { calls: 0 },
            tool_executor,
            permission_policy,
            vec!["sys".to_string()],
        );

        let summary = runtime
            .run_turn("fix the regex", None)
            .expect("loop should finish");

        assert_eq!(
            exec_count.get(),
            1,
            "apply_diff must execute once; the identical repeat is suppressed"
        );
        assert_eq!(summary.iterations, 3);
        let dup = runtime
            .session()
            .messages
            .iter()
            .find_map(|m| {
                m.blocks.iter().find_map(|b| match b {
                    ContentBlock::ToolResult {
                        output, is_error, ..
                    } if *is_error => Some(output.clone()),
                    _ => None,
                })
            })
            .expect("a suppressed duplicate tool_result should exist");
        assert!(dup.contains("duplicate edit suppressed"), "got: {dup}");
    }

    #[test]
    fn block_edit_tool_classifier_matches_edits_only() {
        for edit in ["apply_diff", "edit_file", "apply_patch"] {
            assert!(
                super::is_block_edit_tool(edit),
                "{edit} should be a block edit"
            );
        }
        for other in [
            "read_file",
            "grep_search",
            "bash",
            "write_file",
            "git_commit",
        ] {
            assert!(
                !super::is_block_edit_tool(other),
                "{other} must NOT be a block edit"
            );
        }
    }

    #[test]
    fn read_loop_limit_parses_env_or_falls_back() {
        assert_eq!(super::read_loop_limit_from(None), 2, "absent -> default");
        assert_eq!(super::read_loop_limit_from(Some("5")), 5);
        assert_eq!(super::read_loop_limit_from(Some(" 3 ")), 3, "trimmed");
        // Floor is 2: a limit of 0 or 1 would suppress the FIRST read (a lie),
        // so both are rejected back to the default.
        assert_eq!(
            super::read_loop_limit_from(Some("1")),
            2,
            "1 -> floor default"
        );
        assert_eq!(super::read_loop_limit_from(Some("0")), 2, "zero -> default");
        assert_eq!(
            super::read_loop_limit_from(Some("x")),
            2,
            "garbage -> default"
        );
        assert_eq!(super::read_loop_limit_from(Some("")), 2, "empty -> default");
    }

    #[test]
    fn mutation_tool_classifier_matches_writes_and_edits() {
        for m in ["edit_file", "apply_diff", "apply_patch", "write_file"] {
            assert!(super::is_mutation_tool(m), "{m} should be a mutation");
        }
        for other in ["read_file", "grep_search", "repo_map", "bash", "git_status"] {
            assert!(
                !super::is_mutation_tool(other),
                "{other} must NOT be a mutation"
            );
        }
    }

    // A scripted client that emits a fixed batch of events per call, then
    // falls through to a one-shot text answer. Used by the read-loop tests to
    // drive an arbitrary read/edit sequence.
    struct ScriptApi {
        batches: Vec<Vec<AssistantEvent>>,
        i: usize,
    }
    impl ApiClient for ScriptApi {
        fn stream(&mut self, _r: &ApiRequest<'_>) -> Result<Vec<AssistantEvent>, RuntimeError> {
            let batch = self.batches.get(self.i).cloned().unwrap_or_else(|| {
                vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ]
            });
            self.i += 1;
            Ok(batch)
        }
    }
    fn tool_use(id: &str, name: &str, input: &str) -> Vec<AssistantEvent> {
        vec![
            AssistantEvent::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: input.to_string(),
            },
            AssistantEvent::MessageStop,
        ]
    }
    fn rw_policy() -> PermissionPolicy {
        PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("read_file", PermissionMode::ReadOnly)
            .with_tool_requirement("edit_file", PermissionMode::WorkspaceWrite)
    }
    fn tool_result_outputs(
        runtime: &ConversationRuntime<ScriptApi, StaticToolExecutor>,
    ) -> Vec<String> {
        runtime
            .session()
            .messages
            .iter()
            .flat_map(|m| m.blocks.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolResult { output, .. } => Some(output.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn unchanged_re_read_is_suppressed_even_after_a_failed_edit() {
        use std::cell::Cell;
        use std::rc::Rc;

        // The Task-16 spiral: read repomap.rs -> attempt an edit that FAILS ->
        // re-read the SAME (unchanged) file. The failed edit hits the
        // `seen_nav.clear()` branch, so the existing nav dedup never catches the
        // re-read. The content-hash read-loop breaker must: let the read run
        // (it is post-exec), but replace the re-injected body with a pointer-up
        // notice because the bytes are unchanged.
        let reads = Rc::new(Cell::new(0usize));
        let rc = Rc::clone(&reads);
        let tool_executor = StaticToolExecutor::new()
            .register("read_file", move |_input| {
                rc.set(rc.get() + 1);
                Ok("FILE BODY".to_string())
            })
            .register("edit_file", |_input| {
                Err(super::ToolError::new("no match for before-text"))
            });

        let batches = vec![
            tool_use("r1", "read_file", "{\"path\":\"x.rs\"}"),
            tool_use(
                "e1",
                "edit_file",
                "{\"path\":\"x.rs\",\"before\":\"a\",\"after\":\"b\"}",
            ),
            tool_use("r2", "read_file", "{\"path\":\"x.rs\"}"),
        ];
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            ScriptApi { batches, i: 0 },
            tool_executor,
            rw_policy(),
            vec!["sys".to_string()],
        );

        runtime.run_turn("edit x.rs", None).expect("loop finishes");

        assert_eq!(
            reads.get(),
            2,
            "both reads execute (suppression is post-exec)"
        );
        let outputs = tool_result_outputs(&runtime);
        let read_results: Vec<&String> = outputs
            .iter()
            .filter(|o| o.contains("FILE BODY") || o.contains("unchanged re-read"))
            .collect();
        assert_eq!(read_results.len(), 2, "two read results: {outputs:?}");
        assert_eq!(read_results[0], "FILE BODY", "1st read returns the body");
        assert!(
            read_results[1].contains("unchanged re-read suppressed"),
            "2nd read suppressed: {}",
            read_results[1]
        );
        assert!(
            !read_results[1].contains("FILE BODY"),
            "the suppressed re-read must NOT re-inject the body"
        );
    }

    #[test]
    fn changed_file_between_reads_returns_real_content() {
        use std::cell::Cell;
        use std::rc::Rc;

        // read x.rs -> edit (clears seen_nav) -> read x.rs again, but the file's
        // bytes are now DIFFERENT. The hash differs, so the second read must
        // return the real (new) content, never a suppression notice.
        let n = Rc::new(Cell::new(0usize));
        let nc = Rc::clone(&n);
        let tool_executor = StaticToolExecutor::new()
            .register("read_file", move |_input| {
                nc.set(nc.get() + 1);
                Ok(format!("VERSION {}", nc.get()))
            })
            .register("edit_file", |_input| Ok("{\"ok\":true}".to_string()));

        let batches = vec![
            tool_use("r1", "read_file", "{\"path\":\"x.rs\"}"),
            tool_use(
                "e1",
                "edit_file",
                "{\"path\":\"x.rs\",\"before\":\"a\",\"after\":\"b\"}",
            ),
            tool_use("r2", "read_file", "{\"path\":\"x.rs\"}"),
        ];
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            ScriptApi { batches, i: 0 },
            tool_executor,
            rw_policy(),
            vec!["sys".to_string()],
        );

        runtime
            .run_turn("edit then re-check", None)
            .expect("finishes");

        let outputs = tool_result_outputs(&runtime);
        assert!(
            outputs.iter().any(|o| o == "VERSION 2"),
            "2nd read returns real changed content: {outputs:?}"
        );
        assert!(
            !outputs.iter().any(|o| o.contains("unchanged re-read")),
            "changed content must NOT be suppressed: {outputs:?}"
        );
    }

    #[test]
    fn distinct_files_read_once_are_not_suppressed() {
        let tool_executor =
            StaticToolExecutor::new().register("read_file", |input| Ok(format!("body of {input}")));
        let batches = vec![
            tool_use("r1", "read_file", "{\"path\":\"a.rs\"}"),
            tool_use("r2", "read_file", "{\"path\":\"b.rs\"}"),
        ];
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            ScriptApi { batches, i: 0 },
            tool_executor,
            rw_policy(),
            vec!["sys".to_string()],
        );

        runtime.run_turn("read both", None).expect("finishes");

        let outputs = tool_result_outputs(&runtime);
        assert!(
            !outputs.iter().any(|o| o.contains("unchanged re-read")),
            "two distinct files, each read once: nothing suppressed: {outputs:?}"
        );
    }

    // The alternating read/edit churn batches shared by the two no-progress
    // tests: 5 distinct reads interleaved with 4 distinct edits, then an answer.
    // With FAILED edits, iters_since_mutation climbs to 9 at the 5th read while
    // consecutive_nav keeps resetting -> the no-progress nudge fires. With
    // SUCCESSFUL edits, every edit zeroes the counter -> it never fires.
    fn churn_batches() -> Vec<Vec<AssistantEvent>> {
        let mut batches = Vec::new();
        for k in 1..=5 {
            batches.push(tool_use(
                &format!("r{k}"),
                "read_file",
                &format!("{{\"path\":\"f{k}.rs\"}}"),
            ));
            if k <= 4 {
                batches.push(tool_use(
                    &format!("e{k}"),
                    "edit_file",
                    &format!("{{\"path\":\"f{k}.rs\",\"before\":\"a{k}\",\"after\":\"b{k}\"}}"),
                ));
            }
        }
        batches
    }

    #[test]
    fn no_progress_nudge_fires_in_read_then_failed_edit_churn() {
        let tool_executor = StaticToolExecutor::new()
            .register("read_file", |input| Ok(format!("body of {input}")))
            .register("edit_file", |_input| Err(super::ToolError::new("no match")));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            ScriptApi {
                batches: churn_batches(),
                i: 0,
            },
            tool_executor,
            rw_policy(),
            vec!["sys".to_string()],
        );

        runtime
            .run_turn("keep trying to edit", None)
            .expect("finishes");

        let outputs = tool_result_outputs(&runtime);
        let nudges = outputs
            .iter()
            .filter(|o| o.contains("no-progress:"))
            .count();
        assert_eq!(
            nudges, 1,
            "exactly one no-progress nudge in a failed-edit churn: {outputs:?}"
        );
    }

    #[test]
    fn successful_edits_reset_counter_and_suppress_no_progress_nudge() {
        let tool_executor = StaticToolExecutor::new()
            .register("read_file", |input| Ok(format!("body of {input}")))
            .register("edit_file", |_input| Ok("{\"ok\":true}".to_string()));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            ScriptApi {
                batches: churn_batches(),
                i: 0,
            },
            tool_executor,
            rw_policy(),
            vec!["sys".to_string()],
        );

        runtime.run_turn("edit each file", None).expect("finishes");

        let outputs = tool_result_outputs(&runtime);
        assert!(
            !outputs.iter().any(|o| o.contains("no-progress:")),
            "a successful edit each round resets the counter; nudge must not fire: {outputs:?}"
        );
    }

    #[test]
    fn no_progress_nudge_does_not_fire_on_pure_read_churn() {
        // 10 DISTINCT reads, NO edits. consecutive_nav and iters_since_mutation
        // climb in lockstep, so the `consecutive_nav < iters_since_mutation`
        // gate is never true: the no-progress nudge must stay silent (this churn
        // is search_budget_nudge's job). Locks the single most fragile design-B
        // invariant against a future refactor that would double-fire.
        let tool_executor =
            StaticToolExecutor::new().register("read_file", |input| Ok(format!("body of {input}")));
        let batches: Vec<Vec<AssistantEvent>> = (1..=10)
            .map(|k| {
                tool_use(
                    &format!("r{k}"),
                    "read_file",
                    &format!("{{\"path\":\"f{k}.rs\"}}"),
                )
            })
            .collect();
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            ScriptApi { batches, i: 0 },
            tool_executor,
            rw_policy(),
            vec!["sys".to_string()],
        );

        runtime.run_turn("read a lot", None).expect("finishes");

        let outputs = tool_result_outputs(&runtime);
        assert!(
            !outputs.iter().any(|o| o.contains("no-progress:")),
            "pure-read churn must NOT trigger the no-progress nudge: {outputs:?}"
        );
        assert!(
            outputs.iter().any(|o| o.contains("search-budget:")),
            "pure-read churn should still hit search_budget_nudge: {outputs:?}"
        );
    }

    #[test]
    fn records_denied_tool_results_when_prompt_rejects() {
        struct RejectPrompter;
        impl PermissionPrompter for RejectPrompter {
            fn decide(&mut self, _request: &PermissionRequest) -> PermissionPromptDecision {
                PermissionPromptDecision::Deny {
                    reason: "not now".to_string(),
                }
            }
        }

        struct SingleCallApiClient;
        impl ApiClient for SingleCallApiClient {
            fn stream(
                &mut self,
                request: &ApiRequest<'_>,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                if request
                    .messages
                    .iter()
                    .any(|message| message.role == MessageRole::Tool)
                {
                    return Ok(vec![
                        AssistantEvent::TextDelta("I could not use the tool.".to_string()),
                        AssistantEvent::MessageStop,
                    ]);
                }
                Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-1".to_string(),
                        name: "blocked".to_string(),
                        input: "secret".to_string(),
                    },
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            SingleCallApiClient,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::WorkspaceWrite)
                .with_tool_requirement("blocked", PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        );

        let summary = runtime
            .run_turn("use the tool", Some(&mut RejectPrompter))
            .expect("conversation should continue after denied tool");

        assert_eq!(summary.tool_results.len(), 1);
        assert!(matches!(
            &summary.tool_results[0].blocks[0],
            ContentBlock::ToolResult { is_error: true, output, .. } if output == "not now"
        ));
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "hook snippet uses printf; Windows cmd has no printf builtin"
    )]
    fn denies_tool_use_when_pre_tool_hook_blocks() {
        struct SingleCallApiClient;
        impl ApiClient for SingleCallApiClient {
            fn stream(
                &mut self,
                request: &ApiRequest<'_>,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                if request
                    .messages
                    .iter()
                    .any(|message| message.role == MessageRole::Tool)
                {
                    return Ok(vec![
                        AssistantEvent::TextDelta("blocked".to_string()),
                        AssistantEvent::MessageStop,
                    ]);
                }
                Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-1".to_string(),
                        name: "blocked".to_string(),
                        input: r#"{"path":"secret.txt"}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut runtime = ConversationRuntime::new_with_features(
            Session::new(),
            SingleCallApiClient,
            StaticToolExecutor::new().register("blocked", |_input| {
                panic!("tool should not execute when hook denies")
            }),
            PermissionPolicy::new(PermissionMode::DangerFullAccess)
                .with_tool_requirement("blocked", PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
            RuntimeFeatureConfig::default().with_hooks(RuntimeHookConfig::new(
                vec![shell_snippet("printf 'blocked by hook'; exit 2")],
                Vec::new(),
            )),
        );

        let summary = runtime
            .run_turn("use the tool", None)
            .expect("conversation should continue after hook denial");

        assert_eq!(summary.tool_results.len(), 1);
        let ContentBlock::ToolResult {
            is_error, output, ..
        } = &summary.tool_results[0].blocks[0]
        else {
            panic!("expected tool result block");
        };
        assert!(
            *is_error,
            "hook denial should produce an error result: {output}"
        );
        assert!(
            output.contains("denied tool") || output.contains("blocked by hook"),
            "unexpected hook denial output: {output:?}"
        );
    }

    #[test]
    fn appends_post_tool_hook_feedback_to_tool_result() {
        struct TwoCallApiClient {
            calls: usize,
        }

        impl ApiClient for TwoCallApiClient {
            fn stream(
                &mut self,
                request: &ApiRequest<'_>,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                self.calls += 1;
                match self.calls {
                    1 => Ok(vec![
                        AssistantEvent::ToolUse {
                            id: "tool-1".to_string(),
                            name: "add".to_string(),
                            input: r#"{"lhs":2,"rhs":2}"#.to_string(),
                        },
                        AssistantEvent::MessageStop,
                    ]),
                    2 => {
                        assert!(request
                            .messages
                            .iter()
                            .any(|message| message.role == MessageRole::Tool));
                        Ok(vec![
                            AssistantEvent::TextDelta("done".to_string()),
                            AssistantEvent::MessageStop,
                        ])
                    }
                    _ => Err(RuntimeError::new("unexpected extra API call")),
                }
            }
        }

        let mut runtime = ConversationRuntime::new_with_features(
            Session::new(),
            TwoCallApiClient { calls: 0 },
            StaticToolExecutor::new().register("add", |_input| Ok("4".to_string())),
            PermissionPolicy::new(PermissionMode::DangerFullAccess)
                .with_tool_requirement("add", PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
            RuntimeFeatureConfig::default().with_hooks(RuntimeHookConfig::new(
                vec![shell_snippet("printf 'pre hook ran'")],
                vec![shell_snippet("printf 'post hook ran'")],
            )),
        );

        let summary = runtime
            .run_turn("use add", None)
            .expect("tool loop succeeds");

        assert_eq!(summary.tool_results.len(), 1);
        let ContentBlock::ToolResult {
            is_error, output, ..
        } = &summary.tool_results[0].blocks[0]
        else {
            panic!("expected tool result block");
        };
        assert!(
            !*is_error,
            "post hook should preserve non-error result: {output:?}"
        );
        assert!(
            output.contains('4'),
            "tool output missing value: {output:?}"
        );
        assert!(
            output.contains("pre hook ran"),
            "tool output missing pre hook feedback: {output:?}"
        );
        assert!(
            output.contains("post hook ran"),
            "tool output missing post hook feedback: {output:?}"
        );
    }

    #[test]
    fn reconstructs_usage_tracker_from_restored_session() {
        struct SimpleApi;
        impl ApiClient for SimpleApi {
            fn stream(
                &mut self,
                _request: &ApiRequest<'_>,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut session = Session::new();
        session
            .messages
            .push(crate::session::ConversationMessage::assistant_with_usage(
                vec![ContentBlock::Text {
                    text: "earlier".to_string(),
                }],
                Some(TokenUsage {
                    input_tokens: 11,
                    output_tokens: 7,
                    cache_creation_input_tokens: 2,
                    cache_read_input_tokens: 1,
                }),
            ));

        let runtime = ConversationRuntime::new(
            session,
            SimpleApi,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        );

        assert_eq!(runtime.usage().turns(), 1);
        assert_eq!(runtime.usage().cumulative_usage().total_tokens(), 21);
    }

    #[test]
    fn compacts_session_after_turns() {
        struct SimpleApi;
        impl ApiClient for SimpleApi {
            fn stream(
                &mut self,
                _request: &ApiRequest<'_>,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            SimpleApi,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        );
        runtime.run_turn("a", None).expect("turn a");
        runtime.run_turn("b", None).expect("turn b");
        runtime.run_turn("c", None).expect("turn c");

        let result = runtime.compact(CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
        });
        assert!(result.summary.contains("Conversation summary"));
        assert_eq!(
            result.compacted_session.messages[0].role,
            MessageRole::System
        );
    }

    #[cfg(windows)]
    fn shell_snippet(script: &str) -> String {
        script.replace('\'', "\"")
    }

    #[cfg(not(windows))]
    fn shell_snippet(script: &str) -> String {
        script.to_string()
    }

    #[test]
    fn auto_compacts_when_cumulative_input_threshold_is_crossed() {
        struct SimpleApi;
        impl ApiClient for SimpleApi {
            fn stream(
                &mut self,
                _request: &ApiRequest<'_>,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::Usage(TokenUsage {
                        input_tokens: 120_000,
                        output_tokens: 4,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                    }),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let session = Session {
            version: 1,
            messages: vec![
                crate::session::ConversationMessage::user_text("one"),
                crate::session::ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "two".to_string(),
                }]),
                crate::session::ConversationMessage::user_text("three"),
                crate::session::ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "four".to_string(),
                }]),
            ],
        };

        let mut runtime = ConversationRuntime::new(
            session,
            SimpleApi,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
        .with_auto_compaction_input_tokens_threshold(100_000);

        let summary = runtime
            .run_turn("trigger", None)
            .expect("turn should succeed");

        assert_eq!(
            summary.auto_compaction,
            Some(AutoCompactionEvent {
                removed_message_count: 2,
            })
        );
        assert_eq!(runtime.session().messages[0].role, MessageRole::System);
    }

    #[test]
    fn skips_auto_compaction_below_threshold() {
        struct SimpleApi;
        impl ApiClient for SimpleApi {
            fn stream(
                &mut self,
                _request: &ApiRequest<'_>,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::Usage(TokenUsage {
                        input_tokens: 99_999,
                        output_tokens: 4,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                    }),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            SimpleApi,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
        .with_auto_compaction_input_tokens_threshold(100_000);

        let summary = runtime
            .run_turn("trigger", None)
            .expect("turn should succeed");
        assert_eq!(summary.auto_compaction, None);
        assert_eq!(runtime.session().messages.len(), 2);
    }

    #[test]
    fn auto_compaction_threshold_defaults_and_parses_values() {
        assert_eq!(
            parse_auto_compaction_threshold(None),
            DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD
        );
        assert_eq!(parse_auto_compaction_threshold(Some("4321")), 4321);
        assert_eq!(
            parse_auto_compaction_threshold(Some("not-a-number")),
            DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD
        );
    }

    // ── unknown-tool short-circuit ─────────────────────────────────────

    /// API client that emits one tool_use for `tool_name` on the first call,
    /// then a final text response on the second call (which the runtime
    /// reaches once the unknown-tool tool_result has been recorded).
    struct UnknownToolApi {
        tool_name: String,
        calls: usize,
    }

    impl ApiClient for UnknownToolApi {
        fn stream(
            &mut self,
            _request: &ApiRequest<'_>,
        ) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.calls += 1;
            match self.calls {
                1 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-1".to_string(),
                        name: self.tool_name.clone(),
                        input: "{}".to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                _ => Ok(vec![
                    AssistantEvent::TextDelta("ok, recovered.".to_string()),
                    AssistantEvent::MessageStop,
                ]),
            }
        }
    }

    struct ForbiddenPrompter;

    impl PermissionPrompter for ForbiddenPrompter {
        fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
            panic!(
                "prompter must not be invoked for unknown tool {} — \
                 short-circuit is broken",
                request.tool_name
            );
        }
    }

    struct ForbiddenExecutor;

    impl super::ToolExecutor for ForbiddenExecutor {
        fn execute(&mut self, tool_name: &str, _input: &str) -> Result<String, super::ToolError> {
            panic!("executor must not be invoked for unknown tool {tool_name}");
        }
    }

    #[test]
    fn unknown_tool_short_circuits_without_invoking_prompter_or_executor() {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("note_create", PermissionMode::WorkspaceWrite)
            .with_tool_requirement("note_read", PermissionMode::ReadOnly);
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            UnknownToolApi {
                tool_name: "note_update".to_string(),
                calls: 0,
            },
            ForbiddenExecutor,
            policy,
            vec!["system".to_string()],
        );

        let summary = runtime
            .run_turn("update my note", Some(&mut ForbiddenPrompter))
            .expect("unknown tool should produce a tool_result, not error the loop");

        assert_eq!(summary.tool_results.len(), 1);
        let ContentBlock::ToolResult {
            is_error, output, ..
        } = &summary.tool_results[0].blocks[0]
        else {
            panic!("expected tool result block");
        };
        assert!(*is_error, "unknown-tool result must be flagged is_error");
        assert!(
            output.contains("unknown tool: note_update"),
            "result must name the unknown tool: {output}"
        );
        assert!(
            output.contains("note_create") && output.contains("note_read"),
            "did_you_mean must list both registered note_* tools: {output}"
        );
        assert!(
            output.contains("\"did_you_mean\""),
            "result body must be JSON-shaped with a did_you_mean key: {output}"
        );
    }

    #[test]
    fn unknown_tool_falls_through_to_hinter_when_suggest_for_is_empty() {
        // `facts` is a group-name confabulation — no registered tool name
        // shares any substring or close edit distance with it. The generic
        // suggest_for returns []; the hinter must kick in and supply the
        // group's actual tools.
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("weather_current", PermissionMode::ReadOnly)
            .with_tool_requirement("wikipedia_search", PermissionMode::ReadOnly);
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            UnknownToolApi {
                tool_name: "facts".to_string(),
                calls: 0,
            },
            ForbiddenExecutor,
            policy,
            vec!["system".to_string()],
        )
        .with_unknown_tool_hinter(|name: &str| {
            if name == "facts" {
                vec![
                    "weather_current".to_string(),
                    "wikipedia_search".to_string(),
                ]
            } else {
                Vec::new()
            }
        });

        let summary = runtime
            .run_turn("what's the weather?", Some(&mut ForbiddenPrompter))
            .expect("conversation continues after unknown tool");

        let ContentBlock::ToolResult { output, .. } = &summary.tool_results[0].blocks[0] else {
            panic!("expected tool result block");
        };
        assert!(
            output.contains("weather_current") && output.contains("wikipedia_search"),
            "hinter suggestions must appear in did_you_mean: {output}"
        );
    }

    #[test]
    fn unknown_tool_with_no_suggestions_returns_empty_did_you_mean() {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("weather_current", PermissionMode::ReadOnly);
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            UnknownToolApi {
                tool_name: "totally_unrelated_xyz".to_string(),
                calls: 0,
            },
            ForbiddenExecutor,
            policy,
            vec!["system".to_string()],
        );

        let summary = runtime
            .run_turn("?", Some(&mut ForbiddenPrompter))
            .expect("loop continues");

        let ContentBlock::ToolResult { output, .. } = &summary.tool_results[0].blocks[0] else {
            panic!("expected tool result block");
        };
        // Empty list is still a structured response — the brain knows the
        // tool doesn't exist even if no close match is suggested.
        assert!(
            output.contains("\"did_you_mean\":[]"),
            "no-match case should still produce a structured response: {output}"
        );
    }

    // ─── Vision-discipline hint (P2) ────────────────────────────────────────

    #[test]
    fn build_turn_system_prompt_no_op_when_no_images() {
        let base = vec!["base prompt".to_string()];
        let result = build_turn_system_prompt(&base, false);
        assert_eq!(result, base);
    }

    #[test]
    fn build_turn_system_prompt_appends_hint_when_images_present() {
        let base = vec!["base prompt".to_string()];
        let result = build_turn_system_prompt(&base, true);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "base prompt");
        assert_eq!(result[1], VISION_DISCIPLINE_HINT);
    }

    #[test]
    fn build_turn_system_prompt_preserves_multi_segment_base() {
        let base = vec!["seg one".to_string(), "seg two".to_string()];
        let result = build_turn_system_prompt(&base, true);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "seg one");
        assert_eq!(result[1], "seg two");
        assert_eq!(result[2], VISION_DISCIPLINE_HINT);
    }

    #[test]
    fn vision_discipline_hint_mentions_image() {
        // Cheap sanity check that the constant didn't drift to an unrelated
        // string during refactors.
        assert!(VISION_DISCIPLINE_HINT.to_lowercase().contains("image"));
    }
}
