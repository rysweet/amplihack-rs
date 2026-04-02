use super::*;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

// -- CommandResult tests ----------------------------------------------------

#[test]
fn command_result_success() {
    let r = CommandResult {
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
    };
    assert!(r.success());
}

#[test]
fn command_result_failure_code() {
    let r = CommandResult {
        exit_code: Some(1),
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
    };
    assert!(!r.success());
}

#[test]
fn command_result_timeout() {
    let r = CommandResult {
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        timed_out: true,
    };
    assert!(!r.success());
}

#[test]
fn command_result_no_exit_code() {
    let r = CommandResult {
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
    };
    assert!(!r.success());
}

// -- ProcessManager tests ---------------------------------------------------

#[test]
fn run_echo() {
    let mgr = ProcessManager::new();
    let result = mgr
        .run_command(&["echo", "hello"], None, None, None)
        .expect("echo should succeed");
    assert!(result.success());
    assert_eq!(result.stdout.trim(), "hello");
}

#[test]
fn run_false_command() {
    let mgr = ProcessManager::new();
    let result = mgr
        .run_command(&["false"], None, None, None)
        .expect("should not be an IO error");
    assert!(!result.success());
    assert_eq!(result.exit_code, Some(1));
}

#[test]
fn run_with_cwd() {
    let mgr = ProcessManager::new();
    let result = mgr
        .run_command(&["pwd"], None, Some(Path::new("/")), None)
        .expect("pwd should succeed");
    assert!(result.success());
    assert_eq!(result.stdout.trim(), "/");
}

#[test]
fn run_with_env() {
    let mgr = ProcessManager::new();
    let mut env = HashMap::new();
    env.insert("MY_TEST_VAR".into(), "test_value".into());
    let result = mgr
        .run_command(&["sh", "-c", "echo $MY_TEST_VAR"], None, None, Some(&env))
        .expect("sh should succeed");
    assert!(result.success());
    assert_eq!(result.stdout.trim(), "test_value");
}

#[test]
fn run_with_timeout_completes() {
    let mgr = ProcessManager::new();
    let result = mgr
        .run_command_with_timeout(&["echo", "fast"], Duration::from_secs(5), None)
        .expect("should succeed");
    assert!(result.success());
    assert!(!result.timed_out);
}

#[test]
fn run_with_timeout_kills() {
    let mgr = ProcessManager::new();
    let result = mgr
        .run_command_with_timeout(&["sleep", "60"], Duration::from_millis(200), None)
        .expect("should not be an IO error");
    assert!(result.timed_out);
    assert!(!result.success());
}

#[test]
fn run_empty_args() {
    let mgr = ProcessManager::new();
    let result = mgr
        .run_command(&[], None, None, None)
        .expect("should return empty result");
    assert!(!result.success());
}

#[test]
fn run_captures_stderr() {
    let mgr = ProcessManager::new();
    let result = mgr
        .run_command(&["sh", "-c", "echo err >&2"], None, None, None)
        .expect("should succeed");
    assert!(result.stderr.contains("err"));
}

// -- run_command_with_timeout wrapper ---------------------------------------

#[test]
fn run_command_with_timeout_wrapper() {
    let mgr = ProcessManager::new();
    let result = mgr
        .run_command_with_timeout(&["echo", "wrap"], Duration::from_secs(5), None)
        .expect("should succeed");
    assert!(result.success());
    assert_eq!(result.stdout.trim(), "wrap");
}

// -- ensure_path_within_root tests ------------------------------------------

#[test]
fn path_within_root_ok() {
    let dir = tempfile::tempdir().expect("tempdir");
    let child = dir.path().join("child");
    std::fs::create_dir(&child).expect("mkdir");
    let result = ensure_path_within_root(&child, dir.path());
    assert!(result.is_ok());
}

#[test]
fn path_escapes_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let result = ensure_path_within_root(Path::new("/usr"), dir.path());
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("escapes root"));
}

#[test]
fn path_nonexistent_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bad = dir.path().join("does_not_exist");
    let result = ensure_path_within_root(&bad, dir.path());
    assert!(result.is_err());
}

#[test]
fn path_is_root_itself() {
    let dir = tempfile::tempdir().expect("tempdir");
    let result = ensure_path_within_root(dir.path(), dir.path());
    assert!(result.is_ok());
}

// -- Serde round-trip for CommandResult --------------------------------------

#[test]
fn command_result_serde_roundtrip() {
    let original = CommandResult {
        exit_code: Some(0),
        stdout: "output".into(),
        stderr: "".into(),
        timed_out: false,
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let restored: CommandResult = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.exit_code, original.exit_code);
    assert_eq!(restored.stdout, original.stdout);
    assert_eq!(restored.timed_out, original.timed_out);
}
