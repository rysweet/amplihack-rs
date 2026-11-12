//! Isolated subprocesses shared by the issue 2240 fixtures.
use std::path::Path;
use std::process::Command;

use super::fixture::{Fixture, root, success};

impl Fixture {
    pub fn command(&self, bin: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = crate::isolated_amplihack::isolated(bin);
        let path = std::env::join_paths(std::iter::once(self.tmp.path().join("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        c.env("HOME", &self.home)
            .env("TMPDIR", self.tmp.path())
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("GH_CONFIG_DIR", self.home.join("gh"))
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.home.join("profiles"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join(".gitconfig"))
            .env("AMPLIHACK_HOME", root())
            .env("XDG_RUNTIME_DIR", self.home.join("runtime"))
            .env("AMPLIHACK_WORK_ROOT", self.tmp.path().join("worktrees"))
            .env("AMPLIHACK_WORK_ROOT_EXPLICIT", "1")
            .env("PATH", path);
        c
    }

    pub fn git(&self, dir: &Path, args: &[&str]) -> String {
        // Hooks inherit the same isolated profile roots as recipe commands.
        let mut c = self.command("git");
        c.current_dir(dir);
        success(&c.args(args).output().unwrap())
    }
}
