//! Subprocess execution helpers for TUI testing.
//!
//! Extracted from simple_tui to keep modules under 400 lines.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------

/// Outcome of a single subprocess invocation.
pub(crate) enum CmdOutcome {
    Success(String),
    Failed(String),
    Timeout,
    Error(String),
}

/// Run a command with a wall-clock timeout.
pub(crate) fn run_command_with_timeout(args: &[&str], timeout: Duration, cwd: Option<&Path>) -> CmdOutcome {
    if args.is_empty() {
        return CmdOutcome::Error("empty argument list".into());
    }

    let mut cmd = Command::new(args[0]);
    cmd.args(&args[1..])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("CI", "true")
        .env("DEBIAN_FRONTEND", "noninteractive");

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return CmdOutcome::Error(e.to_string()),
    };

    // Wait with timeout via a polling loop.
    wait_with_timeout(child, timeout)
}

/// Poll a child process until it exits or `timeout` elapses.
pub(crate) fn wait_with_timeout(mut child: std::process::Child, timeout: Duration) -> CmdOutcome {
    let start = Instant::now();
    let poll_interval = Duration::from_millis(50);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = child
                    .stdout
                    .take()
                    .map(|mut r| {
                        let mut s = String::new();
                        std::io::Read::read_to_string(&mut r, &mut s).ok();
                        s
                    })
                    .unwrap_or_default();
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut r| {
                        let mut s = String::new();
                        std::io::Read::read_to_string(&mut r, &mut s).ok();
                        s
                    })
                    .unwrap_or_default();

                return if status.success() {
                    CmdOutcome::Success(stdout)
                } else {
                    CmdOutcome::Failed(stderr)
                };
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return CmdOutcome::Timeout;
                }
                std::thread::sleep(poll_interval);
            }
            Err(e) => return CmdOutcome::Error(e.to_string()),
        }
    }
}

/// Return `true` if `name` resolves via `which`.
pub(crate) fn command_exists_on_path(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
