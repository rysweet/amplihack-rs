//! Agent memory injection and framework file detection.

use crate::agent_memory::{detect_agent_references, detect_slash_command_agent};
use amplihack_cli::memory::{PromptContextMemory, retrieve_prompt_context_memories};
use amplihack_types::ProjectDirs;
use std::fs;
use std::path::PathBuf;

pub(crate) fn inject_memory(prompt: &str, session_id: Option<&str>) -> Option<String> {
    let mut agent_types = detect_agent_references(prompt);
    if let Some(agent) = detect_slash_command_agent(prompt)
        && !agent_types.iter().any(|existing| existing == agent)
    {
        agent_types.push(agent.to_string());
    }

    if agent_types.is_empty() {
        return None;
    }

    let session_id_owned: String;
    let session_id = match session_id.filter(|value| !value.trim().is_empty()) {
        Some(id) => id,
        None => {
            session_id_owned = generate_unique_session_id();
            tracing::warn!(
                "No session_id provided to user_prompt memory hook; using generated fallback: {}",
                session_id_owned
            );
            &session_id_owned
        }
    };
    let query_text = prompt.chars().take(500).collect::<String>();

    match retrieve_prompt_context_memories(session_id, &query_text, 2000) {
        Ok(memories) if !memories.is_empty() => {
            Some(format_agent_memory_context(&agent_types, &memories))
        }
        Ok(_) => None,
        Err(error) => {
            tracing::warn!("Memory injection failed: {}", error);
            None
        }
    }
}

pub fn format_agent_memory_context(
    agent_types: &[String],
    memories: &[PromptContextMemory],
) -> String {
    agent_types
        .iter()
        .map(|agent_type| {
            let mut lines = vec![format!("\n## Memory for {} Agent\n", agent_type)];
            for memory in memories {
                lines.push(format!("- {} (relevance: 0.00)", memory.content));
                if let Some(code_context) = memory.code_context.as_deref()
                    && !code_context.trim().is_empty()
                {
                    lines.push(code_context.to_string());
                }
            }
            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate a unique session ID from PID and current timestamp.
fn generate_unique_session_id() -> String {
    format!(
        "hook-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    )
}

/// Check if AMPLIHACK.md should be injected (differs from CLAUDE.md).
pub(crate) fn check_framework_injection(dirs: &ProjectDirs) -> Option<String> {
    let amplihack_path = find_amplihack_md(dirs)?;
    let claude_path = dirs.claude_md();

    let amplihack_content = fs::read_to_string(&amplihack_path).ok()?;
    let claude_content = fs::read_to_string(&claude_path).ok().unwrap_or_default();

    // Normalize whitespace for comparison.
    let norm_amplihack: String = amplihack_content.split_whitespace().collect();
    let norm_claude: String = claude_content.split_whitespace().collect();

    if norm_amplihack == norm_claude {
        return None; // Already identical.
    }

    Some(amplihack_content)
}

fn find_amplihack_md(dirs: &ProjectDirs) -> Option<PathBuf> {
    // Check CLAUDE_PLUGIN_ROOT env var first.
    if let Ok(root) = std::env::var("CLAUDE_PLUGIN_ROOT") {
        let path = PathBuf::from(root).join("AMPLIHACK.md");
        if path.exists() {
            return Some(path);
        }
    }

    // Check .claude/AMPLIHACK.md.
    let path = dirs.amplihack_md();
    if path.exists() {
        return Some(path);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_single_agent_with_memories() {
        let agents = vec!["builder".to_string()];
        let memories = vec![PromptContextMemory {
            content: "Always use cargo test".to_string(),
            code_context: None,
        }];
        let result = format_agent_memory_context(&agents, &memories);
        assert!(result.contains("## Memory for builder Agent"));
        assert!(result.contains("Always use cargo test"));
        assert!(result.contains("relevance: 0.00"));
    }

    #[test]
    fn format_multiple_agents() {
        let agents = vec!["builder".to_string(), "reviewer".to_string()];
        let memories = vec![PromptContextMemory {
            content: "Test fact".to_string(),
            code_context: None,
        }];
        let result = format_agent_memory_context(&agents, &memories);
        assert!(result.contains("builder Agent"));
        assert!(result.contains("reviewer Agent"));
    }

    #[test]
    fn format_with_code_context() {
        let agents = vec!["builder".to_string()];
        let memories = vec![PromptContextMemory {
            content: "Important fact".to_string(),
            code_context: Some("fn main() {}".to_string()),
        }];
        let result = format_agent_memory_context(&agents, &memories);
        assert!(result.contains("fn main() {}"));
    }

    #[test]
    fn format_empty_code_context_skipped() {
        let agents = vec!["builder".to_string()];
        let memories = vec![PromptContextMemory {
            content: "Fact".to_string(),
            code_context: Some("  ".to_string()),
        }];
        let result = format_agent_memory_context(&agents, &memories);
        // Whitespace-only code_context should be skipped
        assert!(!result.contains("  \n"));
    }

    #[test]
    fn format_empty_memories() {
        let agents = vec!["builder".to_string()];
        let memories: Vec<PromptContextMemory> = vec![];
        let result = format_agent_memory_context(&agents, &memories);
        assert!(result.contains("builder Agent"));
        // No memory lines beyond the header
        assert!(!result.contains("relevance"));
    }
}
