//! Pre-compact hook: exports conversation transcript before context compaction.
//!
//! When the context window is about to be compacted, this hook:
//! 1. Exports the current conversation to a JSONL transcript file
//! 2. Extracts the original request for context preservation
//! 3. Saves compaction metadata

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::HookInput;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
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
        let (session_id, transcript_path) = match input {
            HookInput::PreCompact {
                session_id,
                transcript_path,
                ..
            } => (session_id, transcript_path),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        let session_id = session_id.unwrap_or_else(generate_session_id);

        // Export transcript if path provided.
        if let Some(ref path) = transcript_path
            && let Err(e) = export_transcript(path, &session_id)
        {
            tracing::warn!("Failed to export transcript: {}", e);
        }

        // Save compaction metadata.
        if let Err(e) = save_compaction_metadata(&session_id, transcript_path.as_deref()) {
            tracing::warn!("Failed to save compaction metadata: {}", e);
        }

        Ok(Value::Object(serde_json::Map::new()))
    }
}

fn export_transcript(transcript_path: &std::path::Path, session_id: &str) -> anyhow::Result<()> {
    let export_dir = get_session_dir(session_id)?;
    fs::create_dir_all(&export_dir)?;

    let export_path = export_dir.join("transcript_pre_compact.jsonl");

    if transcript_path.exists() {
        fs::copy(transcript_path, &export_path)?;
        tracing::info!("Exported transcript to {}", export_path.display());
    }

    Ok(())
}

fn save_compaction_metadata(
    session_id: &str,
    transcript_path: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let session_dir = get_session_dir(session_id)?;
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

fn get_session_dir(session_id: &str) -> anyhow::Result<PathBuf> {
    let dir = std::env::current_dir()?
        .join(".claude")
        .join("runtime")
        .join("logs")
        .join(session_id);
    Ok(dir)
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
}
