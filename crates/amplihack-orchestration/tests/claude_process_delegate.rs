//! TDD tests for `claude_process` Rust port of `claude_process.py`.
//!
//! These tests are FAILING by design — they specify the contract for
//! `ProcessRunner`, `ClaudeProcess`, `ProcessResult`, and the
//! `DELEGATE_COMMANDS` table. The implementation in
//! `crates/amplihack-orchestration/src/claude_process.rs` must satisfy them.
//!
//! Behavioral parity with `tests/test_claude_process_delegate.py`:
//! - DELEGATE_COMMANDS lookup table values
//! - Default falls back to ["claude"] when env is unset/unknown (with a warning)
//! - `--dangerously-skip-permissions -p <prompt>` always appended
//! - `--model <model>` appended when set
//! - ProcessResult has exit_code, output, stderr, duration, process_id
//! - `ProcessResult::ok()` / `err()` constructors
//! - exit_code = -1 sentinel for timeout / fatal errors

use std::sync::Arc;
use std::time::Duration;

use amplihack_orchestration::claude_process::{
    ClaudeProcess, DELEGATE_COMMANDS, MockProcessRunner, ProcessResult, ProcessRunner, RunOptions,
    build_command,
};

#[test]
fn delegate_commands_table_matches_python() {
    // Mirror of DELEGATE_COMMANDS in claude_process.py.
    assert_eq!(
        DELEGATE_COMMANDS.get("amplihack claude"),
        Some(&vec!["claude".to_string()])
    );
    assert_eq!(
        DELEGATE_COMMANDS.get("amplihack copilot"),
        Some(&vec!["amplihack".to_string(), "copilot".to_string()])
    );
    assert_eq!(
        DELEGATE_COMMANDS.get("amplihack amplifier"),
        Some(&vec!["amplihack".to_string(), "amplifier".to_string()])
    );
    assert_eq!(DELEGATE_COMMANDS.len(), 3);
}

#[test]
fn build_command_uses_claude_default_when_delegate_unset() {
    let cmd = build_command(None, "hello world", None);
    assert_eq!(
        cmd,
        vec![
            "claude".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "-p".to_string(),
            "hello world".to_string(),
        ]
    );
}

#[test]
fn build_command_uses_claude_default_when_delegate_unknown() {
    let cmd = build_command(Some("bogus"), "hi", None);
    assert_eq!(cmd[0], "claude");
    assert!(cmd.contains(&"--dangerously-skip-permissions".to_string()));
    assert!(cmd.contains(&"-p".to_string()));
    assert!(cmd.contains(&"hi".to_string()));
}

#[test]
fn build_command_uses_amplihack_copilot_when_set() {
    let cmd = build_command(Some("amplihack copilot"), "task", None);
    assert_eq!(&cmd[..2], &["amplihack".to_string(), "copilot".to_string()]);
    assert_eq!(
        &cmd[2..4],
        &[
            "--dangerously-skip-permissions".to_string(),
            "-p".to_string()
        ]
    );
    assert_eq!(cmd[4], "task");
}

#[test]
fn build_command_uses_amplihack_amplifier_when_set() {
    let cmd = build_command(Some("amplihack amplifier"), "x", None);
    assert_eq!(
        &cmd[..2],
        &["amplihack".to_string(), "amplifier".to_string()]
    );
}

#[test]
fn build_command_appends_model_flag_when_provided() {
    let cmd = build_command(Some("amplihack claude"), "p", Some("claude-3-opus"));
    assert!(cmd.windows(2).any(|w| w == ["--model", "claude-3-opus"]));
}

#[test]
fn build_command_omits_model_flag_when_none() {
    let cmd = build_command(Some("amplihack claude"), "p", None);
    assert!(!cmd.iter().any(|s| s == "--model"));
}

#[test]
fn process_result_ok_constructor_marks_success() {
    let r = ProcessResult::ok("out".into(), "id".into(), Duration::from_millis(100));
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.output, "out");
    assert_eq!(r.stderr, "");
    assert_eq!(r.process_id, "id");
    assert!(r.is_success());
}

#[test]
fn process_result_err_constructor_uses_minus_one_sentinel() {
    let r = ProcessResult::err("boom".into(), "id".into(), Duration::from_millis(0));
    assert_eq!(r.exit_code, -1);
    assert_eq!(r.stderr, "boom");
    assert_eq!(r.output, "");
    assert!(!r.is_success());
}

#[tokio::test]
async fn mock_runner_returns_canned_response_for_prompt() {
    let mock = MockProcessRunner::new();
    mock.expect(
        "hello",
        ProcessResult::ok("world".into(), "p1".into(), Duration::from_millis(1)),
    );

    let runner: Arc<dyn ProcessRunner> = Arc::new(mock);
    let opts = RunOptions::new("hello".into(), "p1".into());
    let res = runner.run(opts).await;

    assert_eq!(res.exit_code, 0);
    assert_eq!(res.output, "world");
}

#[tokio::test]
async fn mock_runner_records_invocations() {
    let mock = MockProcessRunner::new();
    mock.expect(
        "a",
        ProcessResult::ok("A".into(), "1".into(), Duration::ZERO),
    );
    mock.expect(
        "b",
        ProcessResult::ok("B".into(), "2".into(), Duration::ZERO),
    );

    let runner = Arc::new(mock);
    runner.run(RunOptions::new("a".into(), "1".into())).await;
    runner.run(RunOptions::new("b".into(), "2".into())).await;

    let calls = runner.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].prompt, "a");
    assert_eq!(calls[1].prompt, "b");
}

#[tokio::test]
async fn mock_runner_returns_error_for_unmatched_prompt() {
    let mock = Arc::new(MockProcessRunner::new());
    let res = mock
        .run(RunOptions::new("unknown".into(), "x".into()))
        .await;
    assert_eq!(res.exit_code, -1);
    assert!(!res.stderr.is_empty());
}

#[tokio::test]
async fn claude_process_run_delegates_to_runner() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "go",
        ProcessResult::ok("done".into(), "pid".into(), Duration::from_millis(5)),
    );

    let log_dir = tempfile::tempdir().unwrap();
    let cp = ClaudeProcess::builder()
        .prompt("go")
        .process_id("pid")
        .working_dir(log_dir.path().to_path_buf())
        .log_dir(log_dir.path().to_path_buf())
        .runner(mock.clone() as Arc<dyn ProcessRunner>)
        .build()
        .unwrap();

    let result = cp.run().await;
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output, "done");
    assert_eq!(result.process_id, "pid");
}

#[tokio::test]
async fn claude_process_writes_log_file_on_run() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "x",
        ProcessResult::ok("y".into(), "logged".into(), Duration::from_millis(1)),
    );
    let log_dir = tempfile::tempdir().unwrap();

    let cp = ClaudeProcess::builder()
        .prompt("x")
        .process_id("logged")
        .working_dir(log_dir.path().to_path_buf())
        .log_dir(log_dir.path().to_path_buf())
        .runner(mock as Arc<dyn ProcessRunner>)
        .build()
        .unwrap();

    cp.run().await;
    let log_file = log_dir.path().join("logged.log");
    assert!(
        log_file.exists(),
        "Expected per-process log file to be written"
    );
}

#[tokio::test]
async fn claude_process_propagates_timeout_via_run_options() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "slow",
        ProcessResult::err("timeout".into(), "t".into(), Duration::from_secs(1)),
    );

    let log_dir = tempfile::tempdir().unwrap();
    let cp = ClaudeProcess::builder()
        .prompt("slow")
        .process_id("t")
        .working_dir(log_dir.path().to_path_buf())
        .log_dir(log_dir.path().to_path_buf())
        .timeout(Duration::from_millis(10))
        .runner(mock.clone() as Arc<dyn ProcessRunner>)
        .build()
        .unwrap();

    let _ = cp.run().await;
    let calls = mock.calls();
    assert_eq!(calls[0].timeout, Some(Duration::from_millis(10)));
}
