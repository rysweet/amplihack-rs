// crates/amplihack-builders/tests/claude_builder.rs
//
// TDD: failing tests for ClaudeTranscriptBuilder (port of
// amplifier-bundle/tools/amplihack/builders/claude_transcript_builder.py).

use amplihack_builders::claude::{ClaudeTranscriptBuilder, TranscriptOptions};
use tempfile::TempDir;

fn write_messages(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let p = dir.join("messages.json");
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn builds_session_transcript_with_user_and_assistant_turns() {
    let tmp = TempDir::new().unwrap();
    let msgs = r#"[
        {"role":"user","content":"hello"},
        {"role":"assistant","content":"hi there"}
    ]"#;
    let path = write_messages(tmp.path(), msgs);
    let b = ClaudeTranscriptBuilder::new("sess-1", tmp.path().to_path_buf());
    let out = b
        .build_session_transcript(&path, &TranscriptOptions::default())
        .unwrap();
    assert!(out.contains("hello"));
    assert!(out.contains("hi there"));
    assert!(out.contains("sess-1"));
}

#[test]
fn build_session_summary_includes_counts() {
    let tmp = TempDir::new().unwrap();
    let msgs = r#"[
        {"role":"user","content":"q1"},
        {"role":"assistant","content":"a1"},
        {"role":"user","content":"q2"},
        {"role":"assistant","content":"a2"}
    ]"#;
    let path = write_messages(tmp.path(), msgs);
    let b = ClaudeTranscriptBuilder::new("sess-2", tmp.path().to_path_buf());
    let summary = b.build_session_summary(&path).unwrap();
    assert!(summary.message_count >= 4);
    assert!(summary.user_turns >= 2);
    assert!(summary.assistant_turns >= 2);
}

#[test]
fn malformed_messages_file_returns_error() {
    let tmp = TempDir::new().unwrap();
    let path = write_messages(tmp.path(), "{not json");
    let b = ClaudeTranscriptBuilder::new("sess-bad", tmp.path().to_path_buf());
    let res = b.build_session_transcript(&path, &TranscriptOptions::default());
    assert!(res.is_err(), "malformed input must fail closed");
}

#[test]
fn export_for_codex_writes_file() {
    let tmp = TempDir::new().unwrap();
    let msgs = r#"[{"role":"assistant","content":"answer"}]"#;
    let path = write_messages(tmp.path(), msgs);
    let b = ClaudeTranscriptBuilder::new("sess-3", tmp.path().to_path_buf());
    let out = tmp.path().join("export.md");
    b.export_for_codex(&path, &out).unwrap();
    assert!(out.exists());
    let body = std::fs::read_to_string(&out).unwrap();
    assert!(body.contains("answer"));
}

#[test]
fn empty_messages_array_produces_empty_but_valid_transcript() {
    let tmp = TempDir::new().unwrap();
    let path = write_messages(tmp.path(), "[]");
    let b = ClaudeTranscriptBuilder::new("sess-empty", tmp.path().to_path_buf());
    let out = b
        .build_session_transcript(&path, &TranscriptOptions::default())
        .unwrap();
    assert!(out.contains("sess-empty"));
}
