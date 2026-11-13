//! Issue #2585: launcher notices must not reach stdout.
//!
//! A recipe agent step runs `amplihack claude --dangerously-skip-permissions
//! -p <prompt>` (see `claude_process.rs`, `build_command`) and parses the
//! child's stdout as the step result. Before the fix the launcher printed
//! `ℹ NODE_OPTIONS=... (saved preference)` with `println!`, so the step
//! result began with that line instead of the agent's output.
//!
//! These tests launch the real binary through that argv, with a fake
//! `claude` that prints one JSON object, and a saved `node.heap_consent`
//! preference in a temporary `HOME`. Every path the run may read or write
//! lies under one temp dir: `HOME`, `AMPLIHACK_HOME`, `CLAUDE_CONFIG_DIR`,
//! `AMPLIHACK_CLAUDE_PROFILE_ROOT` and the working directory, which is not a
//! git repository. The environment is set on the child only.
//!
//! The `(saved preference)` text needs RAM detection, which reads
//! `/proc/meminfo`, so the tests run on Linux only.

#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

/// What the fake agent writes to stdout, byte for byte.
const AGENT_STDOUT: &[u8] = b"{\"ok\":true}\n";
/// What the fake agent writes to stderr, so a test can tell the agent's
/// stderr from the launcher's.
const AGENT_STDERR_MARKER: &str = "fake-agent-stderr-2585";

/// The fake `claude`: answers `--version` like Claude Code, otherwise
/// ignores its arguments, prints one JSON object on stdout and a marker on
/// stderr, and exits 0.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
printf '%s\n' '{"ok":true}'
printf '%s\n' 'fake-agent-stderr-2585' >&2
exit 0
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    amplihack_home: PathBuf,
    profiles: PathBuf,
    config: PathBuf,
    work: PathBuf,
    tools: PathBuf,
}

impl Fixture {
    /// A fixture whose user config saves `node.heap_consent = <consent>`.
    fn with_heap_consent(consent: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let amplihack_home = root.join("amplihack-home");
        let profiles = root.join("profiles");
        let config = root.join("claude-config");
        let work = root.join("work");
        let tools = root.join("tools");
        for d in [&home, &amplihack_home, &config, &work, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        fs::create_dir_all(&profiles).unwrap();
        fs::set_permissions(&profiles, fs::Permissions::from_mode(0o700)).unwrap();

        // `user_config::heap_consent()` reads only `$HOME/.amplihack/config.toml`.
        let user_config = home.join(".amplihack").join("config.toml");
        fs::create_dir_all(user_config.parent().unwrap()).unwrap();
        fs::write(&user_config, format!("[node]\nheap_consent = {consent}\n")).unwrap();

        crate::support::exec_stub::write_executable(&tools.join("claude"), FAKE_CLAUDE, 0o755);

        Self {
            _tmp: tmp,
            home,
            amplihack_home,
            profiles,
            config,
            work,
            tools,
        }
    }

    /// The command a recipe agent step runs, isolated, with only this
    /// fixture's environment.
    fn agent_step(&self) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(["claude", "--dangerously-skip-permissions", "-p", "step"])
            .current_dir(&self.work)
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", &self.amplihack_home)
            .env("CLAUDE_CONFIG_DIR", &self.config)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.profiles)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_GOVERNOR", "off");
        c
    }

    fn run_agent_step(&self) -> Output {
        self.agent_step().output().expect("run amplihack")
    }
}

fn describe(out: &Output) -> String {
    format!(
        "status: {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The acceptance test from #2585: with a saved consent, stdout is exactly
/// the agent's output and the NODE_OPTIONS notice is on stderr.
#[test]
fn agent_step_stdout_is_only_agent_output_with_saved_heap_consent() {
    let fx = Fixture::with_heap_consent(true);
    let out = fx.run_agent_step();

    assert!(out.status.success(), "launch failed\n{}", describe(&out));
    assert_eq!(
        out.stdout,
        AGENT_STDOUT,
        "stdout must be the agent's output and nothing else\n{}",
        describe(&out)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("NODE_OPTIONS=--max-old-space-size=")
            && stderr.contains("(saved preference)"),
        "the saved-preference NODE_OPTIONS notice must be on stderr\n{}",
        describe(&out)
    );
}

/// A declined consent prints the "skipped (saved preference)" notice, which
/// is a different branch of `display_memory_config`. It must also stay off
/// stdout.
#[test]
fn agent_step_stdout_is_only_agent_output_with_declined_heap_consent() {
    let fx = Fixture::with_heap_consent(false);
    let out = fx.run_agent_step();

    assert!(out.status.success(), "launch failed\n{}", describe(&out));
    assert_eq!(
        out.stdout,
        AGENT_STDOUT,
        "stdout must be the agent's output and nothing else\n{}",
        describe(&out)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("NODE_OPTIONS memory config: skipped (saved preference)"),
        "the skipped NODE_OPTIONS notice must be on stderr\n{}",
        describe(&out)
    );
}

/// Moving notices to stderr must not swap the agent's own streams: its
/// stderr stays on stderr and never reaches stdout.
#[test]
fn agent_stderr_passes_through_to_stderr_not_stdout() {
    let fx = Fixture::with_heap_consent(true);
    let out = fx.run_agent_step();

    assert!(out.status.success(), "launch failed\n{}", describe(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stdout.contains(AGENT_STDERR_MARKER),
        "the agent's stderr leaked to stdout\n{}",
        describe(&out)
    );
    assert!(
        stderr.contains(AGENT_STDERR_MARKER),
        "the agent's stderr must reach the launcher's stderr\n{}",
        describe(&out)
    );
}
