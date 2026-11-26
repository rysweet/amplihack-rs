//! Tests for pre-compact hook.

use super::export::generate_session_id;
use super::*;
use crate::protocol::Hook;
use crate::test_support::env_lock;
use amplihack_types::HookInput;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// The last record in `<logs>/compaction_metadata.jsonl`.
fn last_metadata(logs: &Path) -> Value {
    let text = fs::read_to_string(logs.join("compaction_metadata.jsonl")).unwrap();
    let line = text.lines().last().expect("one metadata record");
    serde_json::from_str(line).unwrap()
}

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
fn pre_compact_prints_empty_object_and_exports_transcript() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _state = crate::test_support::isolate_state_home(home.path());
    let sessions = home.path().join(".amplihack/runtime/sessions");
    let cwd = crate::test_support::CwdGuard::set(dir.path()).unwrap();

    let transcript = dir.path().join("input.jsonl");
    let body = "{\"type\":\"user\",\"message\":\"hello\"}\n";
    fs::write(&transcript, body).unwrap();

    let hook = PreCompactHook;
    let result = hook
        .process(HookInput::PreCompact {
            session_id: Some("test-session".to_string()),
            transcript_path: Some(transcript.clone()),
            extra: Value::Null,
        })
        .unwrap();

    drop(cwd);

    assert!(result.get("hookSpecificOutput").is_none(), "{result}");
    assert_eq!(result, json!({}));
    let logs = sessions.join("test-session/logs");
    let exported = logs.join("transcript_pre_compact.jsonl");
    assert_eq!(fs::read_to_string(&exported).unwrap(), body);
    assert!(!dir.path().join(".claude").exists());
    let metadata = last_metadata(&logs);
    assert_eq!(metadata["event"], "pre_compact");
    assert_eq!(
        metadata["transcript_path"].as_str(),
        Some(exported.to_string_lossy().as_ref())
    );
    assert_eq!(metadata["original_request_preserved"], false);
}

#[test]
fn pre_compact_preserves_original_request_from_conversation_payload() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _state = crate::test_support::isolate_state_home(home.path());
    let sessions = home.path().join(".amplihack/runtime/sessions");
    let cwd = crate::test_support::CwdGuard::set(dir.path()).unwrap();

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

    drop(cwd);

    assert_eq!(result, json!({}));
    let logs = sessions.join("compact-session/logs");
    let metadata = last_metadata(&logs);
    assert_eq!(metadata["original_request_preserved"], true);
    assert_eq!(metadata["compaction_trigger"], "token_limit");
    assert!(logs.join("ORIGINAL_REQUEST.md").exists());
    assert!(logs.join("original_request.json").exists());
}

#[test]
fn pre_compact_preserves_original_request_from_transcript_jsonl() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _state = crate::test_support::isolate_state_home(home.path());
    let sessions = home.path().join(".amplihack/runtime/sessions");
    let cwd = crate::test_support::CwdGuard::set(dir.path()).unwrap();

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

    drop(cwd);

    assert_eq!(result, json!({}));
    let logs = sessions.join("compact-transcript/logs");
    assert_eq!(last_metadata(&logs)["original_request_preserved"], true);
    assert!(logs.join("ORIGINAL_REQUEST.md").exists());
}
