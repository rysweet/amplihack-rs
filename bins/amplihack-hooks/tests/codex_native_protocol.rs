//! Codex wire contract at the actual multicall protocol boundary.
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn hook(subcommand: &str, payload: Value) -> Value {
    hook_bytes(subcommand, &payload.to_string(), "codex")
}
fn hook_bytes(subcommand: &str, payload: &str, provider: &str) -> Value {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_amplihack-hooks"))
        .arg(subcommand)
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("AMPLIHACK_HOME", home.path().join(".amplihack"))
        .env("CODEX_HOME", home.path().join(".codex"))
        .env("AMPLIHACK_AGENT_BINARY", provider)
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
        .write_all(payload.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("hook input exceeds")
            || provider == "codex"
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

#[test]
fn oversized_codex_security_input_denies_explicitly() {
    let output = hook(
        "pre-tool-use",
        json!({"tool_name":"shell_command",
        "tool_input":{"command":"rm -rf .", "padding":"x".repeat(4 * 1024 * 1024)}}),
    );
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
}
#[test]
fn malformed_codex_security_input_denies_explicitly() {
    let output = hook("pre-tool-use", json!({"tool_name":42}));
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
}

#[test]
fn codex_exact_limit_is_processed_and_over_limit_denies() {
    let base =
        json!({"tool_name":"shell_command","tool_input":{"command":"printf safe"}}).to_string();
    let exact = format!("{}{}", base, " ".repeat(4 * 1024 * 1024 - base.len()));
    assert_ne!(
        hook_bytes("pre-tool-use", &exact, "codex")["hookSpecificOutput"]["permissionDecision"],
        "deny"
    );
    let over = format!("{exact} ");
    assert_eq!(
        hook_bytes("pre-tool-use", &over, "codex")["hookSpecificOutput"]["permissionDecision"],
        "deny"
    );
}
#[test]
fn legacy_providers_continue_processing_large_post_tool_payloads() {
    let payload = json!({"hook_event_name":"PostToolUse","tool_name":"Bash", "tool_input":{"command":"printf safe"},
        "tool_result":{"output":"x".repeat(4 * 1024 * 1024 + 1)}}).to_string();
    for provider in ["claude", "copilot"] {
        hook_bytes("post-tool-use", &payload, provider);
    }
}
