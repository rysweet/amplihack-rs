//! Tests for session stop hook.

use super::*;
use crate::protocol::Hook;
use crate::test_support::env_lock;
use std::fs;

#[test]
fn handles_unknown_events() {
    let hook = SessionStopHook;
    let result = hook.process(HookInput::Unknown).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn warn_uncommitted_work_does_not_panic() {
    git::warn_uncommitted_work();
}

#[test]
fn read_transcript_turns_parses_message_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let t = dir.path().join("transcript.jsonl");
    fs::write(
        &t,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Use /analyze auth"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done"}]}}
"#,
    )
    .unwrap();

    let turns = transcript::read_transcript_turns(&t).unwrap();

    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].content, "Use /analyze auth");
    assert_eq!(turns[1].role, "assistant");
}

#[test]
fn detect_agents_from_transcript_uses_user_turns() {
    let turns = vec![
        transcript::TranscriptTurn {
            role: "user".to_string(),
            content: "Please /analyze auth flow".to_string(),
        },
        transcript::TranscriptTurn {
            role: "assistant".to_string(),
            content: "I am using the analyzer agent".to_string(),
        },
    ];

    let agents = transcript::detect_agents_from_transcript(&turns);

    assert_eq!(agents, vec!["analyzer".to_string()]);
}

#[test]
fn session_stop_stores_learning_in_sqlite_backend() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let prev_home = std::env::var_os("HOME");
    let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
    unsafe {
        std::env::set_var("HOME", dir.path());
        std::env::set_var("AMPLIHACK_MEMORY_BACKEND", "sqlite");
    }

    let t = dir.path().join("session.jsonl");
    fs::write(
        &t,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Use /analyze the auth flow"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Found the failing middleware and proposed a fix."}]}}
"#,
    )
    .unwrap();

    let hook = SessionStopHook;
    let result = hook
        .process(HookInput::SessionStop {
            session_id: Some("session-stop-test".to_string()),
            transcript_path: Some(t),
            extra: serde_json::json!({}),
        })
        .unwrap();

    let memories = amplihack_cli::memory::retrieve_prompt_context_memories(
        "session-stop-test",
        "auth flow",
        2000,
    )
    .unwrap();

    match prev_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match prev_backend {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
    }

    assert!(result.as_object().unwrap().is_empty());
    assert_eq!(memories.len(), 1);
    assert!(memories[0].content.contains("Found the failing middleware"));
}

#[test]
fn generate_unique_session_id_produces_different_ids() {
    let id1 = super::generate_unique_session_id();
    // Small sleep to ensure timestamp differs.
    std::thread::sleep(std::time::Duration::from_millis(2));
    let id2 = super::generate_unique_session_id();
    assert_ne!(id1, id2, "Two calls should produce different session IDs");
    assert!(id1.starts_with("hook-"), "ID should start with 'hook-'");
    assert!(id2.starts_with("hook-"), "ID should start with 'hook-'");
}
