//! Whether a folder holds git work that is not on a remote (issue #2785).
//!
//! Every cleanup path that removes a folder able to hold a git worktree or
//! clone asks [`work_inside`] first: the runner's own removal of a runtime
//! folder, `run-tmp` and `tmp-leftover` at plan and apply time. Reclaim of
//! stable execution storage and release of a run's private repository ask
//! [`tree_work`] about one worktree. The rules are in
//! `docs/howto/clean-up-after-recipe-runs.md`, "Uncommitted and unpushed git
//! work".
//!
//! The checks themselves are the ones that already keep a `run-worktree`:
//! [`git_has_dirty_state`] and [`unpushed_reason`], unchanged. This module
//! only decides which trees they are asked about, and runs no git in a tree
//! another user owns or others can write.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::commands::hygiene::cleanup::{
    git_has_dirty_state, hardened_git, sanitized_stderr_short,
};
use anyhow::{Result, bail};
use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Size, age and git-tree walks stop after this many entries.
pub(crate) const WALK_LIMIT: usize = 1_000_000;
pub(crate) const GIT_TIMEOUT: Duration = Duration::from_secs(60);
/// The end-of-run sweep's time budget, and the time the runner gives the
/// search of its own runtime folder.
pub(crate) const END_OF_RUN_DEADLINE: Duration = Duration::from_secs(120);
/// At most this many trees in one folder are asked about work.
pub(crate) const TREE_LIMIT: usize = 64;

/// Why a tree is kept without running git in it.
const NOT_INSPECTED: &str = "is writable by other users and was not inspected";
const DEADLINE_REASON: &str = "search for git trees did not finish before the end-of-run deadline";
/// A `.git` file is one line; anything longer is not a gitdir file.
const GITFILE_MAX_BYTES: u64 = 64 * 1024;
/// How often the walk looks at the deadline, in entries.
const DEADLINE_STRIDE: usize = 4096;

/// The limits and identity one search runs with.
struct Policy {
    deadline: Option<Instant>,
    walk_limit: usize,
    tree_limit: usize,
    uid: u32,
    gid: u32,
}

impl Policy {
    fn past_deadline(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn egid() -> u32 {
    // SAFETY: getegid has no preconditions and cannot fail.
    unsafe { libc::getegid() }
}

// ----- the existing checks, moved from sweep.rs unchanged ----------------------

/// The canonical git common dir of the repository `dir` belongs to.
pub(crate) fn git_common_dir(dir: &Path) -> Option<PathBuf> {
    let mut command = hardened_git();
    command
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--git-common-dir"]);
    let output = crate::util::run_output_with_timeout(command, GIT_TIMEOUT).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let raw = PathBuf::from(text.trim());
    let abs = if raw.is_absolute() {
        raw
    } else {
        dir.join(raw)
    };
    fs::canonicalize(abs).ok()
}

/// Why `worktree` must be kept for commits that may exist only locally, or
/// `None` when every commit is on a remote. With an upstream, commits ahead of
/// it count. When the upstream is gone (the branch was deleted after merge and
/// pruned), commits reachable from any remote-tracking ref count as pushed, so
/// a merged worktree is not kept for ever under a false "unpushed" reason.
pub(crate) fn unpushed_reason(worktree: &Path) -> Option<String> {
    let count = |range: &[&str]| -> Result<u64> {
        let mut command = hardened_git();
        command
            .arg("-C")
            .arg(worktree)
            .args(["rev-list", "--count"])
            .args(range);
        let output = crate::util::run_output_with_timeout(command, GIT_TIMEOUT)?;
        if !output.status.success() {
            bail!("{}", sanitized_stderr_short(&output.stderr));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().parse()?)
    };
    let mut upstream = hardened_git();
    upstream
        .arg("-C")
        .arg(worktree)
        .args(["rev-parse", "--verify", "--quiet", "@{upstream}"]);
    let has_upstream = crate::util::run_output_with_timeout(upstream, GIT_TIMEOUT)
        .is_ok_and(|output| output.status.success());
    let (result, label) = if has_upstream {
        (count(&["@{upstream}..HEAD"]), "has unpushed commits")
    } else {
        (
            count(&["HEAD", "--not", "--remotes"]),
            "has no upstream and commits on no remote branch",
        )
    };
    match result {
        Ok(0) => None,
        Ok(_) => Some(label.to_string()),
        Err(error) => Some(format!("cannot check for unpushed commits: {error:#}")),
    }
}

// ----- reasons ---------------------------------------------------------------

/// `text` with every C0 and C1 control character and DEL escaped, so text a
/// step controls (git's stderr, in an unpushed reason) cannot move the cursor
/// or recolour the terminal when a reason is printed.
pub(crate) fn printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.extend(c.escape_debug());
        } else {
            out.push(c);
        }
    }
    out
}

// ----- one tree ----------------------------------------------------------------

/// What the owner and mode guard decided about a tree, before any git runs.
enum Guard {
    /// Another user's tree, or one whose `.git` entry is gone: git is not
    /// run in it and it does not keep the folder.
    Pass,
    /// Others can write what git reads first: kept, not inspected.
    Unsafe,
    /// Safe to ask git; with the git dir its `.git` entry names, when that
    /// could be worked out without git.
    Inspect(Option<PathBuf>),
}

/// Whether others can write `meta`: other-write always, group-write when the
/// group is not `gid`. A `.git/config` at `0664` in the own group, as umask
/// 002 leaves it, is safe.
fn writable_by_others(meta: &fs::Metadata, gid: u32) -> bool {
    let mode = meta.mode();
    mode & 0o002 != 0 || (mode & 0o020 != 0 && meta.gid() != gid)
}

/// `bytes` without trailing line ends, as git reads a `.git` file and as it
/// ends a path it prints.
fn trim_line_end(mut bytes: &[u8]) -> &[u8] {
    while let [rest @ .., b'\n' | b'\r'] = bytes {
        bytes = rest;
    }
    bytes
}

/// The folder a `.git` file names on its `gitdir:` line, as git reads it:
/// trailing line ends are dropped and a relative path is taken from the
/// tree's folder. `None` when the file cannot be read or has no such line.
fn gitdir_line(gitfile: &Path, tree: &Path) -> Option<PathBuf> {
    // Non-blocking, so a `.git` swapped for a FIFO cannot stall cleanup.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(gitfile)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(GITFILE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > GITFILE_MAX_BYTES {
        return None;
    }
    let named = trim_line_end(&bytes).strip_prefix(b"gitdir: ")?;
    if named.is_empty() {
        return None;
    }
    let named = Path::new(OsStr::from_bytes(named));
    Some(if named.is_absolute() {
        named.to_path_buf()
    } else {
        tree.join(named)
    })
}

/// The owner and mode rules for `tree` (rules 3 and 4 of "Which git trees
/// are checked"), read with no git. The tree's folder and its `.git` entry
/// must belong to `uid`. Then what git reads first must not be writable by
/// others: a `.git` folder and its `config`; a `.git` file and the folder on
/// its `gitdir:` line; for a link, what it points to.
fn guard(tree: &Path, uid: u32, gid: u32) -> Guard {
    let git = tree.join(".git");
    let (Ok(tree_meta), Ok(git_meta)) = (fs::symlink_metadata(tree), fs::symlink_metadata(&git))
    else {
        return Guard::Pass;
    };
    if tree_meta.uid() != uid || git_meta.uid() != uid {
        return Guard::Pass;
    }
    let target = if git_meta.file_type().is_symlink() {
        match fs::metadata(&git) {
            Ok(meta) => meta,
            // A dangling link: git cannot use it either (rule 5).
            Err(_) => return Guard::Inspect(None),
        }
    } else {
        git_meta
    };
    if writable_by_others(&target, gid) {
        return Guard::Unsafe;
    }
    if target.is_dir() {
        // A missing `config` is no reason to keep: a worktree's admin folder
        // used as `.git` has none.
        if fs::metadata(git.join("config")).is_ok_and(|m| writable_by_others(&m, gid)) {
            return Guard::Unsafe;
        }
        return Guard::Inspect(fs::canonicalize(&git).ok());
    }
    if !target.is_file() {
        return Guard::Inspect(None);
    }
    let Some(named) = gitdir_line(&git, tree) else {
        return Guard::Inspect(None);
    };
    match fs::metadata(&named) {
        Ok(meta) if writable_by_others(&meta, gid) => Guard::Unsafe,
        Ok(_) => Guard::Inspect(fs::canonicalize(&named).ok()),
        Err(_) => Guard::Inspect(None),
    }
}

/// The existing checks on one tree, with the reason made printable.
fn checks(tree: &Path) -> Option<String> {
    if git_has_dirty_state(tree) {
        return Some("has uncommitted changes".to_string());
    }
    unpushed_reason(tree).map(|reason| printable(&reason))
}

/// Why the tree at `tree` holds work, with the owner and mode rules of
/// [`work_inside`] for this process's effective uid and gid: `None` for a
/// tree with no work and for another user's tree, which is not inspected.
/// The reason is bare (`has uncommitted changes`, ...); the caller names the
/// tree.
pub(crate) fn tree_work(tree: &Path) -> Option<String> {
    match guard(tree, euid(), egid()) {
        Guard::Pass => None,
        Guard::Unsafe => Some(NOT_INSPECTED.to_string()),
        Guard::Inspect(_) => checks(tree),
    }
}

// ----- one folder ----------------------------------------------------------------

/// Why `folder` must be kept because a git tree inside it holds uncommitted
/// changes or commits that are not on a remote, or because the search could
/// not finish; `None` when nothing in it needs keeping. A tree whose
/// repository and remotes all lie inside `folder` goes with it and is not
/// checked. `deadline`, when given, bounds the search.
pub(crate) fn work_inside(folder: &Path, deadline: Option<Instant>) -> Option<String> {
    work_inside_with(
        folder,
        &Policy {
            deadline,
            walk_limit: WALK_LIMIT,
            tree_limit: TREE_LIMIT,
            uid: euid(),
            gid: egid(),
        },
    )
}

fn work_inside_with(folder: &Path, policy: &Policy) -> Option<String> {
    let root = match fs::canonicalize(folder) {
        Ok(root) => root,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => {
            return Some(format!(
                "cannot search for git trees: cannot resolve {folder:?}: {error}"
            ));
        }
    };
    if policy.past_deadline() {
        return Some(DEADLINE_REASON.to_string());
    }
    let candidates = match candidates(&root, policy) {
        Ok(candidates) => candidates,
        Err(reason) => return Some(reason),
    };
    let mut selected = Vec::new();
    for tree in candidates {
        if policy.past_deadline() {
            return Some(DEADLINE_REASON.to_string());
        }
        match select(&tree, &root, policy) {
            Selection::Pass => {}
            Selection::Keep(reason) => return Some(format!("git tree {tree:?} {reason}")),
            Selection::Check => {
                selected.push(tree);
                if selected.len() > policy.tree_limit {
                    return Some(format!(
                        "more than {} git trees to check",
                        policy.tree_limit
                    ));
                }
            }
        }
    }
    for tree in selected {
        if policy.past_deadline() {
            return Some(DEADLINE_REASON.to_string());
        }
        if let Some(reason) = checks(&tree) {
            return Some(format!("git tree {tree:?} {reason}"));
        }
    }
    None
}

/// Whether a walk error passes over the entry: it cannot be read for lack of
/// permission, or it went away during the walk.
fn passable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::PermissionDenied | ErrorKind::NotFound
    )
}

/// Every folder under `root`, `root` included, that holds a `.git` entry of
/// any kind, sorted. Links are not followed and no `.git` folder is entered.
/// `Err` is the reason the folder is kept.
fn candidates(root: &Path, policy: &Policy) -> Result<Vec<PathBuf>, String> {
    let cannot = |path: &Path, error: std::io::Error| {
        format!("cannot search for git trees: cannot read {path:?}: {error}")
    };
    let mut out = Vec::new();
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if passable(&error) => continue,
            Err(error) => return Err(cannot(&dir, error)),
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if passable(&error) => continue,
                Err(error) => return Err(cannot(&dir, error)),
            };
            seen += 1;
            if seen > policy.walk_limit {
                return Err(format!(
                    "search for git trees stopped after {} entries",
                    policy.walk_limit
                ));
            }
            if seen.is_multiple_of(DEADLINE_STRIDE) && policy.past_deadline() {
                return Err(DEADLINE_REASON.to_string());
            }
            let path = entry.path();
            if entry.file_name() == ".git" {
                out.push(dir.clone());
                continue;
            }
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_dir() => stack.push(path),
                Ok(_) => {}
                Err(error) if passable(&error) => {}
                Err(error) => return Err(cannot(&path, error)),
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// What the search does with one candidate.
enum Selection {
    /// Not a tree, another user's, or self-contained: it goes with the folder.
    Pass,
    /// Kept without running the checks, for this reason.
    Keep(String),
    /// Ask the checks.
    Check,
}

/// Rules 3 to 6 for the candidate `tree` inside the canonical `root`.
fn select(tree: &Path, root: &Path, policy: &Policy) -> Selection {
    let named = match guard(tree, policy.uid, policy.gid) {
        Guard::Unsafe => return Selection::Keep(NOT_INSPECTED.to_string()),
        Guard::Inspect(Some(named)) => named,
        Guard::Pass | Guard::Inspect(None) => return Selection::Pass,
    };
    // git must agree that `tree` is the top of the repository its `.git`
    // names, so a stray `.git` inside another repository is not taken for
    // that repository.
    if absolute_git_dir(tree).as_deref() != Some(named.as_path()) {
        return Selection::Pass;
    }
    let Some(common) = git_common_dir(tree) else {
        return Selection::Pass;
    };
    if common.starts_with(root) && remotes_inside(tree, root) {
        Selection::Pass
    } else {
        Selection::Check
    }
}

/// `git rev-parse --absolute-git-dir` in `tree`, canonical.
fn absolute_git_dir(tree: &Path) -> Option<PathBuf> {
    let mut command = hardened_git();
    command
        .arg("-C")
        .arg(tree)
        .args(["rev-parse", "--absolute-git-dir"]);
    let output = crate::util::run_output_with_timeout(command, GIT_TIMEOUT).ok()?;
    if !output.status.success() {
        return None;
    }
    fs::canonicalize(OsStr::from_bytes(trim_line_end(&output.stdout))).ok()
}

/// Whether every remote URL of the repository at `tree` points inside
/// `root`; true when it has none. Any doubt counts as outside, so the tree is
/// checked. The URLs never leave this function: they can carry credentials.
fn remotes_inside(tree: &Path, root: &Path) -> bool {
    let mut command = hardened_git();
    command.arg("-C").arg(tree).args([
        "config",
        "-z",
        "--get-regexp",
        r"^remote\..*\.(url|pushurl)$",
    ]);
    let Ok(output) = crate::util::run_output_with_timeout(command, GIT_TIMEOUT) else {
        return false;
    };
    match output.status.code() {
        Some(0) => {}
        // No key matched: the repository has no remotes.
        Some(1) => return true,
        _ => return false,
    }
    output
        .stdout
        .split(|b| *b == 0)
        .filter(|record| !record.is_empty())
        .all(|record| {
            let mut parts = record.splitn(2, |b| *b == b'\n');
            let _key = parts.next();
            parts.next().is_some_and(|url| url_inside(url, root))
        })
}

/// Whether one remote URL points inside `root`: an absolute path, or a
/// `file://` URL with no host, that resolves inside it, or, when it does not
/// exist, that lies inside it lexically with no `.` or `..` component and no
/// `%`. Every other form points outside.
fn url_inside(url: &[u8], root: &Path) -> bool {
    let path = match url.strip_prefix(b"file://") {
        Some(rest) if rest.starts_with(b"/") && !rest.contains(&b'%') => rest,
        Some(_) => return false,
        None if url.starts_with(b"/") => url,
        None => return false,
    };
    let path = Path::new(OsStr::from_bytes(path));
    match fs::canonicalize(path) {
        Ok(resolved) => resolved.starts_with(root),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let bytes = path.as_os_str().as_bytes();
            !bytes.contains(&b'%')
                && !bytes
                    .split(|b| *b == b'/')
                    .any(|part| part == b"." || part == b"..")
                && path.starts_with(root)
        }
        Err(_) => false,
    }
}

#[cfg(test)]
#[path = "tests_git_work.rs"]
mod tests;
