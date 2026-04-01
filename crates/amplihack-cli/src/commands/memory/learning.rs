use anyhow::Result;
use sha2::{Digest, Sha256};

use super::backend;
use super::prompt_context::{
    resolve_runtime_memory_backend_choice, retrieve_prompt_context_memories_from_backend,
};
use super::types::{BackendChoice, PromptContextMemory, SessionLearningRecord};

pub fn retrieve_prompt_context_memories(
    session_id: &str,
    query_text: &str,
    token_budget: usize,
) -> Result<Vec<PromptContextMemory>> {
    if session_id.trim().is_empty() || query_text.trim().is_empty() || token_budget == 0 {
        return Ok(Vec::new());
    }

    let choice = resolve_runtime_memory_backend_choice()?;
    retrieve_prompt_context_memories_from_backend(choice, session_id, query_text, token_budget)
}

pub(super) fn build_memory_id(record: &SessionLearningRecord, timestamp: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(record.session_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(record.agent_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(record.content.as_bytes());
    hasher.update(b"\0");
    hasher.update(timestamp.as_bytes());
    let digest = hasher.finalize();
    format!("mem-{:x}", digest)
}

fn heuristic_importance(content: &str) -> i64 {
    // Byte length as a proxy for character count — same result for ASCII,
    // slight overestimate for multibyte UTF-8, and O(1).
    let len = content.trim().len();
    match len {
        0..=99 => 5,
        100..=199 => 6,
        _ => 7,
    }
}

pub(super) fn build_learning_record(
    session_id: &str,
    agent_id: &str,
    content: &str,
    task: Option<&str>,
    success: bool,
) -> Option<SessionLearningRecord> {
    let trimmed = content.trim();
    if trimmed.len() < 10 {
        return None;
    }

    let summary = trimmed.chars().take(500).collect::<String>();
    let title = summary.chars().take(50).collect::<String>();
    let project_id =
        std::env::var("AMPLIHACK_PROJECT_ID").unwrap_or_else(|_| "amplihack".to_string());
    Some(SessionLearningRecord {
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        content: format!("Agent {agent_id}: {summary}"),
        title: title.trim().to_string(),
        importance: heuristic_importance(trimmed),
        metadata: serde_json::json!({
            "new_memory_type": "semantic",
            "tags": ["learning", "session_end"],
            "task": task.unwrap_or_default(),
            "success": success,
            "project_id": project_id,
            "agent_type": agent_id,
        }),
    })
}

pub fn store_session_learning(
    session_id: &str,
    agent_id: &str,
    content: &str,
    task: Option<&str>,
    success: bool,
) -> Result<Option<String>> {
    let Some(record) = build_learning_record(session_id, agent_id, content, task, success) else {
        return Ok(None);
    };

    let choice = resolve_runtime_memory_backend_choice()?;
    store_learning_with_backend(choice, &record)
}

pub(super) fn store_learning_with_backend(
    choice: BackendChoice,
    record: &SessionLearningRecord,
) -> Result<Option<String>> {
    backend::open_runtime_backend(choice)?.store_session_learning(record)
}
