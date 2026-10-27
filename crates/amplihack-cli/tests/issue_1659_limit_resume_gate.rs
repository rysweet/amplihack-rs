//! Issue #1659 (crusty #1687 round 1): the install-time Claude Code version
//! gate, end to end. A stub `claude` on `PATH` answers `--version` with
//! 2.1.76; the real binary's `install` must leave `StopFailure` out of
//! settings.json, remove the one #1660 wrote, keep every other hook and say
//! why. Unit tests cannot cover this: they never run the host's `claude`.
#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;

fn executable(path: &Path, script: &str) {
    crate::support::exec_stub::write_executable(path, script, 0o700);
}

fn link_host_tools(tools: &Path) {
    for tool in [
        "sh",
        "mkdir",
        "cat",
        "cp",
        "chmod",
        "dirname",
        "rm",
        "mv",
        "git",
        "uname",
        "head",
        "sed",
        "date",
        "cut",
        "sha256sum",
    ] {
        let out = Command::new("/bin/sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        assert!(out.status.success(), "{tool} not on the host PATH");
        std::os::unix::fs::symlink(
            String::from_utf8(out.stdout).unwrap().trim(),
            tools.join(tool),
        )
        .unwrap();
    }
}

/// `amplihack install --local <repo>` in a fresh HOME whose PATH holds only
/// `tools`.
fn install(home: &Path, tools: &Path) -> std::process::Output {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    crate::isolated_amplihack::isolated(amplihack_bin())
        .args(["install", "--local", source.to_str().unwrap()])
        .env("HOME", home)
        .env("PATH", tools)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_SOURCE_ROOT", &source)
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env(
            "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
            tools.join("amplihack-hooks"),
        )
        .env("RECIPE_RUNNER_RS_PATH", tools.join("recipe-runner-rs"))
        .env_remove("AMPLIHACK_ORCHESTRATION_BOOTSTRAP")
        .env_remove("AMPLIHACK_DEFAULT_MODEL")
        .env_remove("AMPLIHACK_HOME")
        .env_remove("AMPLIHACK_ASSET_RESOLVER")
        .env_remove("AMPLIHACK_SESSION_ID")
        .env_remove("AMPLIHACK_SESSION_DEPTH")
        .env_remove("AMPLIHACK_SKIP_AUTO_INSTALL")
        .env_remove("AMPLIHACK_CLAUDE_BINARY_PATH")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        // `install` resolves hook scope from its cwd; keep it off the process
        // cwd, which this binary shares with every other test module.
        .current_dir(home)
        .output()
        .unwrap()
}

fn stop_failure_wrapper(hooks_bin: &Path) -> Value {
    json!({
        "matcher": "rate_limit",
        "hooks": [{
            "type": "command",
            "command": format!("\"{}\" limit-resume", hooks_bin.display()),
            "timeout": 600,
            "asyncRewake": true
        }]
    })
}

fn amplihack_stop_failure(settings: &Value) -> bool {
    settings["hooks"]["StopFailure"]
        .as_array()
        .is_some_and(|wrappers| {
            wrappers.iter().any(|w| {
                w["hooks"][0]["command"]
                    .as_str()
                    .is_some_and(|c| c.contains("amplihack-hooks") && c.ends_with("limit-resume"))
            })
        })
}

fn run(claude_version: &str) -> (Value, String) {
    let home = tempfile::tempdir().unwrap();
    let tools = home.path().join("tools");
    fs::create_dir(&tools).unwrap();
    link_host_tools(&tools);
    executable(&tools.join("node"), "#!/bin/sh\necho v22.14.0\n");
    executable(&tools.join("npm"), "#!/bin/sh\necho 10.9.0\n");
    executable(&tools.join("amplihack-hooks"), "#!/bin/sh\necho '{}'\n");
    executable(
        &tools.join("recipe-runner-rs"),
        "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[]}'\n",
    );
    executable(
        &tools.join("claude"),
        &format!(
            "#!/bin/sh\ncase \"$*\" in\n *--version*) echo '{claude_version} (Claude Code)';;\n *) printf '%s\\n' \"$*\" >> \"$HOME/claude-calls\";;\nesac\n"
        ),
    );
    // settings.json as #1660 left it, with a user's own StopFailure hook.
    let claude_dir = home.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    let user_hook = json!({
        "matcher": "overloaded",
        "hooks": [{"type": "command", "command": "/usr/local/bin/notify-me"}]
    });
    fs::write(
        claude_dir.join("settings.json"),
        json!({"hooks": {"StopFailure": [
            stop_failure_wrapper(&tools.join("amplihack-hooks")),
            user_hook
        ]}})
        .to_string(),
    )
    .unwrap();

    let out = install(home.path(), &tools);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "install failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let settings: Value =
        serde_json::from_str(&fs::read_to_string(claude_dir.join("settings.json")).unwrap())
            .unwrap();
    (settings, stdout)
}

#[test]
fn install_with_claude_2_1_76_drops_stop_failure_and_says_why() {
    let (settings, stdout) = run("2.1.76");

    assert!(!amplihack_stop_failure(&settings), "{settings:#}");
    assert_eq!(
        settings["hooks"]["StopFailure"],
        json!([{
            "matcher": "overloaded",
            "hooks": [{"type": "command", "command": "/usr/local/bin/notify-me"}]
        }]),
        "the user's own StopFailure hook stays: {settings:#}"
    );
    assert!(
        settings["hooks"]["SessionStart"].is_array(),
        "every other hook is still registered: {settings:#}"
    );
    assert!(
        stdout.contains("Claude Code 2.1.76 is older than")
            && stdout.contains("resume hook (StopFailure) is not registered"),
        "no version-gate warning:\n{stdout}"
    );
}

#[test]
fn install_with_a_current_claude_keeps_stop_failure() {
    let (settings, stdout) = run("2.1.306");

    assert!(amplihack_stop_failure(&settings), "{settings:#}");
    assert!(
        !stdout.contains("resume hook (StopFailure) is not registered"),
        "warned for a current claude:\n{stdout}"
    );
}
