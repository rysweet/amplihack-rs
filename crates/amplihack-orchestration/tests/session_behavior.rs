//! TDD tests for `OrchestratorSession` — port of `session.py`.
//!
//! Behavioral parity contract:
//! - Session ID is `<pattern_name>_<unix_timestamp>` and is unique per session.
//! - Log directory is `<base_log_dir>/<session_id>` and is created on init.
//! - Default base log dir is `<working_dir>/.claude/runtime/logs`.
//! - Auto-generated process IDs follow `process_001`, `process_002`, ...
//!   (zero-padded width 3) and increment per call.
//! - `summarize()` includes session ID, pattern name, working dir, log dir,
//!   and current process counter.
//! - `log()` appends `[HH:MM:SS] [LEVEL] msg\n` to `session.log`.
//! - Metadata is written on init (header in session.log).

use std::sync::Arc;
use std::time::Duration;

use amplihack_orchestration::claude_process::{MockProcessRunner, ProcessResult, ProcessRunner};
use amplihack_orchestration::session::OrchestratorSession;

#[test]
fn session_id_starts_with_pattern_name() {
    let dir = tempfile::tempdir().unwrap();
    let session = OrchestratorSession::builder()
        .pattern_name("debate")
        .working_dir(dir.path().to_path_buf())
        .base_log_dir(dir.path().to_path_buf())
        .build()
        .unwrap();

    assert!(session.session_id().starts_with("debate_"));
}

#[test]
fn session_creates_log_dir_on_init() {
    let dir = tempfile::tempdir().unwrap();
    let session = OrchestratorSession::builder()
        .pattern_name("test")
        .working_dir(dir.path().to_path_buf())
        .base_log_dir(dir.path().to_path_buf())
        .build()
        .unwrap();

    assert!(session.log_dir().exists());
    assert!(session.log_dir().is_dir());
}

#[test]
fn session_writes_metadata_to_session_log() {
    let dir = tempfile::tempdir().unwrap();
    let session = OrchestratorSession::builder()
        .pattern_name("metaTest")
        .working_dir(dir.path().to_path_buf())
        .base_log_dir(dir.path().to_path_buf())
        .build()
        .unwrap();

    let log_path = session.session_log_path();
    assert!(log_path.exists());
    let contents = std::fs::read_to_string(log_path).unwrap();
    assert!(contents.contains("Session ID:"));
    assert!(contents.contains("metaTest"));
    assert!(contents.contains("Pattern:"));
}

#[test]
fn session_default_base_log_dir_is_claude_runtime_logs() {
    let dir = tempfile::tempdir().unwrap();
    let session = OrchestratorSession::builder()
        .pattern_name("defaults")
        .working_dir(dir.path().to_path_buf())
        .build()
        .unwrap();

    let expected_prefix = dir.path().join(".claude").join("runtime").join("logs");
    assert!(
        session.log_dir().starts_with(&expected_prefix),
        "log_dir should start with {:?}, got {:?}",
        expected_prefix,
        session.log_dir(),
    );
}

#[test]
fn session_create_process_auto_generates_zero_padded_ids() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "p",
        ProcessResult::ok("".into(), "x".into(), Duration::ZERO),
    );

    let mut session = OrchestratorSession::builder()
        .pattern_name("ids")
        .working_dir(dir.path().to_path_buf())
        .base_log_dir(dir.path().to_path_buf())
        .runner(mock as Arc<dyn ProcessRunner>)
        .build()
        .unwrap();

    let p1 = session.create_process("p", None, None, None).unwrap();
    let p2 = session.create_process("p", None, None, None).unwrap();
    let p3 = session.create_process("p", None, None, None).unwrap();

    assert_eq!(p1.process_id(), "process_001");
    assert_eq!(p2.process_id(), "process_002");
    assert_eq!(p3.process_id(), "process_003");
}

#[test]
fn session_create_process_uses_provided_id_when_given() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "p",
        ProcessResult::ok("".into(), "x".into(), Duration::ZERO),
    );

    let mut session = OrchestratorSession::builder()
        .pattern_name("ids")
        .working_dir(dir.path().to_path_buf())
        .base_log_dir(dir.path().to_path_buf())
        .runner(mock as Arc<dyn ProcessRunner>)
        .build()
        .unwrap();

    let p = session
        .create_process("p", Some("custom"), None, None)
        .unwrap();
    assert_eq!(p.process_id(), "custom");
}

#[test]
fn session_summarize_includes_key_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "p",
        ProcessResult::ok("".into(), "x".into(), Duration::ZERO),
    );

    let mut session = OrchestratorSession::builder()
        .pattern_name("summary-test")
        .working_dir(dir.path().to_path_buf())
        .base_log_dir(dir.path().to_path_buf())
        .runner(mock as Arc<dyn ProcessRunner>)
        .build()
        .unwrap();
    let _ = session.create_process("p", None, None, None).unwrap();

    let s = session.summarize();
    assert!(s.contains("summary-test"));
    assert!(s.contains(session.session_id()));
    assert!(s.contains("Processes Created"));
}

#[test]
fn session_log_appends_to_session_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let session = OrchestratorSession::builder()
        .pattern_name("loggy")
        .working_dir(dir.path().to_path_buf())
        .base_log_dir(dir.path().to_path_buf())
        .build()
        .unwrap();

    session.log_info("hello world");
    session.log_warn("something fishy");

    let contents = std::fs::read_to_string(session.session_log_path()).unwrap();
    assert!(contents.contains("hello world"));
    assert!(contents.contains("something fishy"));
    assert!(contents.contains("[INFO]"));
    assert!(contents.contains("[WARNING]"));
}
