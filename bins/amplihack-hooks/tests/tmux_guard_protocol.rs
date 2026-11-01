//! Issue #1733: the tmux server guard denies the incident command through the
//! real hook binary for every host protocol. tmux is never run: the command
//! only reaches the hook as JSON on stdin.
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

const INCIDENT: &str = "TMUX_TMPDIR=$SB/tmuxsock tmux kill-server";
const SCOPED: &str = "env -u TMUX tmux -S /tmp/tx1733/a.sock kill-server";
const REQUIRED: &[&str] = &[
    "-S <private socket>",
    "env -u TMUX",
    "TMUX_TMPDIR alone does not redirect tmux while $TMUX is set",
];

/// Runs the hook with a cleared environment: only `PATH` is inherited, so
/// nothing from the caller (`TMUX`, `CLAUDE_CONFIG_DIR`, a profile root, the
/// auto-drive marker) reaches it, and every path it could write to is inside
/// a temporary directory.
fn hook_bytes(subcommand: &str, payload: &str, provider: &str) -> Value {
    hook_bytes_with_env(subcommand, payload, provider, &[])
}

/// [`hook_bytes`] with `extra_env` added to the cleared environment.
fn hook_bytes_with_env(
    subcommand: &str,
    payload: &str,
    provider: &str,
    extra_env: &[(&str, &str)],
) -> Value {
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_amplihack-hooks"));
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
        .arg(subcommand)
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("AMPLIHACK_HOME", home.path().join(".amplihack"))
        .env("CODEX_HOME", home.path().join(".codex"))
        .env("AMPLIHACK_AGENT_BINARY", provider)
        .env("AMPLIHACK_RECIPE_RUN_ID", "test-leaf-run");
    if provider == "codex" {
        command.env("AMPLIHACK_HOOK_PROTOCOL", "codex");
    }
    command.envs(extra_env.iter().copied());
    let mut child = command
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
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON object ({e}): {:?}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn hook(payload: Value, provider: &str) -> Value {
    hook_bytes("pre-tool-use", &payload.to_string(), provider)
}

/// [`hook`] with XPIA opted in (it is off by default, #1734, and the harness
/// clears the environment).
fn hook_xpia_on(payload: Value, provider: &str) -> Value {
    hook_bytes_with_env(
        "pre-tool-use",
        &payload.to_string(),
        provider,
        &[("XPIA_ENABLED", "true")],
    )
}

fn assert_reason(reason: &Value) {
    let reason = reason.as_str().unwrap_or_default();
    for needle in REQUIRED {
        assert!(reason.contains(needle), "reason lacks {needle:?}: {reason}");
    }
}

/// Claude Code and Copilot receive the full superset.
fn assert_full_deny(output: &Value) {
    assert_eq!(output["block"], true, "{output}");
    assert_eq!(output["decision"], "block", "{output}");
    assert_eq!(output["permissionDecision"], "deny", "{output}");
    assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_reason(&output["message"]);
    assert_reason(&output["reason"]);
    assert_reason(&output["permissionDecisionReason"]);
    assert_reason(&output["hookSpecificOutput"]["permissionDecisionReason"]);
}

/// Codex receives only `hookSpecificOutput`, with the guard's own reason
/// rather than the generic security-failure reason.
fn assert_codex_deny(output: &Value) {
    assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(
        output["hookSpecificOutput"]["permissionDecision"], "deny",
        "{output}"
    );
    assert_reason(&output["hookSpecificOutput"]["permissionDecisionReason"]);
    assert!(output.get("block").is_none(), "{output}");
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
        "hook_event_name": "PreToolUse", "session_id": "s-1733",
        "tool_name": "Bash", "tool_input": {"command": command},
        "transcript_path": null
    })
}

/// Claude Code's `Monitor` tool: its `command` is a shell command or script
/// run in the same shell environment as `Bash`.
fn claude_monitor_payload(command: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse", "session_id": "s-1733",
        "tool_name": "Monitor",
        "tool_input": {"command": command, "description": "wait for the run"},
        "transcript_path": null
    })
}

fn copilot_payload(command: &str) -> Value {
    json!({
        "sessionId": "copilot-1733", "timestamp": 1_700_000_000u64, "cwd": "/repo",
        "toolName": "bash",
        "toolArgs": json!({"command": command}).to_string()
    })
}

fn codex_exec_payload(command: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse", "tool_name": "exec_command",
        "tool_use_id": "tool-1733", "tool_input": {"cmd": command},
        "transcript_path": null
    })
}

fn codex_shell_argv_payload(command: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse", "tool_name": "shell",
        "tool_use_id": "tool-1733", "tool_input": {"command": ["bash", "-lc", command]},
        "transcript_path": null
    })
}

#[test]
fn claude_code_denies_the_incident_command() {
    assert_full_deny(&hook(claude_payload(INCIDENT), "claude"));
}

#[test]
fn claude_code_allows_a_scoped_kill_server() {
    assert_not_denied(&hook(claude_payload(SCOPED), "claude"));
}

/// Crusty round 6, concern 1: the `Monitor` tool reaches the guard.
#[test]
fn claude_code_monitor_denies_the_incident_command() {
    for command in [
        INCIDENT,
        "until [ -f done ]; do sleep 1; done; tmux kill-session -a",
    ] {
        assert_full_deny(&hook(claude_monitor_payload(command), "claude"));
    }
}

#[test]
fn claude_code_monitor_allows_a_scoped_kill_server() {
    assert_not_denied(&hook(claude_monitor_payload(SCOPED), "claude"));
}

#[test]
fn copilot_denies_the_incident_command() {
    assert_full_deny(&hook(copilot_payload(INCIDENT), "copilot"));
}

#[test]
fn copilot_allows_a_scoped_kill_server() {
    assert_not_denied(&hook(copilot_payload(SCOPED), "copilot"));
}

#[test]
fn codex_exec_command_denies_the_incident_command() {
    assert_codex_deny(&hook(codex_exec_payload(INCIDENT), "codex"));
}

#[test]
fn codex_exec_command_allows_a_scoped_kill_server() {
    assert_not_denied(&hook(codex_exec_payload(SCOPED), "codex"));
}

#[test]
fn codex_shell_argv_denies_the_incident_command() {
    assert_codex_deny(&hook(codex_shell_argv_payload(INCIDENT), "codex"));
}

#[test]
fn codex_shell_argv_allows_a_scoped_kill_server() {
    assert_not_denied(&hook(codex_shell_argv_payload(SCOPED), "codex"));
}

#[test]
fn deny_never_echoes_the_command() {
    let command = format!("GH_TOKEN=zq-secret-7f3a {INCIDENT}");
    for (payload, provider) in [
        (claude_payload(&command), "claude"),
        (copilot_payload(&command), "copilot"),
        (codex_exec_payload(&command), "codex"),
    ] {
        let output = hook(payload, provider);
        assert!(!output.to_string().contains("zq-secret-7f3a"), "{output}");
    }
}

/// Crusty round 1, concern 1: a socket path read from tmux is the user's
/// server, including in the deny message's own `env -u TMUX` form.
#[test]
fn every_host_denies_a_socket_path_taken_from_tmux() {
    let command =
        "sock=$(tmux display -p '#{socket_path}'); env -u TMUX tmux -S \"$sock\" kill-server";
    assert_full_deny(&hook(claude_payload(command), "claude"));
    assert_full_deny(&hook(copilot_payload(command), "copilot"));
    assert_codex_deny(&hook(codex_exec_payload(command), "codex"));
}

/// Crusty round 1, concerns 3 and 4: amplihack's own launches and stops are
/// allowed, including inline task text that mentions the guard.
#[test]
fn claude_code_allows_amplihack_own_tmux_use() {
    for command in [
        "amplihack recipe run default-workflow --detach -c task_description=\"Follow-up to #1733: tmux kill-server guard false positives\"",
        "amplihack recipe run default-workflow --detach --context-from-file task_description=/tmp/task.md",
        "amplihack recipe stop amp-default-workflow-repo-20261029-101500",
        "tmux kill-session -t '=amp-default-workflow-repo-20261029-101500'",
    ] {
        assert_not_denied(&hook(claude_payload(command), "claude"));
    }
}

/// Crusty round 3, concern 2: a targeted kill fed by `tmux ls` is allowed,
/// because stdin flows left to right and the `tmux ls` reads nothing the
/// later stages hold. A kill piped into an unscoped control client is still
/// denied.
#[test]
fn claude_code_judges_pipeline_stages_in_order() {
    for command in [
        "tmux ls -F '#S' | grep '^amp-' | xargs -r -n1 tmux kill-session -t",
        "tmux list-sessions -F '#{session_name}' | grep '^amp-' | xargs -I{} tmux kill-session -t {}",
        "tmux ls 2>/dev/null | grep -c kill-session",
    ] {
        assert_not_denied(&hook(claude_payload(command), "claude"));
    }
    for command in [
        "echo kill-server | tmux -C attach",
        "printf 'ls\\nkill-server\\n' | tmux -C attach",
    ] {
        assert_full_deny(&hook(claude_payload(command), "claude"));
    }
}

/// Crusty round 8: tmux spelt as an expansion or with quoting, inside an
/// embedded script (concern 1) or as a program word fed a kill on stdin
/// (concern 2), gets the guard's enforceable deny, as the literal spelling
/// does, and the same spellings with a targeted kill or no fed kill are
/// allowed.
#[test]
fn claude_code_denies_tmux_spelt_as_an_expansion_or_with_quoting() {
    for command in [
        "bash -c '${TMUX_BIN:-tmux} kill-server'",
        "echo \"$(${TMUX_BIN:-tmux} kill-server)\"",
        "bash -c \"t''mux kill-server\"",
        "bash -c 't\\mux kill-session -a'",
        "echo kill-server | \"$(command -v tmux)\" -C attach",
    ] {
        assert_full_deny(&hook(claude_payload(command), "claude"));
    }
    for command in [
        "bash -c \"t''mux kill-session -t amp-x\"",
        "\"$(command -v tmux)\" -C attach </dev/null",
    ] {
        assert_not_denied(&hook(claude_payload(command), "claude"));
    }
}

/// Kill commands that XPIA's command-injection pattern also matches: a
/// separator or newline followed by `rm` or `kill…`. The first is the
/// incident command with the usual sandbox clean-up after it; the heredoc is
/// the second incident's form.
const KILLS_XPIA_ALSO_MATCHES: &[&str] = &[
    "TMUX_TMPDIR=$SB/tmuxsock tmux kill-server; rm -rf \"$SB\"",
    "TMUX_TMPDIR=$SB/tmuxsock tmux kill-server && rm -rf \"$SB\"",
    "TMUX_TMPDIR=$SB/tmuxsock tmux kill-server\nrm -rf \"$SB\"",
    "tmux -C attach <<EOF\nkill-server\nEOF",
    "tmux new -d 'sleep 1' \\; kill-server",
    "tmux kill-session -a; rm -f /tmp/qa.log",
    // Crusty round 8, concern 2: the second incident's heredoc with tmux
    // spelt as an expansion, which got only XPIA's bare block at
    // `f66c7c460`.
    "\"$(command -v tmux)\" -C attach <<EOF\nkill-server\nEOF",
    "${TMUX_BIN:-tmux} -C attach <<EOF\nkill-server\nEOF",
];

/// Crusty round 7, concern 1: with XPIA opted in, these commands get the
/// guard's deny, not XPIA's bare `{"block": true}`, which
/// Claude Code does not enforce (#1773). Claude Code acts on
/// `hookSpecificOutput.permissionDecision == "deny"` (or
/// `decision == "block"`), so that is what is asserted; a `block` key alone
/// would not count.
#[test]
fn every_host_gets_the_guard_deny_where_xpia_also_matches() {
    // Control: XPIA is on here and answers a command the guard allows, so
    // each case below would reach XPIA if the guard did not run first.
    let control = hook_xpia_on(claude_payload("echo done; rm -rf \"$SB\""), "claude");
    assert!(
        control["message"]
            .as_str()
            .is_some_and(|m| m.contains("XPIA")),
        "XPIA is not active in this harness: {control}"
    );

    for command in KILLS_XPIA_ALSO_MATCHES {
        for output in [
            hook_xpia_on(claude_payload(command), "claude"),
            hook_xpia_on(claude_monitor_payload(command), "claude"),
            hook_xpia_on(copilot_payload(command), "copilot"),
        ] {
            assert_eq!(
                output["hookSpecificOutput"]["permissionDecision"], "deny",
                "{command:?}: {output}"
            );
            assert_full_deny(&output);
        }
        assert_codex_deny(&hook_xpia_on(codex_exec_payload(command), "codex"));
    }
}

/// XPIA is off by default (#1734) and the guard does not depend on it: with
/// nothing set in the cleared environment, every host still gets the deny, and
/// XPIA does not answer the control above, which shows XPIA is off here.
#[test]
fn every_host_gets_the_guard_deny_with_xpia_at_its_default_off() {
    let control = hook(claude_payload("echo done; rm -rf \"$SB\""), "claude");
    assert!(
        !control["message"]
            .as_str()
            .unwrap_or_default()
            .contains("XPIA"),
        "XPIA must be off here: {control}"
    );

    for command in KILLS_XPIA_ALSO_MATCHES {
        for output in [
            hook(claude_payload(command), "claude"),
            hook(claude_monitor_payload(command), "claude"),
            hook(copilot_payload(command), "copilot"),
        ] {
            assert_full_deny(&output);
        }
        assert_codex_deny(&hook(codex_exec_payload(command), "codex"));
    }
}
