//! Tests for pre-compact hook.

use super::export::generate_session_id;
use super::*;
use crate::protocol::Hook;
use crate::test_support::env_lock;
use amplihack_types::HookInput;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

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

    let output = &result["hookSpecificOutput"];
    assert_eq!(output["hookEventName"], "PreCompact");
    assert_eq!(output["status"], "success");
    assert!(
        output["message"]
            .as_str()
            .unwrap()
            .contains("Conversation exported successfully")
    );
    assert_eq!(output["metadata"]["event"], "pre_compact");
    let exported = PathBuf::from(output["transcript_path"].as_str().unwrap());
    assert!(exported.exists());
    assert!(
        dir.path()
            .join(".claude/runtime/logs/test-session/compaction_metadata.jsonl")
            .exists()
    );
    assert_eq!(output["metadata"]["original_request_preserved"], false);
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

    let output = &result["hookSpecificOutput"];
    assert_eq!(output["hookEventName"], "PreCompact");
    assert_eq!(output["status"], "success");
    assert_eq!(output["metadata"]["original_request_preserved"], true);
    assert_eq!(output["metadata"]["compaction_trigger"], "token_limit");
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

    let output = &result["hookSpecificOutput"];
    assert_eq!(output["hookEventName"], "PreCompact");
    assert_eq!(output["status"], "success");
    assert_eq!(output["metadata"]["original_request_preserved"], true);
    assert!(
        dir.path()
            .join(".claude/runtime/logs/compact-transcript/ORIGINAL_REQUEST.md")
            .exists()
    );
}
