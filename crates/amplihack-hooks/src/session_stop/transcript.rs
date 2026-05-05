//! Transcript parsing, agent detection, and conversation utilities.

use crate::agent_memory::{
    detect_agent_references, detect_slash_command_agent, normalize_agent_name,
};
use serde_json::Value;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TranscriptTurn {
    pub role: String,
    pub content: String,
}

pub(super) fn read_transcript_turns(path: &Path) -> anyhow::Result<Vec<TranscriptTurn>> {
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

pub(super) fn detect_agents_from_transcript(turns: &[TranscriptTurn]) -> Vec<String> {
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

pub(super) fn flatten_conversation(turns: &[TranscriptTurn]) -> String {
    turns
        .iter()
        .filter(|turn| !turn.content.trim().is_empty())
        .map(|turn| format!("{}: {}", turn.role, turn.content.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) fn first_user_prompt(turns: &[TranscriptTurn]) -> Option<String> {
    turns.iter().find_map(|turn| {
        (turn.role == "user" && !turn.content.trim().is_empty()).then(|| turn.content.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn turn(role: &str, content: &str) -> TranscriptTurn {
        TranscriptTurn {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    // --- extract_text_content ---

    #[test]
    fn extract_text_content_string() {
        let v = json!("hello world");
        assert_eq!(extract_text_content(&v), Some("hello world".into()));
    }

    #[test]
    fn extract_text_content_empty_string() {
        assert_eq!(extract_text_content(&json!("")), None);
        assert_eq!(extract_text_content(&json!("   ")), None);
    }

    #[test]
    fn extract_text_content_array_blocks() {
        let v = json!([
            {"type": "text", "text": "line 1"},
            {"type": "image", "url": "pic.png"},
            {"type": "text", "text": "line 2"}
        ]);
        assert_eq!(extract_text_content(&v), Some("line 1\nline 2".into()));
    }

    #[test]
    fn extract_text_content_empty_array() {
        assert_eq!(extract_text_content(&json!([])), None);
    }

    #[test]
    fn extract_text_content_non_string_non_array() {
        assert_eq!(extract_text_content(&json!(42)), None);
        assert_eq!(extract_text_content(&json!(null)), None);
        assert_eq!(extract_text_content(&json!(true)), None);
    }

    // --- parse_transcript_turn ---

    #[test]
    fn parse_turn_role_format() {
        let entry = json!({"role": "user", "content": "hello"});
        let t = parse_transcript_turn(&entry).unwrap();
        assert_eq!(t.role, "user");
        assert_eq!(t.content, "hello");
    }

    #[test]
    fn parse_turn_type_message_format() {
        let entry = json!({
            "type": "user",
            "message": {"role": "human", "content": "question?"}
        });
        let t = parse_transcript_turn(&entry).unwrap();
        assert_eq!(t.role, "human");
        assert_eq!(t.content, "question?");
    }

    #[test]
    fn parse_turn_type_message_falls_back_to_entry_type() {
        let entry = json!({
            "type": "assistant",
            "message": {"content": "answer"}
        });
        let t = parse_transcript_turn(&entry).unwrap();
        assert_eq!(t.role, "assistant");
    }

    #[test]
    fn parse_turn_ignores_non_user_assistant_types() {
        let entry = json!({"type": "system", "message": {"content": "sys"}});
        assert!(parse_transcript_turn(&entry).is_none());
    }

    #[test]
    fn parse_turn_empty_content_returns_none() {
        let entry = json!({"role": "user", "content": ""});
        assert!(parse_transcript_turn(&entry).is_none());
    }

    // --- read_transcript_turns ---

    #[test]
    fn read_transcript_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = format!(
            "{}\n{}\n",
            json!({"role": "user", "content": "hi"}),
            json!({"role": "assistant", "content": "hello"})
        );
        fs::write(&path, content).unwrap();

        let turns = read_transcript_turns(&path).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[1].role, "assistant");
    }

    #[test]
    fn read_transcript_skips_blank_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = format!(
            "{}\n\n  \n{}\n",
            json!({"role": "user", "content": "a"}),
            json!({"role": "assistant", "content": "b"})
        );
        fs::write(&path, content).unwrap();
        let turns = read_transcript_turns(&path).unwrap();
        assert_eq!(turns.len(), 2);
    }

    #[test]
    fn read_transcript_missing_file() {
        assert!(read_transcript_turns(Path::new("/nonexistent/file.jsonl")).is_err());
    }

    #[test]
    fn read_transcript_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.jsonl");
        fs::write(&path, "not json\n").unwrap();
        assert!(read_transcript_turns(&path).is_err());
    }

    // --- flatten_conversation ---

    #[test]
    fn flatten_conversation_basic() {
        let turns = vec![turn("user", "hi"), turn("assistant", "hello")];
        let flat = flatten_conversation(&turns);
        assert_eq!(flat, "user: hi\n\nassistant: hello");
    }

    #[test]
    fn flatten_conversation_skips_empty() {
        let turns = vec![
            turn("user", "hi"),
            turn("assistant", "  "),
            turn("user", "bye"),
        ];
        let flat = flatten_conversation(&turns);
        assert_eq!(flat, "user: hi\n\nuser: bye");
    }

    #[test]
    fn flatten_conversation_empty_input() {
        assert_eq!(flatten_conversation(&[]), "");
    }

    // --- first_user_prompt ---

    #[test]
    fn first_user_prompt_basic() {
        let turns = vec![
            turn("assistant", "welcome"),
            turn("user", "my question"),
            turn("user", "follow up"),
        ];
        assert_eq!(first_user_prompt(&turns), Some("my question".into()));
    }

    #[test]
    fn first_user_prompt_skips_empty() {
        let turns = vec![turn("user", "  "), turn("user", "real prompt")];
        assert_eq!(first_user_prompt(&turns), Some("real prompt".into()));
    }

    #[test]
    fn first_user_prompt_no_user_turns() {
        let turns = vec![turn("assistant", "hello")];
        assert_eq!(first_user_prompt(&turns), None);
    }

    #[test]
    fn first_user_prompt_empty() {
        assert_eq!(first_user_prompt(&[]), None);
    }
}
