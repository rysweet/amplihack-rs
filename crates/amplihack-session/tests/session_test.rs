//! Integration tests for `amplihack-session::session` (TDD: failing).
//!
//! Ports `tests/test_claude_session.py` to Rust.

use amplihack_session::{
    ClaudeSession, CommandExecutor, NoopExecutor, SessionConfig, SessionError, SessionState,
};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

#[test]
fn config_default_matches_python_defaults() {
    let cfg = SessionConfig::default();
    assert_eq!(cfg.timeout, Duration::from_secs_f64(300.0));
    assert_eq!(cfg.max_retries, 3);
    assert_eq!(cfg.retry_delay, Duration::from_secs_f64(1.0));
    assert_eq!(cfg.heartbeat_interval, Duration::from_secs_f64(30.0));
    assert!(cfg.enable_logging);
    assert_eq!(cfg.log_level, "INFO");
    assert!(cfg.session_id.is_none());
    assert_eq!(cfg.auto_save_interval, Duration::from_secs_f64(60.0));
}

#[test]
fn state_new_initializes_active_session() {
    let s = SessionState::new("sess-1");
    assert_eq!(s.session_id, "sess-1");
    assert!(s.is_active);
    assert_eq!(s.command_count, 0);
    assert_eq!(s.error_count, 0);
    assert!(s.last_error.is_none());
}

#[test]
fn new_session_generates_id_when_absent() {
    let cfg = SessionConfig::default();
    let s = ClaudeSession::new(cfg);
    assert!(
        s.state.session_id.starts_with("claude_session_"),
        "auto id must use claude_session_ prefix, got {}",
        s.state.session_id
    );
}

#[test]
fn new_session_uses_provided_id() {
    let cfg = SessionConfig {
        session_id: Some("custom-id-123".into()),
        ..SessionConfig::default()
    };
    let s = ClaudeSession::new(cfg);
    assert_eq!(s.state.session_id, "custom-id-123");
}

#[test]
fn execute_command_on_inactive_session_errors() {
    let cfg = SessionConfig::default();
    let mut s = ClaudeSession::new(cfg);
    s.stop();
    let err = s
        .execute_command("noop", None, json!({}))
        .expect_err("inactive should error");
    matches!(err, SessionError::NotActive);
}

#[test]
fn execute_command_increments_counters_and_history() {
    let cfg = SessionConfig::default();
    let mut s = ClaudeSession::new(cfg);
    s.start();
    let _ = s
        .execute_command("analyze", None, json!({"k": "v"}))
        .unwrap();
    let _ = s.execute_command("report", None, json!({})).unwrap();
    assert_eq!(s.state.command_count, 2);
    let hist = s.get_command_history(10);
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[0].command, "analyze");
    assert_eq!(hist[1].command, "report");
    assert_eq!(hist[0].result, "success");
}

#[test]
fn save_and_restore_checkpoint_roundtrips_state() {
    let cfg = SessionConfig::default();
    let mut s = ClaudeSession::new(cfg);
    s.start();
    s.state.command_count = 7;
    s.save_checkpoint();
    s.state.command_count = 99;
    s.restore_checkpoint(-1).unwrap();
    assert_eq!(s.state.command_count, 7);
}

#[test]
fn restore_checkpoint_with_no_checkpoints_errors() {
    let cfg = SessionConfig::default();
    let mut s = ClaudeSession::new(cfg);
    s.start();
    let err = s.restore_checkpoint(-1).unwrap_err();
    matches!(err, SessionError::NoCheckpoints);
}

#[test]
fn check_health_flags_inactive_after_timeout() {
    let cfg = SessionConfig {
        timeout: Duration::from_millis(10),
        ..SessionConfig::default()
    };
    let mut s = ClaudeSession::new(cfg);
    s.start();
    std::thread::sleep(Duration::from_millis(50));
    let err = s.check_health().unwrap_err();
    matches!(err, SessionError::Timeout { .. });
}

#[test]
fn statistics_contain_expected_keys() {
    let cfg = SessionConfig::default();
    let mut s = ClaudeSession::new(cfg);
    s.start();
    let _ = s.execute_command("x", None, json!({})).unwrap();
    let stats = s.get_statistics();
    let obj = stats.as_object().expect("statistics object");
    for key in [
        "session_id",
        "uptime",
        "command_count",
        "error_count",
        "error_rate",
        "is_active",
        "checkpoints",
    ] {
        assert!(obj.contains_key(key), "stats missing key {key}");
    }
}

#[test]
fn clear_history_drops_commands_and_checkpoints() {
    let cfg = SessionConfig::default();
    let mut s = ClaudeSession::new(cfg);
    s.start();
    let _ = s.execute_command("a", None, json!({})).unwrap();
    s.save_checkpoint();
    s.clear_history();
    assert!(s.get_command_history(100).is_empty());
    assert_eq!(s.checkpoint_count(), 0);
}

// ---- CommandExecutor trait extension point ----

#[derive(Default, Clone)]
struct CountingExecutor {
    calls: Arc<AtomicU32>,
}

impl CommandExecutor for CountingExecutor {
    fn execute(
        &self,
        command: &str,
        _kwargs: &serde_json::Value,
    ) -> Result<serde_json::Value, SessionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"echo": command}))
    }
}

#[test]
fn custom_executor_is_invoked() {
    let exec = CountingExecutor::default();
    let mut s = ClaudeSession::with_executor(SessionConfig::default(), Box::new(exec.clone()));
    s.start();
    let out = s.execute_command("ping", None, json!({})).unwrap();
    assert_eq!(out, json!({"echo": "ping"}));
    assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn noop_executor_returns_completed_status() {
    let mut s = ClaudeSession::with_executor(SessionConfig::default(), Box::new(NoopExecutor));
    s.start();
    let out = s.execute_command("noop", None, json!({})).unwrap();
    assert_eq!(
        out.get("status").and_then(|v| v.as_str()),
        Some("completed"),
        "NoopExecutor must report completed status"
    );
}
