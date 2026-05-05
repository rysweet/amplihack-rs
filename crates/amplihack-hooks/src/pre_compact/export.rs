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
    writeln!(file, "{json}")?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_response_format() {
        let err = anyhow::anyhow!("disk full");
        let resp = error_response("writing log", &err);
        let output = &resp["hookSpecificOutput"];
        assert_eq!(output["hookEventName"], "PreCompact");
        assert_eq!(output["status"], "error");
        assert!(output["message"].as_str().unwrap().contains("writing log"));
        assert!(output["error"].as_str().unwrap().contains("disk full"));
    }

    #[test]
    fn generate_session_id_format() {
        let id = generate_session_id();
        assert!(id.starts_with("session-"));
        let secs_str = &id["session-".len()..];
        let secs: u64 = secs_str.parse().expect("suffix should be numeric");
        assert!(secs > 1_700_000_000, "timestamp should be recent");
    }

    #[test]
    fn export_transcript_missing_file() {
        let result = export_transcript(Path::new("/nonexistent/transcript.jsonl"), "test-session");
        // Should return Ok(None) when file doesn't exist
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn export_transcript_with_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("transcript.jsonl");
        fs::write(&src, r#"{"role":"user","content":"hello"}"#).unwrap();

        let result = export_transcript(&src, "export-test");
        let exported = result.unwrap().unwrap();
        assert!(exported.exists());
        assert!(exported.to_string_lossy().contains("transcript_pre_compact.jsonl"));
    }

    #[test]
    fn save_compaction_metadata_basic() {
        let extra = json!({"trigger": "auto"});
        let result = save_compaction_metadata("meta-test", None, &extra, true);
        let metadata = result.unwrap();
        assert_eq!(metadata["session_id"], "meta-test");
        assert_eq!(metadata["original_request_preserved"], true);
        assert_eq!(metadata["compaction_trigger"], "auto");
        assert!(metadata["timestamp"].as_u64().unwrap() > 0);
    }

    #[test]
    fn save_compaction_metadata_no_trigger() {
        let extra = json!({});
        let result = save_compaction_metadata("no-trigger", None, &extra, false);
        let metadata = result.unwrap();
        assert_eq!(metadata["original_request_preserved"], false);
        assert!(metadata.get("compaction_trigger").is_none());
    }
}
