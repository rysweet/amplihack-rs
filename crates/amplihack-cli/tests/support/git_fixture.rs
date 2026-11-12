//! A real Git repository for fixtures that need a project root.
//!
//! A recipe run's checkout discovery fails closed on a malformed `.git`
//! (issue #2096), so a hand-written `.git/HEAD` marker no longer counts as a
//! repository. This makes a real one, with one empty commit so `HEAD` is born,
//! without reading the caller's global or system Git configuration.
use std::path::Path;

/// `git init` in `dir` (created if missing) and one empty commit on `main`.
/// Panics with Git's output on any failure.
pub fn init_with_commit(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    for args in [
        &["init", "-q", "-b", "main"][..],
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fixture root",
        ][..],
    ] {
        let output = amplihack_git::command_in(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", dir.display()));
        assert!(
            output.status.success(),
            "git {args:?} in {}: {output:?}",
            dir.display()
        );
    }
}
