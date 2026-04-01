//! Transcript export, compaction metadata, and utility functions.

use amplihack_types::ProjectDirs;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) fn error_response(context: &str, error: &anyhow::Error) -> Value {
    let message = format!("{context}: {error}");
    tracing::warn!("{}", message);
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreCompact",
            "status": "error",
            "message": message,
            "error": error.to_string(),
        }
    })
}

pub(crate) fn export_transcript(
    transcript_path: &Path,
    session_id: &str,
) -> anyhow::Result<Option<PathBuf>> {
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

pub(crate) fn save_compaction_metadata(
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

pub(crate) fn generate_session_id() -> String {
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
