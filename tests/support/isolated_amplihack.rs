//! Issue #1606 (review of #1647): the one way a test starts the `amplihack`
//! binary, so that no test can read or migrate a developer's real
//! `~/.amplihack` user config.
//!
//! Shared by every test harness through `#[path]`:
//! `crates/amplihack-cli/tests/main.rs`, `bins/amplihack/tests/main.rs` and
//! `tests/integration/main.rs`. `tests/integration/isolated_amplihack_scan.rs`
//! fails when a test builds a `Command` from the binary's path without
//! [`isolated`].
//!
//! The child gets:
//! - a cleared environment with only [`KEPT_VARS`] (and `CARGO*`, `RUST*`,
//!   `LC_*`) carried over, so nothing from the developer's shell or agent
//!   session leaks in;
//! - `HOME` set to a temp directory shared by this test process (a test that
//!   needs its own sets `HOME` again afterwards);
//! - `AMPLIHACK_TEST_HOME_GUARD=1`, on this child only: with it the config
//!   loader ignores any `HOME` outside the temp dir, and the binary never
//!   autostarts the Claude login rotator;
//! - `AMPLIHACK_ROTATE_AUTOSTART=0` (issue #1690), so a launch never starts a
//!   detached `profile rotate watch --spawned` daemon that outlives the test.
//!   The guard above is the stop that still holds when a test clears the
//!   environment and sets only the guard again.
//! - no `AMPLIHACK_CLAUDE_PROFILE_ROOT` (issue #1699), so an install under the
//!   temp `HOME` cannot refresh the developer's real profiles. `env_clear()`
//!   already drops it; the explicit removal holds if the kept list changes.
//! - `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=true` and `SSH_ASKPASS=true`
//!   ([`NO_GIT_PROMPT_ENV`], issue #1764) and a null stdin, through
//!   [`no_git_prompt`], so no git the child starts can wait on a credential
//!   prompt. `.cargo/config.toml` puts the same variables into the test
//!   process, but `env_clear()` drops them, so they are set again here. A
//!   test that calls `.env()` or `.stdin()` afterwards wins, and a test that
//!   clears another command's environment calls [`no_git_prompt`] itself
//!   after `env_clear()`.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Variables carried over from the test process. The temp dir must survive,
/// or the child's temp dir would differ from the test's and the guard would
/// reject a HOME the test created under it.
const KEPT_VARS: &[&str] = &[
    "PATH", "TMPDIR", "LANG", "TERM", "USER", "LOGNAME", "SHELL", "TZ",
];

/// The variable the user-config loader checks.
pub const HOME_GUARD_ENV: &str = "AMPLIHACK_TEST_HOME_GUARD";

/// The variable that turns the rotator's launch autostart off.
pub const ROTATE_AUTOSTART_ENV: &str = "AMPLIHACK_ROTATE_AUTOSTART";

/// The variable naming the Claude login profile root.
pub const PROFILE_ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";

/// The variables that stop git from asking for a credential (issue #1764),
/// with their values: the same three that `.cargo/config.toml` sets.
pub const NO_GIT_PROMPT_ENV: [(&str, &str); 3] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_ASKPASS", "true"),
    ("SSH_ASKPASS", "true"),
];

/// Set [`NO_GIT_PROMPT_ENV`] and a null stdin on `command`, and return it.
///
/// Call it after `env_clear()`, which drops both the values cargo's `[env]`
/// table gave the test process and any `GIT_TERMINAL_PROMPT=0` an
/// `amplihack_git` builder set. A later `.env()` or `.stdin()` call wins. A
/// test that hands the command to rexpect's `spawn_command` sets
/// `.stdin(Stdio::inherit())` afterwards: rexpect makes its pseudo-terminal
/// stdin before `exec`, and the null stdin would replace it.
pub fn no_git_prompt(command: &mut Command) -> &mut Command {
    for (name, value) in NO_GIT_PROMPT_ENV {
        command.env(name, value);
    }
    command.stdin(Stdio::null())
}

/// A temp `HOME` for tests that do not make their own: one per test process,
/// created by `tempfile` (a fresh random name, mode 0700, owned by this user,
/// so never a directory someone else made or an earlier run left), and
/// removed when the process exits.
pub fn shared_temp_home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempfile::Builder::new()
            .prefix("amplihack-test-home-")
            .tempdir()
            .expect("create the per-process test HOME")
            .keep();
        // A static is never dropped, so a TempDir kept here would outlive the
        // process; remove the directory at exit instead.
        extern "C" fn remove_at_exit() {
            if let Some(home) = HOME.get() {
                let _ = std::fs::remove_dir_all(home);
            }
        }
        // SAFETY: registering a plain `extern "C"` function has no
        // preconditions; it runs once, at normal process exit.
        unsafe { libc::atexit(remove_at_exit) };
        dir
    })
}

/// A `Command` for the `amplihack` binary at `bin`, isolated as described in
/// the module docs.
pub fn isolated(bin: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(bin);
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        let Some(name) = key.to_str() else { continue };
        let kept = KEPT_VARS.contains(&name)
            || name.starts_with("CARGO")
            || name.starts_with("RUST")
            || name.starts_with("LC_");
        if kept {
            command.env(&key, value);
        }
    }
    command.env_remove(PROFILE_ROOT_ENV);
    no_git_prompt(&mut command);
    // The toolchain lives under the real HOME unless these say otherwise; a
    // child that runs cargo must still find it after HOME moves.
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for (var, dir) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
            if std::env::var_os(var).is_none() && home.join(dir).is_dir() {
                command.env(var, home.join(dir));
            }
        }
    }
    command
        .env("HOME", shared_temp_home())
        .env(HOME_GUARD_ENV, "1")
        .env(ROTATE_AUTOSTART_ENV, "0");
    command
}
