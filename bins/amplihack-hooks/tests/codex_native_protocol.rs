//! Codex wire contract at the actual multicall protocol boundary.
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn hook(subcommand: &str, payload: Value) -> Value {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_amplihack-hooks"))
        .arg(subcommand)
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("AMPLIHACK_HOME", home.path().join(".amplihack"))
        .env("CODEX_HOME", home.path().join(".codex"))
        .env("AMPLIHACK_AGENT_BINARY", "codex")
        .env("AMPLIHACK_RECIPE_RUN_ID", "test-leaf-run")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.ends_with(b"\n"));
    serde_json::from_slice(&output.stdout).expect("stdout is exactly one native JSON object")
}
#[test]
fn unknown_codex_event_has_unversioned_empty_output() {
    assert_eq!(
        hook("stop", json!({"hook_event_name":"FutureCodexEvent"})),
        json!({})
    );
}
#[test]
fn stop_recursion_guard_and_session_end_never_continue_or_emit_generic_versions() {
    let stop = hook(
        "stop",
        json!({"hook_event_name":"Stop", "stop_hook_active":true, "transcript_path":null, "last_assistant_message":null}),
    );
    assert_eq!(stop, json!({}));
    let end = hook(
        "session-stop",
        json!({"hook_event_name":"SessionEnd", "reason":"exit", "transcript_path":null}),
    );
    assert_eq!(end, json!({}));
}
#[test]
fn malformed_codex_input_fails_open_without_protocol_wrappers() {
    assert_eq!(
        hook(
            "stop",
            json!({"hook_event_name":"Stop", "stop_hook_active":"invalid"})
        ),
        json!({})
    );
}

#[test]
fn codex_native_shell_denial_uses_permission_decision_without_losing_shared_cwd_guard() {
    let output = hook(
        "pre-tool-use",
        json!({
            "hook_event_name":"PreToolUse", "tool_name":"shell_command", "tool_use_id":"tool-1",
            "tool_input":{"command":"rm -rf ."}, "transcript_path":null
        }),
    );
    assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        output["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty())
    );
    assert!(output.get("version").is_none());
}
