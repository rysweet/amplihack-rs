//! Git fixtures for a folder that holds unsaved work (issue #2785).
//!
//! A bare origin with one pushed commit, and clones of it. Every clone is on
//! `main` with `origin/main` as its upstream, so "pushed" is a fact the
//! tests set up, not an assumption: a fresh clone is clean and pushed;
//! [`make_dirty`] leaves a file that was never committed; [`make_unpushed`]
//! leaves a commit that is on no remote.
//!
//! No command reads the caller's global or system Git configuration, and each
//! has a fixed identity and no commit signing. Nothing touches the network:
//! the origin is a folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn command(dir: &Path) -> Command {
    let mut command = amplihack_git::command_in(dir);
    command
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    command
}

/// Runs `git <args>` in `dir`; panics with Git's own words on failure.
/// Returns trimmed stdout.
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    git_paths(dir, args, &[])
}

/// [`git`] with `paths` appended as arguments, so a path that is not UTF-8
/// still reaches Git byte for byte.
pub(crate) fn git_paths(dir: &Path, args: &[&str], paths: &[&Path]) -> String {
    let output = command(dir)
        .args(args)
        .args(paths)
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", dir.display()));
    assert!(
        output.status.success(),
        "git {args:?} {paths:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A bare repository on `main` that holds one commit.
pub(crate) struct Origin {
    pub(crate) path: PathBuf,
}

impl Origin {
    /// Makes the bare repository at `at` (its parent is created).
    pub(crate) fn create(at: &Path) -> Self {
        let parent = at.parent().expect("an origin path has a parent");
        fs::create_dir_all(parent).unwrap();
        git_paths(parent, &["init", "-q", "--bare", "-b", "main"], &[at]);
        let seed = at.with_extension("seed");
        git_paths(parent, &["clone", "-q"], &[at, &seed]);
        fs::write(seed.join("file.txt"), "base\n").unwrap();
        git(&seed, &["add", "file.txt"]);
        git(&seed, &["commit", "-q", "-m", "base"]);
        git(&seed, &["push", "-q", "-u", "origin", "main"]);
        fs::remove_dir_all(&seed).unwrap();
        Self {
            path: at.to_path_buf(),
        }
    }

    /// Clones the origin to `dest` (its parents are created): a work tree
    /// with nothing unsaved in it.
    pub(crate) fn clone_to(&self, dest: &Path) {
        let parent = dest.parent().expect("a clone path has a parent");
        fs::create_dir_all(parent).unwrap();
        git_paths(parent, &["clone", "-q"], &[&self.path, dest]);
    }
}

/// A temporary folder with an origin in it, outside any folder a test
/// judges, so the origin itself is never a candidate.
pub(crate) struct Scratch {
    _tmp: tempfile::TempDir,
    /// The canonical path of the temporary folder.
    pub(crate) root: PathBuf,
    pub(crate) origin: Origin,
}

impl Scratch {
    pub(crate) fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let origin = Origin::create(&root.join("fixture-origin/origin.git"));
        Self {
            _tmp: tmp,
            root,
            origin,
        }
    }
}

/// A file that was never committed.
pub(crate) fn make_dirty(clone: &Path) {
    fs::write(clone.join("unsaved.txt"), "not committed\n").unwrap();
}

/// A commit that is on no remote.
pub(crate) fn make_unpushed(clone: &Path) {
    git(
        clone,
        &["commit", "-q", "--allow-empty", "-m", "local only"],
    );
}
