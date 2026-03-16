//! Workflow classification reminder hook.
//!
//! Injects a short system reminder when a new topic boundary is detected so the
//! agent classifies the request and routes non-trivial work through the
//! dev-orchestrator workflow.

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::{HookInput, ProjectDirs, sanitize_session_id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

pub struct WorkflowClassificationReminderHook;

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

        Ok(json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": format!(
                    "<system-reminder source=\"hooks-workflow-classification\">\n{}\n</system-reminder>",
                    build_reminder(&prompt)
                )
            }
        }))
    }
}

fn extract_user_prompt(user_prompt: Option<&str>, extra: &Value) -> String {
    if let Some(prompt) = user_prompt
        && !prompt.trim().is_empty()
    {
        return prompt.to_string();
    }

    if let Some(prompt) = extra.get("prompt").and_then(Value::as_str)
        && !prompt.trim().is_empty()
    {
        return prompt.to_string();
    }

    match extra.get("userMessage") {
        Some(Value::String(prompt)) if !prompt.trim().is_empty() => prompt.clone(),
        Some(Value::Object(obj)) => obj
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
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

fn build_reminder(user_prompt: &str) -> String {
    let truncated = if user_prompt.chars().count() > 100 {
        let prefix = user_prompt.chars().take(100).collect::<String>();
        format!("{prefix}...")
    } else {
        user_prompt.to_string()
    };

    format!(
        "NEW TOPIC DETECTED - Classify and Route\n\n\
         Request: \"{truncated}\"\n\n\
         Classify (choose ONE):\n\
           Q&A         -> \"what is\", \"explain\", \"how do I\"        -> respond directly\n\
           OPERATIONS  -> \"cleanup\", \"delete\", \"git status\"       -> execute directly\n\
           INVESTIGATION/DEVELOPMENT -> all other non-trivial work -> use dev-orchestrator\n\n\
         For INVESTIGATION or DEVELOPMENT tasks:\n\
           Invoke Skill(skill=\"dev-orchestrator\") -- the smart-orchestrator will:\n\
             - Classify the task and formulate a clear goal\n\
             - Detect parallel workstreams if task has independent components\n\
             - Execute via recipe runner (single task or parallel workstreams)\n\
             - Reflect on goal achievement\n\n\
           Entry point: /dev <task description>\n\
           (Legacy: /ultrathink is deprecated -- use /dev)\n\n\
         DO NOT start implementation without invoking dev-orchestrator first."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn build_reminder_mentions_dev_orchestrator() {
        let reminder = build_reminder("Port this Python hook to Rust");
        assert!(reminder.contains("dev-orchestrator"));
        assert!(reminder.contains("NEW TOPIC DETECTED"));
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
}
