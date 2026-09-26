//! Issue #1482, outside in: spawn the Claude command the launcher builds and
//! check the `IS_SANDBOX` the child process actually sees.
//!
//! Each scenario re-runs this test binary with a controlled environment (the
//! decision reads the real process environment, uid and filesystem, so it
//! needs a process of its own) and a fake `claude` first on `PATH` that prints
//! the `IS_SANDBOX` it inherited. Expectations follow the real uid, so the
//! root branches run in a root container and the non-root ones in CI.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use amplihack_launcher::flag_matrix::AgentBinary;
use amplihack_launcher::prompt_delivery::build_tool_command_with_prompt_delivery;
use amplihack_utils::prompt_delivery::PromptDelivery;

const PROBE_ENV: &str = "AMPLIHACK_TEST_ROOT_SANDBOX_PROBE";

/// Runs only when re-executed by [`claude_child_sees_the_root_sandbox_decision`].
#[test]
#[ignore = "helper process for claude_child_sees_the_root_sandbox_decision"]
fn probe_spawn_claude() {
    if std::env::var_os(PROBE_ENV).is_none() {
        return;
    }
    match build_tool_command_with_prompt_delivery(
        AgentBinary::Claude,
        Path::new("."),
        &[],
        "hello",
        PromptDelivery::Argv,
    ) {
        Err(error) => println!("BUILD_ERR {error}"),
        Ok(mut delivered) => {
            let output = delivered.command.output().expect("fake claude runs");
            print!("CHILD {}", String::from_utf8_lossy(&output.stdout));
        }
    }
}

struct Outcome {
    stdout: String,
    stderr: String,
}

fn run_probe(fake_bin: &Path, env: &[(&str, &str)]) -> Outcome {
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "probe_spawn_claude", "--ignored", "--nocapture"])
        .args(["--test-threads", "1"])
        .env(PROBE_ENV, "1")
        .env("PATH", path)
        .env_remove("IS_SANDBOX")
        .env_remove("CLAUDE_CODE_BUBBLEWRAP")
        .env_remove("CLAUDE_CODE_REMOTE")
        .env_remove("AMPLIHACK_PROMPT_DELIVERY");
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().expect("probe process runs");
    assert!(output.status.success(), "probe failed: {output:?}");
    Outcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn is_root() -> bool {
    let output = Command::new("id").arg("-u").output().expect("id -u runs");
    String::from_utf8_lossy(&output.stdout).trim() == "0"
}

#[test]
fn claude_child_sees_the_root_sandbox_decision() {
    let fake_bin = tempfile::tempdir().unwrap();
    let fake_claude = fake_bin.path().join("claude");
    std::fs::write(
        &fake_claude,
        "#!/bin/sh\nprintf 'IS_SANDBOX=%s\\n' \"${IS_SANDBOX-<unset>}\"\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let root = is_root();
    let notice = "passing IS_SANDBOX=1 to claude";

    // A Claude Code cloud session with IS_SANDBOX unset.
    let remote = run_probe(fake_bin.path(), &[("CLAUDE_CODE_REMOTE", "true")]);
    if root {
        assert!(
            remote.stdout.contains("CHILD IS_SANDBOX=1"),
            "{}",
            remote.stdout
        );
        assert!(remote.stderr.contains(notice), "{}", remote.stderr);
        assert!(
            remote.stderr.contains("CLAUDE_CODE_REMOTE=true"),
            "{}",
            remote.stderr
        );
    } else {
        assert!(
            remote.stdout.contains("CHILD IS_SANDBOX=<unset>"),
            "{}",
            remote.stdout
        );
        assert!(!remote.stderr.contains(notice), "{}", remote.stderr);
    }

    // The user's affirmative spelling reaches claude as the 1 it accepts.
    let yes = run_probe(fake_bin.path(), &[("IS_SANDBOX", "yes")]);
    let expected = if root { "1" } else { "yes" };
    assert!(
        yes.stdout.contains(&format!("CHILD IS_SANDBOX={expected}")),
        "{}",
        yes.stdout
    );
    assert!(!yes.stderr.contains(notice), "{}", yes.stderr);

    // The opt-out is never overridden, even inside a detected sandbox.
    let refused = run_probe(
        fake_bin.path(),
        &[("IS_SANDBOX", "0"), ("CLAUDE_CODE_REMOTE", "true")],
    );
    if root {
        assert!(
            refused.stdout.contains("BUILD_ERR") && refused.stdout.contains("IS_SANDBOX=1"),
            "{}",
            refused.stdout
        );
        assert!(!refused.stdout.contains("CHILD"), "claude must not start");
    } else {
        assert!(
            refused.stdout.contains("CHILD IS_SANDBOX=0"),
            "{}",
            refused.stdout
        );
    }
}
