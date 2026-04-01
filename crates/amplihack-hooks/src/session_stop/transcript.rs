//! Transcript parsing, agent detection, and conversation utilities.

use crate::agent_memory::{detect_agent_references, detect_slash_command_agent, normalize_agent_name};
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
