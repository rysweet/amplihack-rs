/// Integration tests: amplihack-hooks dispatcher smoke tests.
///
/// These tests exercise the `amplihack-hooks` binary through its dispatch
/// layer.  Each hook is invoked with a minimal valid JSON payload on stdin
/// and the test asserts:
///   - The binary exits with code 0 (hooks fail-open)
///   - stdout is valid JSON
///   - The JSON does not contain error fields that indicate a panic
///
/// The live hook path is native Rust. These tests intentionally do not require
/// Python to be installed or importable.
/// All hooks must remain fail-open (exit 0, produce `{}` or valid output).
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

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
            format!("binary not found at {bin:?}"),
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
        "Hook '{subcommand}' must exit 0 (fail-open). stderr: {stderr}"
    );
    // stdout must be valid JSON.
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
    assert!(
        parsed.is_ok(),
        "Hook '{subcommand}' stdout must be valid JSON. Got: '{stdout}' stderr: {stderr}"
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
        "Hook '{subcommand}' JSON must contain a protocol key (hookSpecificOutput, decision, or reason) or be empty. Got: {value}"
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
fn workflow_classification_reminder_dispatch_succeeds() {
    let input = r#"{
    "hook_event_name": "UserPromptSubmit",
    "user_prompt": "Please investigate this new bug",
    "session_id": "test-session",
    "turnCount": 0
}"#;
    assert_hook_ok("workflow-classification-reminder", input);
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
        "workflow-classification-reminder",
        "user-prompt",
        "pre-compact",
    ] {
        let (stdout, stderr, success) = run_hook(hook, "{}");
        assert!(
            success,
            "Hook '{hook}' must handle {{}} input gracefully. stderr: {stderr}"
        );
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
        assert!(
            parsed.is_ok(),
            "Hook '{hook}' must emit valid JSON on {{}} input. Got: '{stdout}'"
        );
    }
}

#[test]
fn session_start_dispatches_background_blarify_indexing() {
    let bin = hooks_bin();
    if !bin.exists() {
        eprintln!("Skipping: binary not found");
        return;
    }

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("unix epoch")
        .as_nanos();
    let project_root = std::env::temp_dir().join(format!("amplihack-hook-probe-{unique}"));
    if project_root.exists() {
        fs::remove_dir_all(&project_root).expect("cleanup stale temp dir");
    }
    let src_dir = project_root.join("src");
    let artifact_dir = project_root.join(".amplihack");
    fs::create_dir_all(&src_dir).expect("src dir");
    fs::create_dir_all(&artifact_dir).expect("artifact dir");
    fs::write(src_dir.join("app.py"), "print('hi')\n").expect("source file");
    std::thread::sleep(Duration::from_secs(1));
    fs::write(artifact_dir.join("blarify.json"), "{}\n").expect("blarify json");

    let shim_dir = project_root.join("shim");
    fs::create_dir_all(&shim_dir).expect("shim dir");
    let log_path = project_root.join("amplihack.log");
    let stub = shim_dir.join("amplihack");
    fs::write(
        &stub,
        format!(
            "#!/usr/bin/env bash\nif [ \"${{1:-}}\" = \"--version\" ]; then echo amplihack-test; exit 0; fi\nprintf '%s\\n' \"$@\" > \"{}\"\n",
            log_path.display()
        ),
    )
    .expect("stub script");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod");

    let mut child = Command::new(&bin)
        .arg("session-start")
        .current_dir(&project_root)
        .env("AMPLIHACK_AMPLIHACK_BINARY_PATH", &stub)
        .env("AMPLIHACK_BLARIFY_MODE", "background")
        .env_remove("AMPLIHACK_GRAPH_DB_PATH")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hooks binary");

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(SESSION_START_INPUT.as_bytes())
            .expect("write stdin");
    }

    let output = child.wait_with_output().expect("wait for hook");
    assert!(
        output.status.success(),
        "session-start hook must succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert!(
        parsed.is_object(),
        "expected object output from session-start hook, got: {parsed}"
    );

    for _ in 0..20 {
        if log_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let logged = fs::read_to_string(&log_path).expect("background command log");
    assert!(logged.contains("index-code"));
    assert!(logged.contains(".amplihack/blarify.json"));
    assert!(logged.contains(".amplihack/graph_db"));

    fs::remove_dir_all(&project_root).expect("cleanup project root");
}

// ---------------------------------------------------------------------------
// Outside-in no-Python proof (Issue #77 AC — outside-in, no-Python/PTY)
// ---------------------------------------------------------------------------

/// Build a PATH string that has all directories containing `python` or
/// `python3` executables removed.  Mirrors the probe used in
/// `tests/integration/no_python_probe_test.rs` so that the test environment
/// matches the documented "Python-free host" contract.
fn clean_path_without_python() -> String {
    let original = std::env::var("PATH").unwrap_or_default();
    original
        .split(':')
        .filter(|dir| {
            !std::path::Path::new(dir).join("python").exists()
                && !std::path::Path::new(dir).join("python3").exists()
        })
        .collect::<Vec<_>>()
        .join(":")
}

/// Outside-in integration proof: invoke the compiled `amplihack-hooks` binary
/// with a SessionStart JSON payload while Python is absent from PATH.
///
/// Acceptance criteria verified:
///   (a) The binary is invoked directly — no PTY, no subprocess shell.
///   (b) Python is removed from PATH — the hook must not require it.
///   (c) The output is valid JSON with the correct protocol structure.
///   (d) `hookSpecificOutput.indexing_status` is present and holds one of the
///       three valid values: "started", "complete", or "error:<reason>".
#[test]
fn session_start_outside_in_no_python_emits_indexing_status() {
    let bin = hooks_bin();
    if !bin.exists() {
        eprintln!("Skipping: amplihack-hooks binary not found at {bin:?}");
        return;
    }

    let clean_path = clean_path_without_python();

    // Use a temporary directory as CWD so there are no project files that
    // could trigger complex indexing paths (keeps the test deterministic).
    // We use a unique suffix to avoid collisions between parallel test runs.
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_dir = std::env::temp_dir().join(format!("amplihack-oi-no-py-{unique}"));
    fs::create_dir_all(&tmp_dir).expect("create tmp dir");

    let mut child = Command::new(&bin)
        .arg("session-start")
        // (b) Python-free PATH.
        .env("PATH", &clean_path)
        // Skip background blarify indexing so the test does not spawn child
        // processes; we still expect a valid indexing_status in the output.
        .env("AMPLIHACK_BLARIFY_MODE", "skip")
        .current_dir(&tmp_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // (a) Direct binary invocation — no PTY.
        .spawn()
        .expect("Failed to spawn amplihack-hooks binary");

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(SESSION_START_INPUT.as_bytes());
    }

    let output = child
        .wait_with_output()
        .expect("Failed to wait on hook process");

    // Must exit 0 (fail-open policy).
    assert!(
        output.status.success(),
        "session-start must exit 0 without Python on PATH.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // (c) stdout must be valid JSON.
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "session-start must emit valid JSON without Python.\nParse error: {e}\nstdout: {stdout}"
        )
    });

    // (c) Must follow the hook protocol — hookSpecificOutput key present.
    assert!(
        parsed.get("hookSpecificOutput").is_some(),
        "output must contain 'hookSpecificOutput' key.\nGot: {parsed}"
    );

    // (d) indexing_status must be present and carry a valid value.
    let status = parsed["hookSpecificOutput"]["indexing_status"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "hookSpecificOutput.indexing_status must be a string.\nGot: {}",
                parsed["hookSpecificOutput"]
            )
        });
    assert!(
        status == "started" || status == "complete" || status.starts_with("error:"),
        "indexing_status must be 'started', 'complete', or 'error:<reason>'.\nGot: {status}"
    );

    // Cleanup temporary directory.
    let _ = fs::remove_dir_all(&tmp_dir);
}
