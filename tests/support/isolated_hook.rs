//! Isolated environment for tests that start `amplihack-hooks` (#2361).
//!
//! `HookSandbox::new()` owns a temporary directory holding `home/`,
//! `amplihack-home/`, `profiles/` and `project/`. `command(bin)` returns a
//! `Command` whose environment is cleared (`env_clear`) and then rebuilt from
//! three parts only:
//!
//! - the variables of the test process accepted by [`is_kept`]: the names in
//!   [`KEPT_VARS`] and the names starting with one of [`KEPT_PREFIXES`];
//! - [`NO_GIT_PROMPT_ENV`], which `.cargo/config.toml` gives the test process
//!   and `env_clear` drops;
//! - `HOME`, `AMPLIHACK_HOME`, `AMPLIHACK_CLAUDE_PROFILE_ROOT` and
//!   `CLAUDE_PROJECT_DIR`, set to the sandbox directories.
//!
//! The child runs in `project/`. Callers add the hook-specific variables a
//! test needs, and may override any of the values above, on the returned
//! `Command`.
//!
//! Nothing else reaches the child. Without this, a hook child inherits the
//! `AMPLIHACK_HOME` and working directory of the test process, and its
//! `test-session` records reach whatever checkout the test run started in.
//! An earlier version removed a fixed list of variables and inherited the
//! rest; any variable missing from that list (`AMPLIHACK_SIGNAL_STATE_DIR`,
//! `AMPLIHACK_GRAPH_DB_PATH`, `AMPLIHACK_WORK_ROOT`, ...) still reached the
//! child. `bins/amplihack-hooks/tests/issue_2361_hook_sandbox_environment.rs`
//! fails if any variable outside the three parts above reaches a child.
//!
//! The kept list is the one `tests/support/isolated_amplihack.rs` (#1606)
//! uses for the `amplihack` binary. That helper also sets the variables that
//! only the `amplihack` binary reads (the home guard, the fake process table,
//! the rotator autostart switch), so hook tests do not use it directly.
//!
//! The in-process golden tests (`tests/golden/run_golden.rs`) apply the same
//! kept list to the test process itself, under their environment lock.
//!
//! The `#[path]` value is relative to the directory of the including file:
//!
//! - from `tests/integration/main.rs` or `tests/golden/run_golden.rs`:
//!   `#[path = "../support/isolated_hook.rs"] mod isolated_hook;`
//! - from a file in `bins/amplihack-hooks/tests/`:
//!   `#[path = "../../../tests/support/isolated_hook.rs"] mod isolated_hook;`
#![allow(dead_code)]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Variables a hook child keeps from the test process, by exact name. The
/// same names as `KEPT_VARS` in `tests/support/isolated_amplihack.rs`.
pub const KEPT_VARS: &[&str] = &[
    "PATH", "TMPDIR", "LANG", "TERM", "USER", "LOGNAME", "SHELL", "TZ",
];

/// Variables a hook child keeps from the test process, by prefix: the
/// toolchain (`CARGO*`, `RUST*`) and the locale (`LC_*`).
pub const KEPT_PREFIXES: &[&str] = &["CARGO", "RUST", "LC_"];

/// The variables that stop git from asking for a credential (#1764), with
/// their values: the three that `.cargo/config.toml` sets for the test
/// process.
pub const NO_GIT_PROMPT_ENV: [(&str, &str); 3] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_ASKPASS", "true"),
    ("SSH_ASKPASS", "true"),
];

/// Whether a hook child keeps the test process's value of `name`.
pub fn is_kept(name: &str) -> bool {
    KEPT_VARS.contains(&name) || KEPT_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// A temporary tree that owns every directory a hook child may write to.
pub struct HookSandbox {
    root: tempfile::TempDir,
}

impl HookSandbox {
    pub fn new() -> Self {
        // Recipe TMPDIR can be beneath a real framework tree. Context lookup
        // walks ancestors, so place hook fixtures outside those roots.
        #[cfg(unix)]
        let root = tempfile::tempdir_in(fs::canonicalize("/tmp").expect("scratch root"))
            .expect("hook sandbox temp dir");
        #[cfg(not(unix))]
        let root = tempfile::tempdir().expect("hook sandbox temp dir");
        for dir in ["home", "amplihack-home", "profiles", "project"] {
            fs::create_dir_all(root.path().join(dir)).expect("hook sandbox subdirectory");
        }
        Self { root }
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// `HOME` of the child.
    pub fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    /// `AMPLIHACK_HOME` of the child.
    pub fn amplihack_home(&self) -> PathBuf {
        self.root().join("amplihack-home")
    }

    /// `AMPLIHACK_CLAUDE_PROFILE_ROOT` of the child.
    pub fn profiles(&self) -> PathBuf {
        self.root().join("profiles")
    }

    /// `CLAUDE_PROJECT_DIR` and working directory of the child.
    pub fn project(&self) -> PathBuf {
        self.root().join("project")
    }

    /// A `Command` for `bin` with the isolated environment described in the
    /// module documentation.
    pub fn command(&self, bin: impl AsRef<OsStr>) -> Command {
        let project = self.project();
        self.command_in(bin, &project)
    }

    /// As [`HookSandbox::command`], with `project` as both the working
    /// directory and `CLAUDE_PROJECT_DIR`. For tests that already own a
    /// narrower project directory.
    pub fn command_in(&self, bin: impl AsRef<OsStr>, project: &Path) -> Command {
        let mut command = Command::new(bin);
        command.env_clear();
        for (key, value) in std::env::vars_os() {
            if key.to_str().is_some_and(is_kept) {
                command.env(&key, value);
            }
        }
        command
            .envs(NO_GIT_PROMPT_ENV)
            .env("HOME", self.home())
            .env("AMPLIHACK_HOME", self.amplihack_home())
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.profiles())
            .env("CLAUDE_PROJECT_DIR", project)
            .current_dir(project);
        command
    }
}

impl Default for HookSandbox {
    fn default() -> Self {
        Self::new()
    }
}
