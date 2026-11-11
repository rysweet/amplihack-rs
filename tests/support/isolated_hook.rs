//! Isolated environment for tests that start `amplihack-hooks` (#2361).
//!
//! `HookSandbox::new()` owns a temporary directory holding `home/`,
//! `amplihack-home/`, `profiles/` and `project/`. `command(bin)` returns a
//! `Command` that sets `HOME`, `AMPLIHACK_HOME`,
//! `AMPLIHACK_CLAUDE_PROFILE_ROOT` and `CLAUDE_PROJECT_DIR` to those
//! directories, runs in `project/`, and removes `REPO_PATH`,
//! `CLAUDE_CONFIG_DIR`, `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE`.
//! The rest of the environment is inherited (no `env_clear`). Callers may
//! override any of these values on the returned `Command`.
//!
//! Without this, a hook child inherits the `AMPLIHACK_HOME` and working
//! directory of the test process, and its `test-session` records reach
//! whatever checkout the test run started in.
//!
//! This is separate from `tests/support/isolated_amplihack.rs` (#1606),
//! which starts the `amplihack` binary with `env_clear()` and a kept-variable
//! list. Hook tests run with the rest of the test process environment and
//! set hook-specific variables of their own, which a cleared environment
//! with a fixed kept list would drop. This helper therefore removes only the
//! variables that steer state into a checkout or a Git repository, and sets
//! the four directories above.
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

/// Variables removed from every hook child: each can point a hook, or a git
/// call made by a hook, at a real checkout or a real profile.
pub const REMOVED_VARS: &[&str] = &[
    "REPO_PATH",
    "CLAUDE_CONFIG_DIR",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
];

/// A temporary tree that owns every directory a hook child may write to.
pub struct HookSandbox {
    root: tempfile::TempDir,
}

impl HookSandbox {
    pub fn new() -> Self {
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
        command
            .env("HOME", self.home())
            .env("AMPLIHACK_HOME", self.amplihack_home())
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.profiles())
            .env("CLAUDE_PROJECT_DIR", project)
            .current_dir(project);
        for var in REMOVED_VARS {
            command.env_remove(var);
        }
        command
    }
}

impl Default for HookSandbox {
    fn default() -> Self {
        Self::new()
    }
}
