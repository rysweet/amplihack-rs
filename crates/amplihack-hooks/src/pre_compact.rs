//! Pre-compact hook: exports conversation transcript before context compaction.
//!
//! When the context window is about to be compacted, this hook:
//! 1. Exports the current conversation to a JSONL transcript file
//! 2. Extracts the original request for context preservation
//! 3. Saves compaction metadata

use crate::original_request::capture_original_request;
use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::{HookInput, ProjectDirs};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct PreCompactHook;

impl Hook for PreCompactHook {
    fn name(&self) -> &'static str {
        "pre_compact"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (session_id, transcript_path, extra) = match input {
            HookInput::PreCompact {
                session_id,
                transcript_path,
                extra,
            } => (session_id, transcript_path, extra),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        let session_id = session_id.unwrap_or_else(generate_session_id);
        let dirs = ProjectDirs::from_cwd();

        let exported_path = match transcript_path.as_deref() {
            Some(path) => match export_transcript(path, &session_id) {
                Ok(path) => path,
                Err(error) => return Ok(error_response("Failed to export transcript", &error)),
            },
            None => None,
        };

        let original_request_preserved =
            match preserve_original_request(&dirs, &session_id, transcript_path.as_deref(), &extra)
            {
                Ok(preserved) => preserved,
                Err(error) => {
                    tracing::warn!(
                        session_id,
                        error = %error,
                        "failed to preserve original request during pre-compact"
                    );
                    false
                }
            };

        let metadata = match save_compaction_metadata(
            &session_id,
            exported_path.as_deref(),
            &extra,
            original_request_preserved,
        ) {
            Ok(metadata) => metadata,
            Err(error) => {
                return Ok(error_response("Failed to save compaction metadata", &error));
            }
        };

        let message = match exported_path.as_ref() {
            Some(path) => format!(
                "Conversation exported successfully - transcript preserved at {}",
                path.display()
            ),
            None => "Pre-compact metadata saved successfully".to_string(),
        };

        Ok(serde_json::json!({
            "status": "success",
            "message": message,
            "transcript_path": exported_path.map(|path| path.display().to_string()),
            "metadata": metadata,
        }))
    }
}

fn error_response(context: &str, error: &anyhow::Error) -> Value {
    let message = format!("{context}: {error}");
    tracing::warn!("{}", message);
    serde_json::json!({
        "status": "error",
        "message": message,
        "error": error.to_string(),
    })
}

fn export_transcript(transcript_path: &Path, session_id: &str) -> anyhow::Result<Option<PathBuf>> {
    let dirs = ProjectDirs::from_cwd();
    let export_dir = dirs.session_logs(session_id);
    fs::create_dir_all(&export_dir)?;

    if !transcript_path.exists() {
        return Ok(None);
    }

    let export_path = export_dir.join("transcript_pre_compact.jsonl");
    fs::copy(transcript_path, &export_path)?;
    tracing::info!("Exported transcript to {}", export_path.display());
    Ok(Some(export_path))
}

fn save_compaction_metadata(
    session_id: &str,
    transcript_path: Option<&Path>,
    extra: &Value,
    original_request_preserved: bool,
) -> anyhow::Result<Value> {
    let dirs = ProjectDirs::from_cwd();
    let session_dir = dirs.session_logs(session_id);
    fs::create_dir_all(&session_dir)?;

    let mut metadata = serde_json::json!({
        "event": "pre_compact",
        "session_id": session_id,
        "timestamp": now_epoch_secs(),
        "transcript_path": transcript_path.map(|p| p.display().to_string()),
        "original_request_preserved": original_request_preserved,
    });
    if let Some(trigger) = extra.get("trigger").and_then(Value::as_str) {
        metadata["compaction_trigger"] = Value::String(trigger.to_string());
    }

    let metadata_file = session_dir.join("compaction_metadata.jsonl");
    let json = serde_json::to_string(&metadata)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(metadata_file)?;
    writeln!(file, "{}", json)?;

    Ok(metadata)
}

fn generate_session_id() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("session-{}", now.as_secs())
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn preserve_original_request(
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
    use crate::test_support::env_lock;

    #[test]
    fn handles_unknown_events() {
        let hook = PreCompactHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn generates_session_id() {
        let id = generate_session_id();
        assert!(id.starts_with("session-"));
    }

    #[test]
    fn pre_compact_returns_success_and_exports_transcript() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let transcript = dir.path().join("input.jsonl");
        fs::write(&transcript, "{\"type\":\"user\",\"message\":\"hello\"}\n").unwrap();

        let hook = PreCompactHook;
        let result = hook
            .process(HookInput::PreCompact {
                session_id: Some("test-session".to_string()),
                transcript_path: Some(transcript.clone()),
                extra: Value::Null,
            })
            .unwrap();

        let _ = std::env::set_current_dir(&original);

        assert_eq!(result["status"], "success");
        assert!(
            result["message"]
                .as_str()
                .unwrap()
                .contains("Conversation exported successfully")
        );
        assert_eq!(result["metadata"]["event"], "pre_compact");
        let exported = PathBuf::from(result["transcript_path"].as_str().unwrap());
        assert!(exported.exists());
        assert!(
            dir.path()
                .join(".claude/runtime/logs/test-session/compaction_metadata.jsonl")
                .exists()
        );
        assert_eq!(result["metadata"]["original_request_preserved"], false);
    }

    #[test]
    fn pre_compact_preserves_original_request_from_conversation_payload() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let hook = PreCompactHook;
        let result = hook
            .process(HookInput::PreCompact {
                session_id: Some("compact-session".to_string()),
                transcript_path: None,
                extra: serde_json::json!({
                    "trigger": "token_limit",
                    "conversation": [
                        {
                            "role": "user",
                            "content": "Implement complete hook parity. Do not regress tests. Ensure every user-visible hook output matches Python."
                        }
                    ]
                }),
            })
            .unwrap();

        let _ = std::env::set_current_dir(&original);

        assert_eq!(result["status"], "success");
        assert_eq!(result["metadata"]["original_request_preserved"], true);
        assert_eq!(result["metadata"]["compaction_trigger"], "token_limit");
        assert!(
            dir.path()
                .join(".claude/runtime/logs/compact-session/ORIGINAL_REQUEST.md")
                .exists()
        );
        assert!(
            dir.path()
                .join(".claude/runtime/logs/compact-session/original_request.json")
                .exists()
        );
    }

    #[test]
    fn pre_compact_preserves_original_request_from_transcript_jsonl() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let transcript = dir.path().join("copilot-events.jsonl");
        fs::write(
            &transcript,
            concat!(
                "{\"type\":\"user.message\",\"data\":{\"content\":\"Implement complete hook parity. Do not regress tests. Ensure every user-visible hook output matches Python.\"}}\n",
                "{\"type\":\"assistant.message\",\"data\":{\"content\":\"Working on it.\"}}\n"
            ),
        )
        .unwrap();

        let hook = PreCompactHook;
        let result = hook
            .process(HookInput::PreCompact {
                session_id: Some("compact-transcript".to_string()),
                transcript_path: Some(transcript),
                extra: Value::Null,
            })
            .unwrap();

        let _ = std::env::set_current_dir(&original);

        assert_eq!(result["status"], "success");
        assert_eq!(result["metadata"]["original_request_preserved"], true);
        assert!(
            dir.path()
                .join(".claude/runtime/logs/compact-transcript/ORIGINAL_REQUEST.md")
                .exists()
        );
    }
}
