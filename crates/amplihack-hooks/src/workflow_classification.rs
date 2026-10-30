//! Workflow classification reminder hook.
//!
//! Injects a short system reminder when a new topic boundary is detected so the
//! agent classifies the request and routes non-trivial work through the
//! dev-orchestrator workflow.
//!
//! A prompt that starts with the [`COORDINATION_PREFIX`] marker is coordination
//! traffic between sessions (issue #1742). It gets [`COORDINATION_NOTE`] instead
//! of the routing prompt, unless its body starts with a change verb. See
//! `docs/reference/coordination-messages.md`.
//!
//! The marker rule is in `coordination`, the per-session state file in `state`.

mod coordination;
mod state;

pub use coordination::{COORDINATION_NOTE, COORDINATION_PREFIX};

use crate::prompt_input::extract_user_prompt;
use crate::protocol::{FailurePolicy, Hook};
use crate::session_start::{is_nested_recipe_session, is_workflow_active};
use amplihack_types::{HookInput, ProjectDirs};
use coordination::{is_explicit_work_request, strip_coordination_prefix};
use serde_json::{Value, json};
use state::{ClassificationState, next_observed_turn, record_outcome, state_file};
use std::fs;

pub struct WorkflowClassificationReminderHook;

const DEFAULT_ROUTING_PROMPT: &str = include_str!("routing_prompt.txt");

/// Values written to `ClassificationState::last_rule`, one per outcome.
///
/// These strings are read by people inspecting state files, so renaming one is
/// a breaking change. The table is in `docs/reference/coordination-messages.md`.
mod rule {
    pub(super) const EXPLICIT_DEV: &str = "explicit_dev_command";
    pub(super) const COORD_EXPLICIT_DEV: &str = "coordination_explicit_dev";
    pub(super) const COORD_MARKER: &str = "coordination_marker";
    pub(super) const CLASSIFICATION: &str = "classification";
    pub(super) const THROTTLED: &str = "throttled";
}

/// What the topic logic decided for an unmarked prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopicDecision {
    /// The user typed an explicit `/dev` command; the skill handles routing.
    ExplicitDev,
    /// A new topic: inject the routing prompt.
    NewTopic,
    /// A follow-up or a prompt soon after the last classification.
    Throttled,
}

impl Hook for WorkflowClassificationReminderHook {
    fn name(&self) -> &'static str {
        "workflow_classification_reminder"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (prompt, session_id, turn_count) = match input {
            HookInput::UserPromptSubmit {
                user_prompt,
                session_id,
                extra,
            } => (
                extract_user_prompt(user_prompt.as_deref(), &extra),
                // An empty id would reach the non-empty assertion in
                // `sanitize_session_id` on the state-writing paths.
                session_id
                    .filter(|id| !id.is_empty())
                    .unwrap_or_else(|| "unknown-session".to_string()),
                extract_turn_count(&extra),
            ),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        if prompt.trim().is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // A runtime that reports no turn number gets one we keep ourselves, so
        // both runtimes reach `topic_decision` with the same meaning. Falling back
        // to 0 would pin every Copilot prompt at "first turn".
        let dirs_for_turn = ProjectDirs::from_cwd();
        let turn_count = match turn_count {
            Some(reported) => reported,
            None => next_observed_turn(&dirs_for_turn, &session_id),
        };

        // Issue #1328: a prompt written by amplihack for one of its own recipe
        // steps is never a human changing topic, and must never be routed.
        //
        // This is the primary gate. The two below it are secondary and both
        // proved unreliable in the field: `is_nested_recipe_session` reads an env
        // var nothing seeded at a root until #1326, and `is_workflow_active` reads
        // a semaphore whose path is derived from the agent's cwd, so a worktree hop
        // hides it from its own holder.
        if is_agent_authored_prompt() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // When a workflow is already active, skip classification reminders
        // to prevent recursive workflow invocation (ported from Python PR #3974).
        let dirs = ProjectDirs::from_cwd();
        if is_nested_recipe_session() || is_workflow_active(&dirs) {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // Issue #1742: a leading `[coord]` marker means another session typed
        // this message. It runs after every gate above, so it can never make a
        // recipe step or a nested session receive routing text.
        if let Some(body) = strip_coordination_prefix(&prompt) {
            if is_explicit_dev_command(body) {
                record_outcome(&dirs, &session_id, turn_count, rule::EXPLICIT_DEV, false);
                return Ok(Value::Object(serde_json::Map::new()));
            }
            if !is_explicit_work_request(body) {
                record_outcome(&dirs, &session_id, turn_count, rule::COORD_MARKER, false);
                return Ok(context_output(COORDINATION_NOTE));
            }
            // A body that starts with a change verb is classified as usual,
            // without the throttle: the sender asked for work.
            record_outcome(
                &dirs,
                &session_id,
                turn_count,
                rule::COORD_EXPLICIT_DEV,
                true,
            );
        } else {
            match topic_decision(&dirs, &session_id, turn_count, &prompt) {
                TopicDecision::ExplicitDev => {
                    record_outcome(&dirs, &session_id, turn_count, rule::EXPLICIT_DEV, false);
                    return Ok(Value::Object(serde_json::Map::new()));
                }
                TopicDecision::Throttled => {
                    record_outcome(&dirs, &session_id, turn_count, rule::THROTTLED, false);
                    return Ok(Value::Object(serde_json::Map::new()));
                }
                TopicDecision::NewTopic => {
                    record_outcome(&dirs, &session_id, turn_count, rule::CLASSIFICATION, true);
                }
            }
        }

        let reminder = load_routing_prompt(&dirs);
        if reminder.trim().is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        Ok(context_output(&reminder))
    }
}

/// Build the hook output that injects `text` as additional context.
fn context_output(text: &str) -> Value {
    // Two runtimes, two contracts, one gated source.
    //
    // Claude Code reads `hookSpecificOutput.additionalContext`. The Copilot
    // CLI reads a *top-level* `additionalContext` and never looks at
    // `hookSpecificOutput` -- the string does not appear anywhere in its
    // binary. So under Copilot this hook's output was discarded in both
    // directions: it could not route a human, and the provenance gate in `process`
    // could not suppress anything, because there was nothing to suppress.
    //
    // That is why the router still reached recipe steps after #1328: it was
    // arriving through the repo-root AGENTS.md instead, which is ungated and
    // is loaded into every leaf agent (issue #1333). Removing that file
    // without emitting a shape Copilot reads would silently drop human
    // routing under Copilot altogether.
    //
    // Both keys are additive and each runtime ignores the other's.
    json!({
        "additionalContext": text,
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": text
        }
    })
}

/// Environment marker set by `recipe::run::execute` on every agent it spawns.
pub(crate) const RECIPE_RUN_ID_ENV: &str = "AMPLIHACK_RECIPE_RUN_ID";

/// Was this prompt authored by amplihack for one of its own recipe steps?
///
/// Issue #1328. The router used to be injected into machine-authored step prompts,
/// and the agent's first emitted token would be
/// `[auto-routed] INVESTIGATE -> launching dev-orchestrator`, re-orchestrating the
/// very step it had been handed. On the host that motivated this, 43% of all turns
/// (5,220 of 12,085) were the same step -- `classify-and-decompose` -- re-classifying
/// progressively narrower restatements of the orchestrator's own current step.
///
/// Provenance is the right discriminator. The previous one, `turn_count <= 1`, cannot
/// distinguish "first prompt of a human's session" from "the only prompt a
/// machine-spawned step session will ever receive": of 22,964 sessions on that host,
/// 10,972 had zero turns and 11,986 had exactly one, so it was `true` in precisely the
/// case that mattered.
fn is_agent_authored_prompt() -> bool {
    std::env::var_os(RECIPE_RUN_ID_ENV).is_some_and(|v| !v.is_empty())
}

fn extract_turn_count(extra: &Value) -> Option<u64> {
    extra
        .get("turnCount")
        .and_then(Value::as_u64)
        .or_else(|| extra.get("turn_count").and_then(Value::as_u64))
}

fn is_explicit_dev_command(user_prompt: &str) -> bool {
    let prompt_lower = user_prompt.trim().to_lowercase();
    prompt_lower.starts_with("/dev ")
        || prompt_lower == "/dev"
        || prompt_lower.starts_with("/amplihack:dev")
        || prompt_lower.starts_with("/.claude:amplihack:dev")
}

fn topic_decision(
    dirs: &ProjectDirs,
    session_id: &str,
    turn_count: u64,
    user_prompt: &str,
) -> TopicDecision {
    if is_explicit_dev_command(user_prompt) {
        return TopicDecision::ExplicitDev;
    }

    if turn_count <= 1 {
        return TopicDecision::NewTopic;
    }

    let prompt_lower = user_prompt.to_lowercase();
    let transition_keywords = [
        "now let's",
        "next i want",
        "switching to",
        "different question",
        "different topic",
        "new task",
        "moving on to",
    ];
    if transition_keywords
        .iter()
        .any(|keyword| prompt_lower.contains(keyword))
    {
        return TopicDecision::NewTopic;
    }

    let followup_keywords = [
        "also",
        "what about",
        "and",
        "additionally",
        "furthermore",
        "i meant",
        "to clarify",
        "how's it going",
        "what's the status",
        "what's the progress",
    ];
    let first_words = prompt_lower
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    if followup_keywords
        .iter()
        .any(|keyword| first_words.contains(keyword))
    {
        return TopicDecision::Throttled;
    }

    // A session with no classification yet has no window to throttle within.
    let path = state_file(dirs, session_id);
    if let Ok(raw) = fs::read_to_string(path)
        && let Ok(state) = serde_json::from_str::<ClassificationState>(&raw)
        && let Some(last_classified) = state.last_classified_turn
        && turn_count.saturating_sub(last_classified) <= 3
    {
        return TopicDecision::Throttled;
    }

    TopicDecision::NewTopic
}

fn load_routing_prompt(dirs: &ProjectDirs) -> String {
    let Some(path) =
        dirs.resolve_framework_file(".claude/tools/amplihack/hooks/templates/routing_prompt.txt")
    else {
        return DEFAULT_ROUTING_PROMPT.to_string();
    };
    fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_ROUTING_PROMPT.to_string())
}

#[cfg(test)]
mod tests;
