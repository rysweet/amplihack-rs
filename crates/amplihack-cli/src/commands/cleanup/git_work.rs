//! Repository preservation for every cleanup deletion boundary (issue #2785).
//!
//! Discovery includes the candidate and independently nested repositories,
//! without entering `.git` object storage or following directory links. The
//! existing Git checks determine disposability; incomplete discovery keeps work.

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
use std::fs;
use std::io::ErrorKind;
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

const DEADLINE_REASON: &str = "search for git trees did not finish before the end-of-run deadline";
/// How often the walk looks at the deadline, in entries.
const DEADLINE_STRIDE: usize = 4096;

/// The existing traversal limits for one search.
struct Policy {
    deadline: Option<Instant>,
    walk_limit: usize,
    tree_limit: usize,
}

impl Policy {
    fn past_deadline(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }
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

/// Why one repository must stay, with the reason made printable. Dirty-state
/// inspection also preserves on Git errors, using the existing conservative helper.
pub(crate) fn tree_work(tree: &Path) -> Option<String> {
    if git_has_dirty_state(tree) {
        return Some("has uncommitted changes".to_string());
    }
    unpushed_reason(tree).map(|reason| printable(&reason))
}

/// Callback for callers that remove an entire storage boundary.
pub(crate) fn folder_work(folder: &Path) -> Option<String> {
    work_inside(folder, None)
}

// ----- one folder ----------------------------------------------------------------

/// Why `folder` must be kept because a git tree inside it holds uncommitted
/// changes or commits that are not on a remote, or because the search could
/// not finish; `None` when nothing in it needs keeping. `deadline`, when
/// given, bounds the search. Remote location never exempts a repository.
pub(crate) fn work_inside(folder: &Path, deadline: Option<Instant>) -> Option<String> {
    work_inside_with(
        folder,
        &Policy {
            deadline,
            walk_limit: WALK_LIMIT,
            tree_limit: TREE_LIMIT,
        },
    )
    .map(|reason| printable(&reason))
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
    if candidates.len() > policy.tree_limit {
        return Some(format!(
            "more than {} git trees to check",
            policy.tree_limit
        ));
    }
    for (tree, bare) in candidates {
        if policy.past_deadline() {
            return Some(DEADLINE_REASON.to_string());
        }
        // Bare repositories have no working tree to dirty. Their commits
        // still go through the existing unpushed check, including its errors.
        let reason = if bare {
            unpushed_reason(&tree)
        } else {
            tree_work(&tree)
        };
        if let Some(reason) = reason {
            return Some(format!("git tree {tree:?} {reason}"));
        }
        let Some(common) = git_common_dir(&tree) else {
            return Some(format!(
                "cannot inspect the backing repository of git tree {tree:?}"
            ));
        };
        if common.starts_with(&root) {
            // Removing metadata also affects registered worktrees outside the
            // directory. Reuse execution storage's existing dependency rule.
            match crate::commands::recipe::run::run_checkout::exec_storage::repository_dependant(
                &root, &common, tree_work,
            ) {
                Ok(Some(reason)) => return Some(reason),
                Ok(None) => {}
                Err(error) => {
                    return Some(format!("cannot inspect repository {common:?}: {error:#}"));
                }
            }
        }
    }
    None
}

/// Every folder under `root`, `root` included, that holds a `.git` entry or
/// bare repository layout, sorted. Links are not followed and repository
/// metadata is not entered.
/// `Err` is the reason the folder is kept.
fn candidates(root: &Path, policy: &Policy) -> Result<Vec<(PathBuf, bool)>, String> {
    let cannot = |path: &Path, error: std::io::Error| {
        format!("cannot search for git trees: cannot read {path:?}: {error}")
    };
    let mut out = Vec::new();
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => return Err(cannot(&dir, error)),
        };
        let mut children = Vec::new();
        let (mut head, mut objects, mut refs) = (false, false, false);
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
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
            let name = entry.file_name();
            if name == ".git" {
                out.push((dir.clone(), false));
                continue;
            }
            // The type the folder listing already gives, so no entry costs a
            // stat of its own; like `symlink_metadata`, it does not follow a
            // link.
            match entry.file_type() {
                Ok(kind) => {
                    head |= name == "HEAD" && kind.is_file();
                    objects |= name == "objects" && kind.is_dir();
                    refs |= name == "refs" && kind.is_dir();
                    if kind.is_dir() {
                        children.push(entry.path());
                    }
                }
                Err(error) => return Err(cannot(&entry.path(), error)),
            }
        }
        if head && objects && refs {
            // A bare clone has no `.git` entry. Pass it to the same Git
            // checks without walking its object database.
            out.push((dir, true));
            children.retain(|path| {
                !path
                    .file_name()
                    .is_some_and(|name| name == "objects" || name == "refs")
            });
        }
        stack.extend(children);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
#[path = "tests_git_work.rs"]
mod tests;
