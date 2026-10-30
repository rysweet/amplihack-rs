//! Issue #1794: the `tail -f` device guard at the hook binary's wire boundary,
//! for each host: Claude Code, Copilot and Codex.
//!
//! The child runs with `env_clear()`: only `PATH` is kept, and `HOME`,
//! `AMPLIHACK_HOME` and `CODEX_HOME` point into a temp dir, so it never sees
//! the developer's `~/.claude`, `~/.amplihack` or
//! `AMPLIHACK_CLAUDE_PROFILE_ROOT` (#1699). Nothing here runs `tail`.

use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

const DENIED: &str = "tail -f /dev/null";
const ALLOWED: &str = "tail -f some.log";

fn hook(payload: &Value, codex: bool) -> Value {
    let home = tempfile::tempdir().expect("temp HOME");
    let mut command = Command::new(env!("CARGO_BIN_EXE_amplihack-hooks"));
    command
        .arg("pre-tool-use")
        .env_clear()
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("AMPLIHACK_HOME", home.path().join(".amplihack"))
        .env("CODEX_HOME", home.path().join(".codex"));
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    if codex {
        command.env("AMPLIHACK_HOOK_PROTOCOL", "codex");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("amplihack-hooks must start");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("write payload");
    let output = child.wait_with_output().expect("hook output");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON object ({e}): {:?}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn reason_names_wait(reason: &Value) {
    let reason = reason.as_str().unwrap_or_default();
    assert!(reason.contains("amplihack wait"), "reason: {reason:?}");
    assert!(reason.contains("tail -f"), "reason: {reason:?}");
}

/// Claude Code and Copilot receive the full deny superset.
fn assert_full_deny(output: &Value) {
    assert_eq!(output["block"], true, "{output}");
    assert_eq!(output["decision"], "block", "{output}");
    assert_eq!(output["permissionDecision"], "deny", "{output}");
    assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(
        output["hookSpecificOutput"]["permissionDecision"], "deny",
        "{output}"
    );
    reason_names_wait(&output["message"]);
    reason_names_wait(&output["reason"]);
    reason_names_wait(&output["permissionDecisionReason"]);
    reason_names_wait(&output["hookSpecificOutput"]["permissionDecisionReason"]);
}

/// Codex receives only `hookSpecificOutput`, with the guard's own reason.
fn assert_codex_deny(output: &Value) {
    assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(
        output["hookSpecificOutput"]["permissionDecision"], "deny",
        "{output}"
    );
    reason_names_wait(&output["hookSpecificOutput"]["permissionDecisionReason"]);
    assert!(output.get("block").is_none(), "{output}");
    assert!(output.get("permissionDecision").is_none(), "{output}");
}

fn assert_not_denied(output: &Value) {
    assert!(output.get("block").is_none(), "{output}");
    assert_ne!(output["permissionDecision"], "deny", "{output}");
    assert_ne!(
        output["hookSpecificOutput"]["permissionDecision"], "deny",
        "{output}"
    );
}

fn claude_payload(command: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse", "session_id": "s-1794",
        "tool_name": "Bash", "tool_input": {"command": command},
        "transcript_path": null
    })
}

/// Copilot's shape: no event name, `toolName`, and a JSON-encoded string
/// under `toolArgs`.
fn copilot_payload(command: &str) -> Value {
    json!({
        "sessionId": "copilot-1794", "timestamp": 1_700_000_000u64, "cwd": "/repo",
        "toolName": "bash",
        "toolArgs": json!({"command": command}).to_string()
    })
}

fn codex_payload(cmd: Value) -> Value {
    json!({
        "hook_event_name": "PreToolUse", "tool_name": "exec_command",
        "tool_use_id": "tool-1794", "tool_input": {"cmd": cmd},
        "transcript_path": null
    })
}

#[test]
fn claude_bash_is_denied() {
    assert_full_deny(&hook(&claude_payload(DENIED), false));
}

#[test]
fn claude_bash_allows_an_ordinary_log() {
    assert_not_denied(&hook(&claude_payload(ALLOWED), false));
}

#[test]
fn claude_bash_allows_a_container_keep_alive() {
    assert_not_denied(&hook(
        &claude_payload("docker run -d img tail -f /dev/null"),
        false,
    ));
}

#[test]
fn copilot_bash_is_denied() {
    assert_full_deny(&hook(&copilot_payload(DENIED), false));
}

#[test]
fn copilot_bash_allows_an_ordinary_log() {
    assert_not_denied(&hook(&copilot_payload(ALLOWED), false));
}

#[test]
fn codex_exec_command_string_is_denied() {
    assert_codex_deny(&hook(&codex_payload(json!(DENIED)), true));
}

#[test]
fn codex_exec_command_argv_is_denied() {
    assert_codex_deny(&hook(
        &codex_payload(json!(["tail", "-f", "/dev/null"])),
        true,
    ));
}

#[test]
fn codex_exec_command_allows_an_ordinary_log() {
    assert_not_denied(&hook(&codex_payload(json!(ALLOWED)), true));
    assert_not_denied(&hook(
        &codex_payload(json!(["tail", "-f", "some.log"])),
        true,
    ));
}

#[test]
fn codex_argv_shell_c_string_is_not_reparsed() {
    assert_not_denied(&hook(&codex_payload(json!(["bash", "-lc", DENIED])), true));
}

#[test]
fn deny_never_echoes_the_command() {
    let command = format!("GH_TOKEN=zq-secret-7f3a {DENIED}");
    for (payload, codex) in [
        (claude_payload(&command), false),
        (copilot_payload(&command), false),
        (codex_payload(json!(command)), true),
    ] {
        let output = hook(&payload, codex);
        assert_eq!(
            output["hookSpecificOutput"]["permissionDecision"], "deny",
            "{output}"
        );
        assert!(!output.to_string().contains("zq-secret-7f3a"), "{output}");
    }
}
