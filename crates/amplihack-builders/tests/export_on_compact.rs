// crates/amplihack-builders/tests/export_on_compact.rs
//
// TDD: failing tests for ExportOnCompactIntegration (port of
// amplifier-bundle/tools/amplihack/builders/export_on_compact_integration.py).

use amplihack_builders::export_on_compact::ExportOnCompactIntegration;
use serde_json::json;
use tempfile::TempDir;

#[test]
fn process_returns_session_metadata() {
    let tmp = TempDir::new().unwrap();
    let integ = ExportOnCompactIntegration::with_root(tmp.path().to_path_buf());
    let input = json!({
        "session_id": "sess-x",
        "transcript_path": tmp.path().join("transcript.json"),
        "trigger": "compact"
    });
    std::fs::write(tmp.path().join("transcript.json"), b"[]").unwrap();
    let out = integ.process(&input).unwrap();
    assert_eq!(
        out.get("session_id").and_then(|v| v.as_str()),
        Some("sess-x")
    );
    assert!(out.get("exported_path").is_some());
}

#[test]
fn process_creates_export_file_on_disk() {
    let tmp = TempDir::new().unwrap();
    let integ = ExportOnCompactIntegration::with_root(tmp.path().to_path_buf());
    let transcript = tmp.path().join("t.json");
    std::fs::write(&transcript, r#"[{"role":"user","content":"hi"}]"#).unwrap();
    let input = json!({
        "session_id": "sess-y",
        "transcript_path": transcript,
        "trigger": "compact"
    });
    let out = integ.process(&input).unwrap();
    let path = out
        .get("exported_path")
        .and_then(|v| v.as_str())
        .expect("exported_path string");
    assert!(std::path::Path::new(path).exists());
}

#[test]
fn list_available_sessions_returns_previously_exported() {
    let tmp = TempDir::new().unwrap();
    let integ = ExportOnCompactIntegration::with_root(tmp.path().to_path_buf());
    let transcript = tmp.path().join("t.json");
    std::fs::write(&transcript, b"[]").unwrap();
    integ
        .process(&json!({
            "session_id": "sess-listed",
            "transcript_path": transcript,
            "trigger": "compact"
        }))
        .unwrap();
    let sessions = integ.list_available_sessions().unwrap();
    assert!(sessions.iter().any(|s| s.session_id == "sess-listed"));
}

#[test]
fn restore_enhanced_session_data_returns_known_session() {
    let tmp = TempDir::new().unwrap();
    let integ = ExportOnCompactIntegration::with_root(tmp.path().to_path_buf());
    let transcript = tmp.path().join("t.json");
    std::fs::write(&transcript, b"[]").unwrap();
    integ
        .process(&json!({
            "session_id": "sess-restore",
            "transcript_path": transcript,
            "trigger": "compact"
        }))
        .unwrap();
    let data = integ
        .restore_enhanced_session_data(Some("sess-restore"))
        .unwrap();
    assert_eq!(
        data.get("session_id").and_then(|v| v.as_str()),
        Some("sess-restore")
    );
}

#[test]
fn missing_session_returns_error_not_panic() {
    let tmp = TempDir::new().unwrap();
    let integ = ExportOnCompactIntegration::with_root(tmp.path().to_path_buf());
    let res = integ.restore_enhanced_session_data(Some("does-not-exist"));
    assert!(res.is_err());
}

#[test]
fn missing_transcript_path_in_input_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let integ = ExportOnCompactIntegration::with_root(tmp.path().to_path_buf());
    let input = json!({"session_id":"x","trigger":"compact"});
    assert!(integ.process(&input).is_err());
}
