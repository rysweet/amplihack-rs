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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // extract_text
    // -----------------------------------------------------------------------

    #[test]
    fn extract_text_string() {
        let v = json!("hello world");
        assert_eq!(extract_text(&v), Some("hello world".into()));
    }

    #[test]
    fn extract_text_array_of_strings() {
        let v = json!(["line 1", "line 2"]);
        assert_eq!(extract_text(&v), Some("line 1\nline 2".into()));
    }

    #[test]
    fn extract_text_array_with_objects() {
        let v = json!([{"text": "part 1"}, {"value": "part 2"}]);
        assert_eq!(extract_text(&v), Some("part 1\npart 2".into()));
    }

    #[test]
    fn extract_text_object_with_text_key() {
        let v = json!({"text": "inner"});
        assert_eq!(extract_text(&v), Some("inner".into()));
    }

    #[test]
    fn extract_text_object_with_content_key() {
        let v = json!({"content": "nested"});
        assert_eq!(extract_text(&v), Some("nested".into()));
    }

    #[test]
    fn extract_text_null() {
        assert_eq!(extract_text(&Value::Null), None);
    }

    #[test]
    fn extract_text_empty_array() {
        assert_eq!(extract_text(&json!([])), None);
    }

    // -----------------------------------------------------------------------
    // extract_user_prompt_from_entry
    // -----------------------------------------------------------------------

    #[test]
    fn entry_with_role_user() {
        let entry = json!({"role": "user", "content": "Build a feature"});
        assert_eq!(
            extract_user_prompt_from_entry(&entry),
            Some("Build a feature".into())
        );
    }

    #[test]
    fn entry_with_nested_message() {
        let entry = json!({
            "message": {"role": "user", "content": "Fix the bug"}
        });
        assert_eq!(
            extract_user_prompt_from_entry(&entry),
            Some("Fix the bug".into())
        );
    }

    #[test]
    fn entry_type_user_message() {
        let entry = json!({
            "type": "user.message",
            "data": {"content": "Analyze this"}
        });
        assert_eq!(
            extract_user_prompt_from_entry(&entry),
            Some("Analyze this".into())
        );
    }

    #[test]
    fn entry_type_user() {
        let entry = json!({"type": "user", "content": "Hello"});
        assert_eq!(extract_user_prompt_from_entry(&entry), Some("Hello".into()));
    }

    #[test]
    fn entry_assistant_returns_none() {
        let entry = json!({"role": "assistant", "content": "Response"});
        assert_eq!(extract_user_prompt_from_entry(&entry), None);
    }

    #[test]
    fn entry_event_message_user() {
        let entry = json!({
            "event": "message",
            "role": "user",
            "content": "Event prompt"
        });
        assert_eq!(
            extract_user_prompt_from_entry(&entry),
            Some("Event prompt".into())
        );
    }

    // -----------------------------------------------------------------------
    // extract_original_request_prompt
    // -----------------------------------------------------------------------

    #[test]
    fn prompt_from_conversation() {
        let extra = json!({
            "conversation": [
                {"role": "user", "content": "First message"}
            ]
        });
        assert_eq!(
            extract_original_request_prompt(&extra),
            Some("First message".into())
        );
    }

    #[test]
    fn prompt_from_messages() {
        let extra = json!({
            "messages": [
                {"role": "user", "content": "From messages"}
            ]
        });
        assert_eq!(
            extract_original_request_prompt(&extra),
            Some("From messages".into())
        );
    }

    #[test]
    fn prompt_empty_extra() {
        let extra = json!({});
        assert_eq!(extract_original_request_prompt(&extra), None);
    }

    #[test]
    fn prompt_skips_empty_user_content() {
        let extra = json!({
            "conversation": [
                {"role": "user", "content": ""},
                {"role": "user", "content": "Real prompt"}
            ]
        });
        assert_eq!(
            extract_original_request_prompt(&extra),
            Some("Real prompt".into())
        );
    }
}
