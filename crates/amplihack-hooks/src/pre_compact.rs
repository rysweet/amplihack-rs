//! Pre-compact hook: exports conversation transcript before context compaction.
//!
//! When the context window is about to be compacted, this hook:
//! 1. Exports the current conversation to a JSONL transcript file
//! 2. Extracts the original request for context preservation
//! 3. Saves compaction metadata

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
        let session_dir = dirs.session_logs(&session_id);
        fs::create_dir_all(&session_dir)?;

        // Export transcript if path provided.
        if let Some(ref path) = transcript_path
            && let Err(e) = export_transcript(path, &session_id)
        {
            tracing::warn!("Failed to export transcript: {}", e);
        }

        let conversation_entries = extract_conversation_entries(&extra);
        if !conversation_entries.is_empty()
            && let Err(e) = export_legacy_conversation_artifacts(
                &session_dir,
                &session_id,
                &conversation_entries,
                extra.get("trigger").and_then(Value::as_str),
            )
        {
            tracing::warn!("Failed to export legacy pre-compact artifacts: {}", e);
        }

        // Save compaction metadata.
        if let Err(e) = save_compaction_metadata(&session_id, transcript_path.as_deref()) {
            tracing::warn!("Failed to save compaction metadata: {}", e);
        }

        Ok(Value::Object(serde_json::Map::new()))
    }
}

fn export_transcript(transcript_path: &std::path::Path, session_id: &str) -> anyhow::Result<()> {
    let dirs = ProjectDirs::from_cwd();
    let export_dir = dirs.session_logs(session_id);
    fs::create_dir_all(&export_dir)?;

    let export_path = export_dir.join("transcript_pre_compact.jsonl");

    if transcript_path.exists() {
        fs::copy(transcript_path, &export_path)?;
        tracing::info!("Exported transcript to {}", export_path.display());
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct ConversationEntry {
    role: String,
    timestamp: String,
    text: String,
}

fn extract_conversation_entries(extra: &Value) -> Vec<ConversationEntry> {
    let items = extra
        .get("conversation")
        .and_then(Value::as_array)
        .or_else(|| extra.get("messages").and_then(Value::as_array));

    items
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let object = item.as_object()?;
            let role = object
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_string();
            let timestamp = object
                .get("timestamp")
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_string();
            let text = object
                .get("text")
                .or_else(|| object.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();

            Some(ConversationEntry {
                role,
                timestamp,
                text,
            })
        })
        .collect()
}

fn export_legacy_conversation_artifacts(
    session_dir: &Path,
    session_id: &str,
    entries: &[ConversationEntry],
    trigger: Option<&str>,
) -> anyhow::Result<()> {
    let transcript_path = export_conversation_markdown(session_dir, session_id, entries)?;
    write_transcript_copy(session_dir, &transcript_path)?;
    let original_request_preserved = save_original_request(session_dir, session_id, entries)?;
    append_compaction_event(
        session_dir,
        session_id,
        entries.len(),
        &transcript_path,
        original_request_preserved,
        trigger.unwrap_or("unknown"),
    )?;
    Ok(())
}

fn export_conversation_markdown(
    session_dir: &Path,
    session_id: &str,
    entries: &[ConversationEntry],
) -> anyhow::Result<PathBuf> {
    let transcript_file = session_dir.join("CONVERSATION_TRANSCRIPT.md");
    let mut content = vec![
        format!("# Conversation Transcript - Session {session_id}"),
        String::new(),
        format!("**Exported**: {}", now_epoch_secs()),
        format!("**Messages**: {}", entries.len()),
        String::new(),
        "━".repeat(80),
        String::new(),
    ];

    for (index, entry) in entries.iter().enumerate() {
        content.push(format!("## Message {} - {}", index + 1, entry.role.to_uppercase()));
        content.push(format!("**Timestamp**: {}", entry.timestamp));
        content.push(String::new());
        content.push(entry.text.clone());
        content.push(String::new());
        content.push("─".repeat(40));
        content.push(String::new());
    }

    fs::write(&transcript_file, content.join("\n"))?;
    Ok(transcript_file)
}

fn write_transcript_copy(session_dir: &Path, transcript_path: &Path) -> anyhow::Result<()> {
    let transcripts_dir = session_dir.join("transcripts");
    fs::create_dir_all(&transcripts_dir)?;
    let copy_path = transcripts_dir.join(format!("conversation_{}.md", now_epoch_secs()));
    fs::copy(transcript_path, copy_path)?;
    Ok(())
}

fn save_original_request(
    session_dir: &Path,
    session_id: &str,
    entries: &[ConversationEntry],
) -> anyhow::Result<bool> {
    let Some(prompt) = entries
        .iter()
        .find(|entry| entry.role.eq_ignore_ascii_case("user") && entry.text.trim().len() > 50)
        .map(|entry| entry.text.trim().to_string())
    else {
        return Ok(false);
    };

    let target = infer_target(&prompt);
    let request_markdown = format!(
        "# Original User Request\n\n\
**Session**: {session_id}\n\
**Captured**: {}\n\
**Target**: {target}\n\n\
## Raw Request\n\
```\n\
{prompt}\n\
```\n",
        now_epoch_secs()
    );
    fs::write(session_dir.join("ORIGINAL_REQUEST.md"), request_markdown)?;

    let request_json = serde_json::json!({
        "session_id": session_id,
        "raw_prompt": prompt,
        "target": target,
        "word_count": prompt.split_whitespace().count(),
        "char_count": prompt.chars().count(),
        "captured_at": now_epoch_secs(),
    });
    fs::write(
        session_dir.join("original_request.json"),
        serde_json::to_string_pretty(&request_json)?,
    )?;

    Ok(true)
}

fn infer_target(prompt: &str) -> String {
    prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| "General development task".to_string())
}

fn append_compaction_event(
    session_dir: &Path,
    session_id: &str,
    message_count: usize,
    transcript_path: &Path,
    original_request_preserved: bool,
    trigger: &str,
) -> anyhow::Result<()> {
    let metadata_path = session_dir.join("compaction_events.json");
    let mut events = if metadata_path.exists() {
        serde_json::from_str::<Vec<Value>>(&fs::read_to_string(&metadata_path)?).unwrap_or_default()
    } else {
        Vec::new()
    };

    events.push(serde_json::json!({
        "timestamp": now_epoch_secs(),
        "session_id": session_id,
        "messages_exported": message_count,
        "transcript_path": transcript_path.display().to_string(),
        "original_request_preserved": original_request_preserved,
        "compaction_trigger": trigger,
    }));

    fs::write(metadata_path, serde_json::to_string_pretty(&events)?)?;
    Ok(())
}

fn save_compaction_metadata(
    session_id: &str,
    transcript_path: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let dirs = ProjectDirs::from_cwd();
    let session_dir = dirs.session_logs(session_id);
    fs::create_dir_all(&session_dir)?;

    let metadata = serde_json::json!({
        "event": "pre_compact",
        "session_id": session_id,
        "timestamp": now_epoch_secs(),
        "transcript_path": transcript_path.map(|p| p.display().to_string()),
    });

    let metadata_file = session_dir.join("compaction_metadata.jsonl");
    let json = serde_json::to_string(&metadata)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(metadata_file)?;
    writeln!(file, "{}", json)?;

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
    fn exports_legacy_conversation_payloads() {
        let tempdir = TempDir::new().unwrap();
        let previous_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tempdir.path()).unwrap();

        let json = r#"{
            "hook_event_name": "PreCompact",
            "session_id": "legacy-session",
            "trigger": "token_limit",
            "conversation": [
                {
                    "role": "user",
                    "content": "Implement conversation transcript preservation with enough detail to capture the original request for later sessions.",
                    "timestamp": "2025-09-23T11:00:00"
                },
                {
                    "role": "assistant",
                    "content": "Acknowledged",
                    "timestamp": "2025-09-23T11:00:01"
                }
            ]
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();

        let hook = PreCompactHook;
        hook.process(input).unwrap();

        let session_dir = tempdir
            .path()
            .join(".claude/runtime/logs/legacy-session");
        assert!(session_dir.join("CONVERSATION_TRANSCRIPT.md").exists());
        assert!(session_dir.join("ORIGINAL_REQUEST.md").exists());
        assert!(session_dir.join("original_request.json").exists());
        assert!(session_dir.join("compaction_events.json").exists());

        let transcript = fs::read_to_string(session_dir.join("CONVERSATION_TRANSCRIPT.md")).unwrap();
        assert!(transcript.contains("Conversation Transcript"));
        assert!(transcript.contains("Implement conversation transcript preservation"));

        std::env::set_current_dir(previous_cwd).unwrap();
    }
}
