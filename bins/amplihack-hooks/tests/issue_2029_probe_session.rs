//! #2029: run the real hook dispatcher in a disposable child environment.
#![cfg(unix)]

#[path = "../../../tests/support/isolated_hook.rs"]
mod isolated_hook;

use isolated_hook::HookSandbox;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::process::Stdio;

fn session(marker: Option<&str>) -> (HookSandbox, Value) {
    let (sandbox, output, _) = measured_session(marker, false);
    (sandbox, output)
}

fn measured_session(marker: Option<&str>, worker: bool) -> (HookSandbox, Value, String) {
    measured_session_in(HookSandbox::new(), marker, worker)
}

fn measured_session_in(
    sandbox: HookSandbox,
    marker: Option<&str>,
    worker: bool,
) -> (HookSandbox, Value, String) {
    fs::create_dir_all(sandbox.project().join(".git")).unwrap();
    fs::create_dir_all(sandbox.project().join(".claude/context")).unwrap();
    fs::write(
        sandbox
            .project()
            .join(".claude/context/USER_PREFERENCES.md"),
        "ISSUE_2029_ISOLATED_PREFERENCES",
    )
    .unwrap();
    fs::create_dir_all(sandbox.root().join("proc")).unwrap();
    fs::write(
        sandbox.project().join("PROJECT.md"),
        "ISSUE_2029_PROJECT_CONTEXT",
    )
    .unwrap();
    fs::write(
        sandbox.project().join("DISCOVERIES.md"),
        "ISSUE_2029_DISCOVERY",
    )
    .unwrap();
    let mut cmd = sandbox.command(env!("CARGO_BIN_EXE_amplihack-hooks"));
    cmd.env("RUST_LOG", "amplihack_hooks::session_start=debug")
        .env("AMPLIHACK_SESSION_DEPTH", if worker { "1" } else { "0" });
    cmd.arg("session-start")
        .env("CLAUDE_CONFIG_DIR", sandbox.home().join(".claude"))
        .env("CODEX_HOME", sandbox.home().join(".codex"))
        .env("CODEX_PROFILES_ROOT", sandbox.root().join("codex-profiles"))
        .env("XDG_CONFIG_HOME", sandbox.home().join(".config"))
        .env("XDG_STATE_HOME", sandbox.home().join(".local/state"))
        .env("AMPLIHACK_TEST_PROC_ROOT", sandbox.root().join("proc"))
        .env(
            "AMPLIHACK_GRAPH_DB_PATH",
            sandbox.root().join("missing-graph"),
        )
        .env(
            "AMPLIHACK_KUZU_DB_PATH",
            sandbox.root().join("missing-graph"),
        )
        .env("AMPLIHACK_BLARIFY_MODE", "skip")
        .env("AMPLIHACK_DISABLE_BLARIFY", "1")
        .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(marker) = marker {
        cmd.env("AMPLIHACK_USAGE_PROBE", marker);
    } else {
        cmd.env_remove("AMPLIHACK_USAGE_PROBE");
    }
    let mut child = cmd.spawn().unwrap();
    let input = json!({"hook_event_name":"SessionStart", "session_id":"issue-2029-session", "cwd":sandbox.project(), "prompt":"ISSUE_2029_ORIGINAL_REQUEST"});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value = serde_json::from_slice(&out.stdout).expect("hook protocol JSON");
    (sandbox, value, String::from_utf8(out.stderr).unwrap())
}

#[test]
fn usage_probe_session_start_returns_before_context_and_request_initialisation() {
    let (sandbox, output) = session(Some("1"));
    assert_eq!(
        output,
        json!({}),
        "probe must not inject context, initialise graphs or report indexing"
    );
    assert!(
        !sandbox
            .home()
            .join(".amplihack/runtime/sessions/issue-2029-session")
            .exists(),
        "probe must not capture an original request"
    );
    assert!(!sandbox.project().join(".amplihack/index.scip").exists());
    assert!(!sandbox.root().join("missing-graph").exists());
}

#[test]
fn an_ordinary_session_still_loads_project_discoveries_and_preferences() {
    let (_sandbox, output) = session(None);
    let context = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    for expected in [
        "ISSUE_2029_PROJECT_CONTEXT",
        "ISSUE_2029_DISCOVERY",
        "ISSUE_2029_ISOLATED_PREFERENCES",
    ] {
        assert!(context.contains(expected), "missing {expected}: {output}");
    }
    assert_eq!(
        output["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
    assert!(output["hookSpecificOutput"]["indexing_status"].is_string());
}

#[test]
fn a_false_probe_marker_does_not_suppress_an_ordinary_session() {
    for marker in ["", "0"] {
        let (_sandbox, output) = session(Some(marker));
        assert!(
            output["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("ISSUE_2029_PROJECT_CONTEXT"),
            "marker {marker:?}: {output}"
        );
    }
}

#[test]
fn probe_suppression_cannot_bypass_the_tool_security_hook() {
    let sandbox = HookSandbox::new();
    let mut child = sandbox
        .command(env!("CARGO_BIN_EXE_amplihack-hooks"))
        .arg("pre-tool-use")
        .env("AMPLIHACK_USAGE_PROBE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // This is hook input only; no shell executes the supplied command.
    let input = json!({"hook_event_name":"PreToolUse", "tool_name":"Bash", "tool_input":{"command":"rm -rf .; echo done"}});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let output: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        output["block"], true,
        "probe marker bypassed a security hook: {output}"
    );
}

#[test]
fn ordinary_and_recipe_worker_sessions_execute_measured_session_start_stages() {
    for worker in [false, true] {
        let (_sandbox, output, log) = measured_session(None, worker);
        assert_eq!(
            output["hookSpecificOutput"]["hookEventName"],
            "SessionStart"
        );
        for stage in [
            "project-context",
            "discoveries",
            "preferences",
            "workflow-context",
            "version",
            "code-graph-setup",
            "code-graph-compatibility",
            "code-graph-context",
        ] {
            assert!(
                log.contains(stage),
                "worker={worker}: missing {stage}: {log}"
            );
        }
    }
    let (_sandbox, output, log) = measured_session(Some("1"), false);
    assert_eq!(output, json!({}));
    assert!(!log.contains("session-start stage completed"));
}

#[test]
fn a_home_session_captures_registered_ownership_before_any_rotation_ledger_exists() {
    use amplihack_utils::usage_observation::ObservationStore;
    use std::os::unix::fs::PermissionsExt;
    let sandbox = HookSandbox::new();
    let profile = sandbox.profiles().join("work");
    let config = sandbox.home().join(".claude");
    for dir in [&profile, &config] {
        fs::create_dir(dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let credentials = config.join(".credentials.json");
    fs::write(&credentials, b"opaque inode fixture").unwrap();
    fs::set_permissions(&credentials, fs::Permissions::from_mode(0o600)).unwrap();
    let observations = ObservationStore::open(&sandbox.profiles()).unwrap();
    let binding = {
        let mut guard = observations.lock().unwrap();
        let binding = guard.authorised_login("work", "account-work").unwrap();
        guard.set_location(&binding, &credentials).unwrap();
        binding
    };
    assert!(
        !sandbox
            .profiles()
            .join(".auto-switch.d/rotate/ledger.json")
            .exists()
    );
    let (_sandbox, output, _) = measured_session_in(sandbox, None, false);
    assert_eq!(
        output["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
    let sessions = observations.lock().unwrap().sessions().unwrap();
    assert_eq!(
        sessions
            .iter()
            .find(|s| s.session_id == "issue-2029-session")
            .unwrap()
            .binding,
        binding
    );
}
