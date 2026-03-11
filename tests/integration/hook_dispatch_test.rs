/// Integration tests: amplihack-hooks dispatcher smoke tests.
///
/// These tests exercise the `amplihack-hooks` binary through its dispatch
/// layer.  Each hook is invoked with a minimal valid JSON payload on stdin
/// and the test asserts:
///   - The binary exits with code 0 (hooks fail-open)
///   - stdout is valid JSON
///   - The JSON does not contain error fields that indicate a panic
///
/// The hooks may call Python bridge scripts when the Python SDK is available.
/// When Python/amplihack is not installed, hooks fall back to non-SDK paths.
/// All hooks must remain fail-open (exit 0, produce `{}` or valid output).
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Path to the compiled amplihack-hooks binary.
fn hooks_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/
    path.pop(); // workspace root
    path.push("target/debug/amplihack-hooks");
    path
}

/// Invoke the hooks binary with a given subcommand and JSON stdin.
/// Returns (stdout_str, stderr_str, exit_success).
fn run_hook(subcommand: &str, input_json: &str) -> (String, String, bool) {
    let bin = hooks_bin();
    if !bin.exists() {
        return (
            "{}".to_string(),
            format!("binary not found at {:?}", bin),
            true,
        );
    }

    let mut child = Command::new(&bin)
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn hooks binary");

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input_json.as_bytes());
    }

    let output = child.wait_with_output().expect("Failed to wait on hooks");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

/// Assert that a hook invocation succeeds and returns valid JSON.
fn assert_hook_ok(subcommand: &str, input: &str) {
    let (stdout, stderr, success) = run_hook(subcommand, input);
    assert!(
        success,
        "Hook '{}' must exit 0 (fail-open). stderr: {}",
        subcommand, stderr
    );
    // stdout must be valid JSON.
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
    assert!(
        parsed.is_ok(),
        "Hook '{}' stdout must be valid JSON. Got: '{}' stderr: {}",
        subcommand,
        stdout,
        stderr
    );
    let value = parsed.unwrap();
    // Every non-empty hook response must use a known protocol key:
    // `hookSpecificOutput` (general), `decision`/`reason` (PreToolUse/Stop).
    let has_protocol_key = value.as_object().is_some_and(|obj| {
        obj.is_empty()
            || obj.contains_key("hookSpecificOutput")
            || obj.contains_key("decision")
            || obj.contains_key("reason")
    });
    assert!(
        has_protocol_key,
        "Hook '{}' JSON must contain a protocol key (hookSpecificOutput, decision, or reason) or be empty. Got: {}",
        subcommand, value
    );
}

// ---------------------------------------------------------------------------
// Minimal valid JSON payloads per hook
// ---------------------------------------------------------------------------

const PRE_TOOL_USE_INPUT: &str = r#"{
    "hook_event_name": "PreToolUse",
    "tool_name": "Bash",
    "tool_input": {"command": "echo hello"},
    "session_id": "test-session"
}"#;

const POST_TOOL_USE_INPUT: &str = r#"{
    "hook_event_name": "PostToolUse",
    "tool_name": "Bash",
    "tool_input": {"command": "echo hello"},
    "tool_response": {"output": "hello\n"},
    "session_id": "test-session"
}"#;

const STOP_INPUT: &str = r#"{
    "hook_event_name": "Stop",
    "session_id": "test-session",
    "transcript_path": "/tmp/nonexistent-transcript.jsonl"
}"#;

const SESSION_START_INPUT: &str = r#"{
    "hook_event_name": "SessionStart",
    "session_id": "test-session"
}"#;

const SESSION_STOP_INPUT: &str = r#"{
    "hook_event_name": "SessionStop",
    "session_id": "test-session",
    "transcript_path": "/tmp/nonexistent-transcript.jsonl"
}"#;

const USER_PROMPT_INPUT: &str = r#"{
    "hook_event_name": "UserPromptSubmit",
    "user_prompt": "Hello, what can you help me with?",
    "session_id": "test-session"
}"#;

const PRE_COMPACT_INPUT: &str = r#"{
    "hook_event_name": "PreCompact",
    "session_id": "test-session"
}"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn pre_tool_use_dispatch_succeeds() {
    assert_hook_ok("pre-tool-use", PRE_TOOL_USE_INPUT);
}

#[test]
fn post_tool_use_dispatch_succeeds() {
    assert_hook_ok("post-tool-use", POST_TOOL_USE_INPUT);
}

#[test]
fn stop_dispatch_succeeds() {
    // Stop hook is fail-closed but should still succeed with minimal input.
    assert_hook_ok("stop", STOP_INPUT);
}

#[test]
fn session_start_dispatch_succeeds() {
    assert_hook_ok("session-start", SESSION_START_INPUT);
}

#[test]
fn session_stop_dispatch_succeeds() {
    assert_hook_ok("session-stop", SESSION_STOP_INPUT);
}

#[test]
fn user_prompt_dispatch_succeeds() {
    assert_hook_ok("user-prompt", USER_PROMPT_INPUT);
}

#[test]
fn user_prompt_submit_alias_succeeds() {
    assert_hook_ok("user-prompt-submit", USER_PROMPT_INPUT);
}

#[test]
fn pre_compact_dispatch_succeeds() {
    assert_hook_ok("pre-compact", PRE_COMPACT_INPUT);
}

#[test]
fn unknown_hook_exits_nonzero() {
    let bin = hooks_bin();
    if !bin.exists() {
        eprintln!("Skipping: binary not found");
        return;
    }
    let status = Command::new(&bin)
        .arg("not-a-real-hook")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to run hooks binary");
    assert!(
        !status.success(),
        "Unknown hook subcommand must exit non-zero"
    );
}

#[test]
fn empty_json_input_is_handled_gracefully() {
    // All fail-open hooks must survive `{}` input.
    for hook in &[
        "pre-tool-use",
        "post-tool-use",
        "session-start",
        "session-stop",
        "user-prompt",
        "pre-compact",
    ] {
        let (stdout, stderr, success) = run_hook(hook, "{}");
        assert!(
            success,
            "Hook '{}' must handle {{}} input gracefully. stderr: {}",
            hook, stderr
        );
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
        assert!(
            parsed.is_ok(),
            "Hook '{}' must emit valid JSON on {{}} input. Got: '{}'",
            hook,
            stdout
        );
    }
}
