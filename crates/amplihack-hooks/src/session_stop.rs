//! Session stop hook: stores session memory and warns about uncommitted work.
//!
//! Two responsibilities:
//! 1. Store session-end learnings natively in the Rust memory backends
//! 2. Check for uncommitted git changes and warn the user
//!
//! Neither blocks session exit (fail-open).

use crate::agent_memory::{
    detect_agent_references, detect_slash_command_agent, normalize_agent_name,
};
use crate::protocol::{FailurePolicy, Hook};
use amplihack_cli::memory::store_session_learning;
use amplihack_types::HookInput;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;

pub struct SessionStopHook;

impl Hook for SessionStopHook {
    fn name(&self) -> &'static str {
        "session_stop"
    }

    fn failure_policy(&self) -> FailurePolicy {
        // Don't block session exit on memory store failure.
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

        // 1. Store session memory natively.
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

        // 2. Check for uncommitted work and warn.
        warn_uncommitted_work();

        Ok(Value::Object(serde_json::Map::new()))
    }
}

/// Collected git status for uncommitted work.
struct GitStatus {
    staged: Vec<String>,
    unstaged: Vec<String>,
    untracked: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionLearningInput {
    agent_id: String,
    content: String,
    task: Option<String>,
    success: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TranscriptTurn {
    role: String,
    content: String,
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
        .and_then(|path| read_transcript_turns(path).ok())
        .unwrap_or_default();
    let conversation_text =
        explicit_output.unwrap_or_else(|| flatten_conversation(&transcript_turns));

    if conversation_text.trim().is_empty() || session_id.trim().is_empty() {
        return Ok(Vec::new());
    }

    let task = explicit_task.or_else(|| first_user_prompt(&transcript_turns));
    let agents = explicit_agent.map(|agent| vec![agent]).unwrap_or_else(|| {
        let detected = detect_agents_from_transcript(&transcript_turns);
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

fn read_transcript_turns(path: &Path) -> anyhow::Result<Vec<TranscriptTurn>> {
    let raw = fs::read_to_string(path)?;
    let mut turns = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry: Value = serde_json::from_str(trimmed)?;
        if let Some(turn) = parse_transcript_turn(&entry) {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn parse_transcript_turn(entry: &Value) -> Option<TranscriptTurn> {
    if let Some(role) = entry.get("role").and_then(Value::as_str) {
        return Some(TranscriptTurn {
            role: role.to_string(),
            content: extract_text_content(entry.get("content")?)?,
        });
    }

    let entry_type = entry.get("type").and_then(Value::as_str)?;
    if !matches!(entry_type, "user" | "assistant") {
        return None;
    }

    let message = entry.get("message")?;
    Some(TranscriptTurn {
        role: message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or(entry_type)
            .to_string(),
        content: extract_text_content(message.get("content")?)?,
    })
}

fn extract_text_content(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.trim().to_string()).filter(|text| !text.is_empty()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| {
                    if block.get("type").and_then(Value::as_str) == Some("text") {
                        block.get("text").and_then(Value::as_str)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            Some(text.trim().to_string()).filter(|text| !text.is_empty())
        }
        _ => None,
    }
}

fn detect_agents_from_transcript(turns: &[TranscriptTurn]) -> Vec<String> {
    let mut agents = Vec::new();
    for turn in turns {
        if turn.role != "user" {
            continue;
        }
        for agent in detect_agent_references(&turn.content) {
            if !agents.iter().any(|existing| existing == &agent) {
                agents.push(agent);
            }
        }
        if let Some(agent) = detect_slash_command_agent(&turn.content) {
            let normalized = normalize_agent_name(agent);
            if !agents.iter().any(|existing| existing == &normalized) {
                agents.push(normalized);
            }
        }
    }
    agents
}

fn flatten_conversation(turns: &[TranscriptTurn]) -> String {
    turns
        .iter()
        .filter(|turn| !turn.content.trim().is_empty())
        .map(|turn| format!("{}: {}", turn.role, turn.content.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn first_user_prompt(turns: &[TranscriptTurn]) -> Option<String> {
    turns.iter().find_map(|turn| {
        (turn.role == "user" && !turn.content.trim().is_empty()).then(|| turn.content.clone())
    })
}

/// Gather git status (staged, unstaged, untracked files).
///
/// Returns `None` if git is unavailable or not in a repo.
fn get_git_status() -> Option<GitStatus> {
    let staged = match Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect::<Vec<_>>(),
        _ => return None,
    };

    let unstaged = match Command::new("git").args(["diff", "--name-only"]).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

    let untracked = match Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

    Some(GitStatus {
        staged,
        unstaged,
        untracked,
    })
}

/// Check git status and print warnings about uncommitted changes.
///
/// Best-effort: never blocks session exit.
fn warn_uncommitted_work() {
    let status = match get_git_status() {
        Some(s) => s,
        None => return,
    };

    let GitStatus {
        staged,
        unstaged,
        untracked,
    } = status;

    if staged.is_empty() && unstaged.is_empty() && untracked.is_empty() {
        return;
    }

    eprintln!("\n⚠️  Uncommitted work detected:");

    if !staged.is_empty() {
        eprintln!(
            "\n  Staged ({} file{}):",
            staged.len(),
            if staged.len() == 1 { "" } else { "s" }
        );
        for f in staged.iter().take(10) {
            eprintln!("    ✅ {f}");
        }
        if staged.len() > 10 {
            eprintln!("    ... and {} more", staged.len() - 10);
        }
    }

    if !unstaged.is_empty() {
        eprintln!(
            "\n  Modified ({} file{}):",
            unstaged.len(),
            if unstaged.len() == 1 { "" } else { "s" }
        );
        for f in unstaged.iter().take(10) {
            eprintln!("    📝 {f}");
        }
        if unstaged.len() > 10 {
            eprintln!("    ... and {} more", unstaged.len() - 10);
        }
    }

    if !untracked.is_empty() {
        eprintln!(
            "\n  Untracked ({} file{}):",
            untracked.len(),
            if untracked.len() == 1 { "" } else { "s" }
        );
        for f in untracked.iter().take(10) {
            eprintln!("    ❓ {f}");
        }
        if untracked.len() > 10 {
            eprintln!("    ... and {} more", untracked.len() - 10);
        }
    }

    let total = staged.len() + unstaged.len() + untracked.len();
    eprintln!("\n  💡 To commit: git add -A && git commit -m \"save work\"");
    eprintln!("  💡 To stash:  git stash push -m \"session work\"");
    eprintln!(
        "  📊 Total: {total} file{} with uncommitted changes\n",
        if total == 1 { "" } else { "s" }
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;

    #[test]
    fn handles_unknown_events() {
        let hook = SessionStopHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn warn_uncommitted_work_does_not_panic() {
        // Just verify it doesn't panic in test environment.
        warn_uncommitted_work();
    }

    #[test]
    fn read_transcript_turns_parses_message_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("transcript.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Use /analyze auth"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done"}]}}
"#,
        )
        .unwrap();

        let turns = read_transcript_turns(&transcript).unwrap();

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].content, "Use /analyze auth");
        assert_eq!(turns[1].role, "assistant");
    }

    #[test]
    fn detect_agents_from_transcript_uses_user_turns() {
        let turns = vec![
            TranscriptTurn {
                role: "user".to_string(),
                content: "Please /analyze auth flow".to_string(),
            },
            TranscriptTurn {
                role: "assistant".to_string(),
                content: "I am using the analyzer agent".to_string(),
            },
        ];

        let agents = detect_agents_from_transcript(&turns);

        assert_eq!(agents, vec!["analyzer".to_string()]);
    }

    #[test]
    fn session_stop_stores_learning_in_sqlite_backend() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev_home = std::env::var_os("HOME");
        let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("AMPLIHACK_MEMORY_BACKEND", "sqlite");
        }

        let transcript = dir.path().join("session.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Use /analyze the auth flow"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Found the failing middleware and proposed a fix."}]}}
"#,
        )
        .unwrap();

        let hook = SessionStopHook;
        let result = hook
            .process(HookInput::SessionStop {
                session_id: Some("session-stop-test".to_string()),
                transcript_path: Some(transcript),
                extra: serde_json::json!({}),
            })
            .unwrap();

        let memories = amplihack_cli::memory::retrieve_prompt_context_memories(
            "session-stop-test",
            "auth flow",
            2000,
        )
        .unwrap();

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match prev_backend {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
        }

        assert!(result.as_object().unwrap().is_empty());
        assert_eq!(memories.len(), 1);
        assert!(memories[0].content.contains("Found the failing middleware"));
    }
}
