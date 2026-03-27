//! Workflow classification reminder hook.
//!
//! Injects a short system reminder when a new topic boundary is detected so the
//! agent classifies the request and routes non-trivial work through the
//! dev-orchestrator workflow.

use crate::prompt_input::extract_user_prompt;
use crate::protocol::{FailurePolicy, Hook};
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
                extract_turn_count(&extra).unwrap_or(0),
            ),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        if prompt.trim().is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        let dirs = ProjectDirs::from_cwd();
        if !is_new_topic(&dirs, &session_id, turn_count, &prompt) {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        save_classification_state(&dirs, &session_id, turn_count)?;

        let reminder = load_routing_prompt(&dirs);
        if reminder.trim().is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        Ok(json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": reminder
            }
        }))
    }
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

fn save_classification_state(
    dirs: &ProjectDirs,
    session_id: &str,
    turn_count: u64,
) -> anyhow::Result<()> {
    let path = state_file(dirs, session_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let state = ClassificationState {
        last_classified_turn: turn_count,
        session_id: session_id.to_string(),
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
}
