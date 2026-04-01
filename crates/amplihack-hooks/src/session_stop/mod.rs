//! Session stop hook: stores session memory and warns about uncommitted work.
//!
//! Two responsibilities:
//! 1. Store session-end learnings natively in the Rust memory backends
//! 2. Check for uncommitted git changes and warn the user
//!
//! Neither blocks session exit (fail-open).

mod git;
mod transcript;
#[cfg(test)]
mod tests;

use crate::agent_memory::normalize_agent_name;
use crate::protocol::{FailurePolicy, Hook};
use amplihack_cli::memory::store_session_learning;
use amplihack_types::HookInput;
use serde_json::Value;
use std::path::Path;

pub struct SessionStopHook;

impl Hook for SessionStopHook {
    fn name(&self) -> &'static str {
        "session_stop"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (session_id, transcript_path, extra) = match input {
            HookInput::SessionStop {
                session_id,
                transcript_path,
                extra,
            } => (session_id, transcript_path, extra),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        let session_id = session_id.unwrap_or_else(|| "hook_session".to_string());
        match collect_session_learning_inputs(&session_id, transcript_path.as_deref(), &extra) {
            Ok(learnings) => {
                for learning in learnings {
                    if let Err(error) = store_session_learning(
                        &session_id,
                        &learning.agent_id,
                        &learning.content,
                        learning.task.as_deref(),
                        learning.success,
                    ) {
                        tracing::error!("Session-end learning store failed: {}", error);
                    }
                }
            }
            Err(error) => tracing::warn!("Could not derive session-end learnings: {}", error),
        }

        git::warn_uncommitted_work();

        Ok(Value::Object(serde_json::Map::new()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionLearningInput {
    agent_id: String,
    content: String,
    task: Option<String>,
    success: bool,
}

fn collect_session_learning_inputs(
    session_id: &str,
    transcript_path: Option<&Path>,
    extra: &Value,
) -> anyhow::Result<Vec<SessionLearningInput>> {
    let explicit_output = extra
        .get("output")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let explicit_task = extra
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let success = extra
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let explicit_agent = extra
        .get("agent_type")
        .and_then(Value::as_str)
        .map(normalize_agent_name)
        .filter(|value| !value.is_empty());

    let transcript_turns = transcript_path
        .and_then(|path| transcript::read_transcript_turns(path).ok())
        .unwrap_or_default();
    let conversation_text =
        explicit_output.unwrap_or_else(|| transcript::flatten_conversation(&transcript_turns));

    if conversation_text.trim().is_empty() || session_id.trim().is_empty() {
        return Ok(Vec::new());
    }

    let task = explicit_task.or_else(|| transcript::first_user_prompt(&transcript_turns));
    let agents = explicit_agent.map(|agent| vec![agent]).unwrap_or_else(|| {
        let detected = transcript::detect_agents_from_transcript(&transcript_turns);
        if detected.is_empty() {
            vec!["general".to_string()]
        } else {
            detected
        }
    });

    Ok(agents
        .into_iter()
        .map(|agent_id| SessionLearningInput {
            agent_id,
            content: conversation_text.chars().take(500).collect(),
            task: task.clone(),
            success,
        })
        .filter(|learning| !learning.content.trim().is_empty())
        .collect())
}
