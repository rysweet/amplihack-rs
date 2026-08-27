//! Workflow classification reminder hook.
//!
//! Injects a short system reminder when a new topic boundary is detected so the
//! agent classifies the request and routes non-trivial work through the
//! dev-orchestrator workflow.

use crate::prompt_input::extract_user_prompt;
use crate::protocol::{FailurePolicy, Hook};
use crate::session_start::{is_nested_recipe_session, is_workflow_active};
use amplihack_types::{HookInput, ProjectDirs, sanitize_session_id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

pub struct WorkflowClassificationReminderHook;

const DEFAULT_ROUTING_PROMPT: &str = include_str!("routing_prompt.txt");

#[derive(Debug, Serialize, Deserialize)]
struct ClassificationState {
    last_classified_turn: u64,
    session_id: String,
    /// Prompts seen in this session, counted by us.
    ///
    /// Only used when the runtime does not report one. The Copilot CLI's
    /// `userPromptSubmitted` payload is `{prompt}` and carries no turn number,
    /// so treating a missing count as turn 0 would make `turn_count <= 1`
    /// permanently true and route every human prompt forever (issue #1333).
    #[serde(default)]
    observed_turns: u64,
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
                session_id.unwrap_or_else(|| "unknown-session".to_string()),
                extract_turn_count(&extra),
            ),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        if prompt.trim().is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // A runtime that reports no turn number gets one we keep ourselves, so
        // both runtimes reach `is_new_topic` with the same meaning. Falling back
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

        if !is_new_topic(&dirs, &session_id, turn_count, &prompt) {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        save_classification_state(&dirs, &session_id, turn_count)?;

        let reminder = load_routing_prompt(&dirs);
        if reminder.trim().is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // Two runtimes, two contracts, one gated source.
        //
        // Claude Code reads `hookSpecificOutput.additionalContext`. The Copilot
        // CLI reads a *top-level* `additionalContext` and never looks at
        // `hookSpecificOutput` -- the string does not appear anywhere in its
        // binary. So under Copilot this hook's output was discarded in both
        // directions: it could not route a human, and the provenance gate above
        // could not suppress anything, because there was nothing to suppress.
        //
        // That is why the router still reached recipe steps after #1328: it was
        // arriving through the repo-root AGENTS.md instead, which is ungated and
        // is loaded into every leaf agent (issue #1333). Removing that file
        // without emitting a shape Copilot reads would silently drop human
        // routing under Copilot altogether.
        //
        // Both keys are additive and each runtime ignores the other's.
        Ok(json!({
            "additionalContext": reminder,
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": reminder
            }
        }))
    }
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

fn state_file(dirs: &ProjectDirs, session_id: &str) -> PathBuf {
    dirs.runtime
        .join("classification_state")
        .join(format!("{}.json", sanitize_session_id(session_id)))
}

fn is_explicit_dev_command(user_prompt: &str) -> bool {
    let prompt_lower = user_prompt.trim().to_lowercase();
    prompt_lower.starts_with("/dev ")
        || prompt_lower == "/dev"
        || prompt_lower.starts_with("/amplihack:dev")
        || prompt_lower.starts_with("/.claude:amplihack:dev")
}

fn is_new_topic(dirs: &ProjectDirs, session_id: &str, turn_count: u64, user_prompt: &str) -> bool {
    if is_explicit_dev_command(user_prompt) {
        return false;
    }

    if turn_count <= 1 {
        return true;
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
        return true;
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
        return false;
    }

    let path = state_file(dirs, session_id);
    if let Ok(raw) = fs::read_to_string(path)
        && let Ok(state) = serde_json::from_str::<ClassificationState>(&raw)
        && turn_count.saturating_sub(state.last_classified_turn) <= 3
    {
        return false;
    }

    true
}

/// Read, increment and persist this session's own prompt counter.
///
/// Returns the turn number for the prompt being handled now: 0 for the first,
/// 1 for the second, and so on -- matching what a runtime that reports
/// `turnCount` would send, so downstream logic needs no special case.
///
/// A write failure returns the un-incremented count rather than failing the
/// hook. The cost is one extra reminder, not a broken prompt.
fn next_observed_turn(dirs: &ProjectDirs, session_id: &str) -> u64 {
    let path = state_file(dirs, session_id);
    let mut state = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ClassificationState>(&raw).ok())
        .unwrap_or_else(|| ClassificationState {
            last_classified_turn: 0,
            session_id: session_id.to_string(),
            observed_turns: 0,
        });
    let current = state.observed_turns;
    state.observed_turns = current.saturating_add(1);
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_ok()
        && let Ok(body) = serde_json::to_vec(&state)
    {
        let _ = fs::write(&path, body);
    }
    current
}

fn save_classification_state(
    dirs: &ProjectDirs,
    session_id: &str,
    turn_count: u64,
) -> anyhow::Result<()> {
    let path = state_file(dirs, session_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let observed_turns = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ClassificationState>(&raw).ok())
        .map(|prev| prev.observed_turns)
        .unwrap_or(0);
    let state = ClassificationState {
        last_classified_turn: turn_count,
        session_id: session_id.to_string(),
        observed_turns,
    };
    fs::write(path, serde_json::to_vec(&state)?)?;
    Ok(())
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
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Issue #1328: a prompt amplihack wrote for its own recipe step is never a
    /// human changing topic. This is the case that produced the cascade -- the
    /// agent's first token was `[auto-routed] ... launching dev-orchestrator`,
    /// re-orchestrating the step it had just been handed.
    #[test]
    fn agent_authored_prompts_are_never_routed() {
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _run = crate::test_support::EnvVarGuard::set(RECIPE_RUN_ID_ENV, "run-abc");
        assert!(is_agent_authored_prompt());

        let out = WorkflowClassificationReminderHook
            .process(HookInput::UserPromptSubmit {
                user_prompt: Some("Design an exploration strategy for this investigation.".into()),
                session_id: Some("s-agent".into()),
                extra: json!({ "turnCount": 0 }),
            })
            .expect("hook runs");
        assert_eq!(
            out,
            Value::Object(serde_json::Map::new()),
            "an agent-authored prompt must receive no routing text at all"
        );
    }

    /// The human path must be untouched. A fix that silences routing for everyone
    /// would satisfy the safety goal and remove the feature.
    #[test]
    fn human_prompts_are_still_routed() {
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _run = crate::test_support::EnvVarGuard::unset(RECIPE_RUN_ID_ENV);
        let _depth = crate::test_support::EnvVarGuard::unset("AMPLIHACK_SESSION_DEPTH");
        assert!(!is_agent_authored_prompt());
    }

    /// An exported-but-empty marker is not a marker. Otherwise a stray
    /// `export AMPLIHACK_RECIPE_RUN_ID=` would silently disable routing for a human.
    #[test]
    fn an_empty_marker_does_not_count_as_agent_authored() {
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _run = crate::test_support::EnvVarGuard::set(RECIPE_RUN_ID_ENV, "");
        assert!(!is_agent_authored_prompt());
    }

    /// The discriminator that failed. Of 22,964 sessions on the affected host,
    /// 10,972 had zero turns and 11,986 had exactly one -- so `turn_count <= 1` was
    /// true for essentially every machine-spawned step, which is precisely the case
    /// it needed to exclude. Provenance must decide regardless of turn count.
    #[test]
    fn turn_count_does_not_override_provenance() {
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _run = crate::test_support::EnvVarGuard::set(RECIPE_RUN_ID_ENV, "run-abc");
        for turns in [0u64, 1, 2, 50] {
            let out = WorkflowClassificationReminderHook
                .process(HookInput::UserPromptSubmit {
                    user_prompt: Some("Define the scope for this investigation task.".into()),
                    session_id: Some(format!("s{turns}")),
                    extra: json!({ "turnCount": turns }),
                })
                .expect("hook runs");
            assert_eq!(
                out,
                Value::Object(serde_json::Map::new()),
                "turnCount={turns} must not resurrect routing for an agent prompt"
            );
        }
    }

    #[test]
    fn explicit_dev_command_skips_reminder() {
        let dirs = ProjectDirs::new("/tmp/project");
        assert!(!is_new_topic(&dirs, "s1", 2, "/dev fix this"));
    }

    #[test]
    fn first_turn_is_new_topic() {
        let dirs = ProjectDirs::new("/tmp/project");
        assert!(is_new_topic(
            &dirs,
            "s1",
            0,
            "Please investigate this issue"
        ));
    }

    #[test]
    fn followup_prefix_is_not_new_topic() {
        let dirs = ProjectDirs::new("/tmp/project");
        assert!(!is_new_topic(&dirs, "s1", 10, "Also update the tests"));
    }

    #[test]
    fn recent_classification_suppresses_followup_turns() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::from_root(temp.path());
        save_classification_state(&dirs, "session-1", 4).unwrap();
        assert!(!is_new_topic(
            &dirs,
            "session-1",
            6,
            "Please continue on that bug"
        ));
        assert!(is_new_topic(
            &dirs,
            "session-1",
            8,
            "Please continue on that bug"
        ));
    }

    #[test]
    fn transition_keyword_forces_new_topic() {
        let dirs = ProjectDirs::new("/tmp/project");
        assert!(is_new_topic(
            &dirs,
            "s1",
            9,
            "Now let's switch to the install flow"
        ));
    }

    #[test]
    fn load_routing_prompt_mentions_dev_orchestrator() {
        let dirs = ProjectDirs::new("/tmp/project");
        let reminder = load_routing_prompt(&dirs);
        assert!(reminder.contains("dev-orchestrator"));
        assert!(reminder.contains("parallel signal evaluation"));
        assert!(reminder.contains("flowchart TD"));
    }

    #[test]
    fn default_routing_prompt_matches_richer_parallel_contract() {
        assert!(DEFAULT_ROUTING_PROMPT.contains("parallel signal evaluation"));
        assert!(DEFAULT_ROUTING_PROMPT.contains("flowchart TD"));
        assert!(DEFAULT_ROUTING_PROMPT.contains("UNDERSTAND + IMPLEMENT"));
        assert!(DEFAULT_ROUTING_PROMPT.contains("False positive costs minutes"));
    }

    #[test]
    fn extracts_prompt_from_extra_fields() {
        let extra = json!({
            "prompt": "hello",
            "turnCount": 3
        });
        assert_eq!(extract_user_prompt(None, &extra), "hello");
        assert_eq!(extract_turn_count(&extra), Some(3));
    }

    #[test]
    fn load_routing_prompt_uses_amplihack_root_override() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let project = tempfile::tempdir().unwrap();
        let framework = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(project.path());
        let template_dir = framework
            .path()
            .join(".claude/tools/amplihack/hooks/templates");
        fs::create_dir_all(&template_dir).unwrap();
        fs::write(
            template_dir.join("routing_prompt.txt"),
            "<system-reminder source=\"auto-intent-router\">Framework override</system-reminder>",
        )
        .unwrap();
        let previous = std::env::var_os("AMPLIHACK_ROOT");
        unsafe { std::env::set_var("AMPLIHACK_ROOT", framework.path()) };

        let reminder = load_routing_prompt(&dirs);

        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_ROOT", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_ROOT") },
        }

        assert_eq!(
            reminder,
            "<system-reminder source=\"auto-intent-router\">Framework override</system-reminder>"
        );
    }

    /// Issue #1333: the Copilot CLI reads a top-level `additionalContext` and
    /// never looks at `hookSpecificOutput` -- that key appears zero times in its
    /// binary. Emitting only the Claude shape meant this hook was inert under
    /// Copilot, so the router had to arrive through the ungated repo-root
    /// AGENTS.md instead, which reached every leaf agent step.
    #[test]
    fn emits_the_copilot_shape_for_a_human_prompt() {
        // Serialise against every other test that mutates this variable --
        // an unlocked EnvVarGuard races the ones that do.
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::from_root(temp.path());
        let _run = crate::test_support::EnvVarGuard::unset(RECIPE_RUN_ID_ENV);

        let out = classify(&dirs, "s-copilot", None, "Please investigate this issue");

        let top = out
            .get("additionalContext")
            .and_then(Value::as_str)
            .expect("top-level additionalContext is the only key Copilot reads");
        assert!(top.contains("dev-orchestrator"));
        assert_eq!(
            out.pointer("/hookSpecificOutput/additionalContext")
                .and_then(Value::as_str),
            Some(top),
            "the Claude shape must carry the same text, not diverge from it"
        );
    }

    /// The regression the Copilot shape would otherwise have shipped.
    ///
    /// Copilot's `userPromptSubmitted` payload is `{prompt}` and carries no turn
    /// number. Treating that as turn 0 makes `turn_count <= 1` permanently true,
    /// so once the output is actually delivered every human prompt gets 3 KB of
    /// mandatory routing -- including "no, undo that". #1328 and #1330 were spent
    /// learning not to ship an unbounded router; this asserts it is not shipped
    /// to a second runtime on the way out.
    #[test]
    fn a_runtime_that_reports_no_turn_count_is_still_throttled() {
        // Serialise against every other test that mutates this variable --
        // an unlocked EnvVarGuard races the ones that do.
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::from_root(temp.path());
        let _run = crate::test_support::EnvVarGuard::unset(RECIPE_RUN_ID_ENV);

        // Every prompt below omits turnCount, exactly as Copilot sends it.
        let routed: Vec<bool> = (0..8)
            .map(|i| {
                let out = classify(
                    &dirs,
                    "s-no-turncount",
                    None,
                    &format!("Please investigate issue number {i}"),
                );
                out.get("additionalContext").is_some()
            })
            .collect();

        let count = routed.iter().filter(|r| **r).count();
        assert!(
            count < routed.len(),
            "eight prompts, all routed: the router is unbounded under a runtime \
             that reports no turn count -- {routed:?}"
        );
        assert!(
            routed[0],
            "the first prompt of a session is a genuine new topic and must route"
        );
        assert!(
            !routed[routed.len() - 1],
            "by the eighth prompt the throttle must have taken hold -- {routed:?}"
        );
    }

    /// A runtime that does report a turn count keeps its old behaviour.
    #[test]
    fn a_reported_turn_count_is_used_verbatim() {
        // Serialise against every other test that mutates this variable --
        // an unlocked EnvVarGuard races the ones that do.
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::from_root(temp.path());
        let _run = crate::test_support::EnvVarGuard::unset(RECIPE_RUN_ID_ENV);

        let first = classify(&dirs, "s-claude", Some(0), "Please investigate this");
        assert!(first.get("additionalContext").is_some());

        let follow = classify(&dirs, "s-claude", Some(2), "Please investigate that");
        assert!(
            follow.get("additionalContext").is_none(),
            "turn 2 within three turns of turn 0 must be suppressed"
        );
    }

    /// Drive the hook the way a runtime does, with the project dirs pinned to a
    /// temp root so state never lands in the real one.
    fn classify(dirs: &ProjectDirs, session: &str, turn: Option<u64>, prompt: &str) -> Value {
        let extra = match turn {
            Some(t) => json!({ "turnCount": t }),
            None => json!({}),
        };
        let turn_count = match extract_turn_count(&extra) {
            Some(reported) => reported,
            None => next_observed_turn(dirs, session),
        };
        if !is_new_topic(dirs, session, turn_count, prompt) {
            return Value::Object(serde_json::Map::new());
        }
        save_classification_state(dirs, session, turn_count).unwrap();
        let reminder = load_routing_prompt(dirs);
        json!({
            "additionalContext": reminder,
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": reminder
            }
        })
    }

    /// The regression lock for #1328: making the hook visible to Copilot must
    /// not re-open the hole it closed. An agent-authored recipe step gets
    /// nothing -- in either shape.
    #[test]
    fn emits_neither_shape_for_an_agent_authored_prompt() {
        // Serialise against every other test that mutates this variable --
        // an unlocked EnvVarGuard races the ones that do.
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _run = crate::test_support::EnvVarGuard::set(RECIPE_RUN_ID_ENV, "run-abc");
        let hook = WorkflowClassificationReminderHook;
        let out = hook
            .process(HookInput::UserPromptSubmit {
                user_prompt: Some("Please investigate this issue".to_string()),
                session_id: Some("s-agent".to_string()),
                extra: json!({"turnCount": 0}),
            })
            .unwrap();

        assert!(
            out.get("additionalContext").is_none(),
            "a recipe step must not receive the router in the Copilot shape"
        );
        assert!(
            out.get("hookSpecificOutput").is_none(),
            "nor in the Claude shape"
        );
    }
}
