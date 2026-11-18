//! Git trees for the issue #2785 cleanup tests: a bare origin, a clone of it,
//! linked worktrees of that clone, uncommitted files and unpushed commits.
//!
//! Included with `#[path]` by `tests_git_work.rs`, `tests_sweep.rs` and
//! `tests_run_dir.rs`, so each test module gets its own copy.
//!
//! Every git command here runs with no global or system configuration and a
//! fixed identity, and with this host's `PATH` (see
//! [`crate::test_support::host_path`]). Nothing touches the real home.

use std::fs;
use std::path::{Path, PathBuf};

/// Runs `git <args>` in `dir`, asserts success and returns stdout, trimmed.
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(dir)
        .env("PATH", crate::test_support::host_path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn git {args:?} in {}: {e}", dir.display()));
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bare repository at `<base>/origin.git` and a clone of it at
/// `<base>/clone`, with one commit on `main` that is pushed and tracked.
pub(crate) fn origin_and_clone(base: &Path) -> (PathBuf, PathBuf) {
    fs::create_dir_all(base).unwrap();
    let origin = base.join("origin.git");
    let clone = base.join("clone");
    git(
        base,
        &["init", "-q", "--bare", "-b", "main", path_str(&origin)],
    );
    git(base, &["init", "-q", "-b", "main", path_str(&clone)]);
    fs::write(clone.join("base.txt"), "base\n").unwrap();
    git(&clone, &["add", "base.txt"]);
    git(&clone, &["commit", "-q", "-m", "base"]);
    git(&clone, &["remote", "add", "origin", path_str(&origin)]);
    git(&clone, &["push", "-q", "-u", "origin", "main"]);
    (origin, clone)
}

/// A clone of `origin` at `dest`, on `main` and tracking `origin/main`.
pub(crate) fn clone_of(origin: &Path, dest: &Path) -> PathBuf {
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    git(
        dest.parent().unwrap(),
        &["clone", "-q", path_str(origin), path_str(dest)],
    );
    dest.to_path_buf()
}

/// A linked worktree of `clone` at `at`, on a new branch `branch` made from
/// `main`. Its `.git` is a file naming `<clone>/.git/worktrees/<name>`.
pub(crate) fn linked_worktree(clone: &Path, at: &Path, branch: &str) -> PathBuf {
    fs::create_dir_all(at.parent().unwrap()).unwrap();
    git(
        clone,
        &["worktree", "add", "-q", "-b", branch, path_str(at)],
    );
    at.to_path_buf()
}

/// An untracked file in `tree`: `git status --porcelain` lists it.
pub(crate) fn make_dirty(tree: &Path) -> PathBuf {
    let file = tree.join("uncommitted.txt");
    fs::write(&file, "work that is on no remote\n").unwrap();
    file
}

/// Commits a new file named `name` in `tree` and returns the commit's SHA.
pub(crate) fn commit(tree: &Path, name: &str) -> String {
    fs::write(tree.join(name), format!("{name}\n")).unwrap();
    git(tree, &["add", "--", name]);
    git(tree, &["commit", "-q", "-m", name]);
    git(tree, &["rev-parse", "HEAD"])
}

/// Pushes the branch checked out in `tree` to `origin` and sets it as the
/// branch's upstream, so nothing in `tree` is unpushed afterwards.
pub(crate) fn push(tree: &Path) {
    git(tree, &["push", "-q", "-u", "origin", "HEAD"]);
}

/// A linked worktree at `at` that is clean and whose commit is on its
/// upstream: the checks find no work in it.
pub(crate) fn clean_pushed_worktree(clone: &Path, at: &Path, branch: &str) -> PathBuf {
    let tree = linked_worktree(clone, at, branch);
    commit(&tree, &format!("{branch}.txt"));
    push(&tree);
    tree
}

/// Whether the object `sha` is in the repository that `dir` belongs to.
pub(crate) fn has_object(dir: &Path, sha: &str) -> bool {
    amplihack_git::command_in(dir)
        .env("PATH", crate::test_support::host_path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["cat-file", "-e", sha])
        .status()
        .is_ok_and(|s| s.success())
}

/// The `.git` directory of a linked worktree: what its `.git` file names.
pub(crate) fn admin_dir_of(tree: &Path) -> PathBuf {
    PathBuf::from(git(tree, &["rev-parse", "--absolute-git-dir"]))
}

/// This process's effective uid.
pub(crate) fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// This process's effective gid.
pub(crate) fn egid() -> u32 {
    // SAFETY: getegid has no preconditions and cannot fail.
    unsafe { libc::getegid() }
}

/// True when the test runs as root, for which file modes do not stop
/// access; tests that depend on modes return early then.
pub(crate) fn running_as_root() -> bool {
    euid() == 0
}

/// Sets the mode of `path`, a real file or folder.
pub(crate) fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .unwrap_or_else(|e| panic!("chmod {mode:o} {}: {e}", path.display()));
}

/// Gives `path` mode 0700 again when dropped, so a test that locks a folder
/// still lets its temporary directory be removed, even when it fails.
pub(crate) struct Unlock(pub(crate) PathBuf);

impl Drop for Unlock {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
    }
}

fn path_str(path: &Path) -> &str {
    path.to_str()
        .unwrap_or_else(|| panic!("{} is not UTF-8", path.display()))
}
