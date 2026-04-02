//! Transcript parsing and conversation / redirect formatting for reflection.

use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReflectionMessage {
    pub(crate) role: String,
    pub(crate) content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RedirectRecord {
    pub(crate) redirect_number: Option<u64>,
    pub(crate) timestamp: Option<String>,
    pub(crate) failed_considerations: Vec<String>,
    pub(crate) continuation_prompt: String,
}

pub(crate) fn load_transcript_conversation(path: &Path) -> Result<Vec<ReflectionMessage>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read transcript {}", path.display()))?;
    let mut conversation = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry: Value = serde_json::from_str(trimmed)
            .with_context(|| format!("invalid transcript JSON in {}", path.display()))?;
        if let Some(message) = parse_reflection_message(&entry) {
            conversation.push(message);
        }
    }
    Ok(conversation)
}

fn parse_reflection_message(entry: &Value) -> Option<ReflectionMessage> {
    if let Some(role) = entry.get("role").and_then(Value::as_str) {
        return Some(ReflectionMessage {
            role: role.to_string(),
            content: extract_text_content(entry.get("content")?)?,
        });
    }

    let entry_type = entry.get("type").and_then(Value::as_str)?;
    if !matches!(entry_type, "user" | "assistant") {
        return None;
    }

    let message = entry.get("message")?;
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or(entry_type)
        .to_string();
    Some(ReflectionMessage {
        role,
        content: extract_text_content(message.get("content")?)?,
    })
}

fn extract_text_content(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
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
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        _ => None,
    }
}

pub(crate) fn format_conversation_summary(
    conversation: &[ReflectionMessage],
    max_length: usize,
) -> String {
    let mut summary_parts = Vec::new();
    let mut current_length = 0usize;

    for (index, message) in conversation.iter().enumerate() {
        let mut content = message.content.clone();
        if content.len() > 500 {
            content.truncate(497);
            content.push_str("...");
        }

        let snippet = format!(
            "\n**Message {} ({}):** {}\n",
            index + 1,
            message.role,
            content
        );

        if current_length + snippet.len() > max_length {
            summary_parts.push(format!(
                "\n[... {} more messages ...]",
                conversation.len().saturating_sub(index)
            ));
            break;
        }

        current_length += snippet.len();
        summary_parts.push(snippet);
    }

    summary_parts.join("")
}

pub(crate) fn load_power_steering_redirects(session_dir: &Path) -> Option<Vec<RedirectRecord>> {
    let path = session_dir.join("redirects.jsonl");
    let raw = fs::read_to_string(path).ok()?;
    let redirects = raw
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let entry: Value = serde_json::from_str(trimmed).ok()?;
            let failed_considerations = entry
                .get("failed_considerations")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(ToString::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let continuation_prompt = entry.get("continuation_prompt")?.as_str()?.to_string();
            Some(RedirectRecord {
                redirect_number: entry.get("redirect_number").and_then(Value::as_u64),
                timestamp: entry
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                failed_considerations,
                continuation_prompt,
            })
        })
        .collect::<Vec<_>>();
    (!redirects.is_empty()).then_some(redirects)
}

pub(crate) fn format_redirects_context(redirects: Option<Vec<RedirectRecord>>) -> String {
    let Some(redirects) = redirects else {
        return String::new();
    };

    let redirect_word = if redirects.len() == 1 {
        "redirect"
    } else {
        "redirects"
    };
    let mut parts = vec![
        String::new(),
        "## Power-Steering Redirect History".to_string(),
        String::new(),
        format!(
            "This session had {} power-steering {} where Claude was blocked from stopping due to incomplete work:",
            redirects.len(),
            redirect_word
        ),
        String::new(),
    ];

    for redirect in redirects {
        parts.push(format!(
            "### Redirect #{} ({})",
            redirect
                .redirect_number
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".to_string()),
            redirect.timestamp.unwrap_or_else(|| "unknown".to_string())
        ));
        parts.push(String::new());
        parts.push(format!(
            "**Failed Checks:** {}",
            redirect.failed_considerations.join(", ")
        ));
        parts.push(String::new());
        parts.push("**Continuation Prompt Given:**".to_string());
        parts.push("```".to_string());
        parts.push(redirect.continuation_prompt);
        parts.push("```".to_string());
        parts.push(String::new());
    }

    parts.push("**Analysis Note:** These redirects indicate areas where work was incomplete. In your feedback, consider whether the redirects were justified and whether Claude successfully addressed the blockers after being redirected.".to_string());
    parts.push(String::new());
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_transcript_conversation_parses_text_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("transcript.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Investigate auth"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done"}]}}
"#,
        )
        .unwrap();

        let conversation = load_transcript_conversation(&transcript).unwrap();

        assert_eq!(conversation.len(), 2);
        assert_eq!(conversation[0].content, "Investigate auth");
        assert_eq!(conversation[1].role, "assistant");
    }
}
