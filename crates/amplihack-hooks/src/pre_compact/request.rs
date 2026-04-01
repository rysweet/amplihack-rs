//! Original request extraction and prompt parsing from conversation/transcript data.

use crate::original_request::capture_original_request;
use amplihack_types::ProjectDirs;
use serde_json::Value;
use std::fs;
use std::path::Path;

pub(crate) fn preserve_original_request(
    dirs: &ProjectDirs,
    session_id: &str,
    transcript_path: Option<&Path>,
    extra: &Value,
) -> anyhow::Result<bool> {
    let prompt = extract_original_request_prompt(extra)
        .or_else(|| transcript_path.and_then(extract_original_request_prompt_from_transcript));
    let Some(prompt) = prompt else {
        return Ok(false);
    };
    Ok(capture_original_request(dirs, Some(session_id), &prompt)?.is_some())
}

fn extract_original_request_prompt(extra: &Value) -> Option<String> {
    [
        extra.get("conversation").and_then(Value::as_array),
        extra.get("messages").and_then(Value::as_array),
    ]
    .into_iter()
    .flatten()
    .find_map(|entries| {
        entries
            .iter()
            .filter_map(extract_user_prompt_from_entry)
            .find(|prompt| !prompt.trim().is_empty())
    })
}

fn extract_original_request_prompt_from_transcript(transcript_path: &Path) -> Option<String> {
    let contents = fs::read_to_string(transcript_path).ok()?;
    contents.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let entry = serde_json::from_str::<Value>(line).ok()?;
        extract_user_prompt_from_entry(&entry)
    })
}

fn extract_user_prompt_from_entry(entry: &Value) -> Option<String> {
    let object = entry.as_object()?;

    if let Some(message) = object.get("message").and_then(Value::as_object)
        && message.get("role").and_then(Value::as_str) == Some("user")
    {
        return extract_text(message.get("content").unwrap_or(&Value::Null));
    }

    if object.get("role").and_then(Value::as_str) == Some("user") {
        return extract_text(
            object
                .get("content")
                .or_else(|| object.get("text"))
                .unwrap_or(&Value::Null),
        );
    }

    if object.get("type").and_then(Value::as_str) == Some("user.message") {
        return extract_text(
            object
                .get("data")
                .and_then(|value| value.get("content"))
                .unwrap_or(&Value::Null),
        );
    }

    if object.get("type").and_then(Value::as_str) == Some("user") {
        return extract_text(
            object
                .get("content")
                .or_else(|| object.get("message").and_then(|value| value.get("content")))
                .unwrap_or(&Value::Null),
        );
    }

    if matches!(
        object.get("event").and_then(Value::as_str),
        Some("message") | Some("user_message")
    ) && object.get("role").and_then(Value::as_str) == Some("user")
    {
        return extract_text(object.get("content").unwrap_or(&Value::Null));
    }

    None
}

fn extract_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.to_string()),
        Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(extract_text)
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        Value::Object(map) => map
            .get("text")
            .or_else(|| map.get("value"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| map.get("content").and_then(extract_text)),
        _ => None,
    }
}
