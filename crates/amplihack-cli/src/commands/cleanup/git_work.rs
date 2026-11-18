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

use crate::commands::hygiene::cleanup::{git_dirty_state, hardened_git, sanitized_stderr_short};
use anyhow::{Context, Result, bail};
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
        past_deadline(self.deadline)
    }
}

fn past_deadline(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() >= d)
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
fn unpushed_reason(
    worktree: &Path,
    git_dir: Option<&Path>,
    deadline: Option<Instant>,
) -> Option<String> {
    if past_deadline(deadline) {
        return Some(DEADLINE_REASON.to_string());
    }
    let count = |range: &[&str]| -> Result<u64> {
        let mut command = repository_git(worktree, git_dir);
        command.args(["rev-list", "--count"]).args(range);
        let output = crate::util::run_output_with_timeout(command, GIT_TIMEOUT)?;
        if !output.status.success() {
            bail!("{}", sanitized_stderr_short(&output.stderr));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().parse()?)
    };
    let mut upstream = repository_git(worktree, git_dir);
    upstream.args(["rev-parse", "--verify", "--quiet", "@{upstream}"]);
    let has_upstream = crate::util::run_output_with_timeout(upstream, GIT_TIMEOUT)
        .is_ok_and(|output| output.status.success());
    if past_deadline(deadline) {
        return Some(DEADLINE_REASON.to_string());
    }
    let (result, label) = if has_upstream {
        (count(&["@{upstream}..HEAD"]), "has unpushed commits")
    } else {
        (
            count(&["HEAD", "--not", "--remotes"]),
            "has no upstream and commits on no remote branch",
        )
    };
    if past_deadline(deadline) {
        return Some(DEADLINE_REASON.to_string());
    }
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
/// inspection also preserves on Git errors, without claiming files changed.
pub(crate) fn tree_work(tree: &Path, deadline: Option<Instant>) -> Option<String> {
    if past_deadline(deadline) {
        return Some(DEADLINE_REASON.to_string());
    }
    match git_dirty_state(tree) {
        Ok(true) => return Some("has uncommitted changes".to_string()),
        Ok(false) => {}
        Err(error) => return Some(printable(&format!("cannot inspect dirty state: {error:#}"))),
    }
    unpushed_reason(tree, None, deadline).map(|reason| printable(&reason))
}

/// Select metadata explicitly when inspecting a discovered metadata directory;
/// Git must never fall back to a repository surrounding the cleanup process.
fn repository_git(dir: &Path, git_dir: Option<&Path>) -> std::process::Command {
    let mut command = hardened_git();
    command.arg("-C").arg(dir);
    if let Some(git_dir) = git_dir {
        command.arg("--git-dir").arg(git_dir);
    }
    command
}

/// Classify metadata with Git. Non-bare metadata needs a verified checkout:
/// without core.worktree Git defaults to its caller's directory. Linked
/// worktree administration instead identifies its checkout through registration.
fn metadata_checkout(metadata: &Path, deadline: Option<Instant>) -> Result<Option<PathBuf>> {
    let query = |dir: &Path, git_dir: Option<&Path>, args: &[&str]| -> Result<String> {
        if past_deadline(deadline) {
            bail!(DEADLINE_REASON);
        }
        let mut command = repository_git(dir, git_dir);
        command.args(args);
        let output = crate::util::run_output_with_timeout(command, GIT_TIMEOUT)?;
        if past_deadline(deadline) {
            bail!(DEADLINE_REASON);
        }
        if !output.status.success() {
            bail!("{}", sanitized_stderr_short(&output.stderr));
        }
        Ok(String::from_utf8(output.stdout)?
            .trim_end_matches('\n')
            .to_string())
    };
    match query(
        metadata,
        Some(metadata),
        &["rev-parse", "--is-bare-repository"],
    )?
    .as_str()
    {
        "true" => Ok(None),
        "false" => {
            let common = fs::canonicalize(metadata.join(query(
                metadata,
                Some(metadata),
                &["rev-parse", "--git-common-dir"],
            )?))
            .context("cannot resolve metadata's common directory")?;
            let canonical = fs::canonicalize(metadata).context("cannot resolve metadata")?;
            if common != canonical {
                let registered = query(
                    metadata,
                    Some(metadata),
                    &["worktree", "list", "--porcelain", "-z"],
                )?;
                for (index, checkout) in registered
                    .split('\0')
                    .filter_map(|field| field.strip_prefix("worktree "))
                    .enumerate()
                {
                    if index >= TREE_LIMIT {
                        bail!("more than {TREE_LIMIT} registered worktrees to check");
                    }
                    let checkout = Path::new(checkout);
                    let actual = query(checkout, None, &["rev-parse", "--absolute-git-dir"])
                        .ok()
                        .and_then(|path| fs::canonicalize(path).ok());
                    if actual.as_ref() == Some(&canonical) {
                        return fs::canonicalize(checkout)
                            .map(Some)
                            .context("cannot resolve registered checkout");
                    }
                }
                bail!("linked metadata has no verified registered checkout");
            }
            let configured = query(
                metadata,
                Some(metadata),
                &["config", "--path", "--get", "core.worktree"],
            )
            .map_err(|error| anyhow::anyhow!("cannot resolve core.worktree: {error:#}"))?;
            if configured.is_empty() {
                bail!("non-bare metadata has no explicit core.worktree");
            }
            // Git resolves relative core.worktree values against the Git dir.
            let checkout = fs::canonicalize(metadata.join(configured))
                .context("cannot resolve checkout configured by core.worktree")?;
            if git_common_dir(&checkout).as_ref() != Some(&common) {
                bail!("core.worktree does not identify a checkout of this repository");
            }
            Ok(Some(checkout))
        }
        _ => bail!("Git did not establish whether the repository is bare"),
    }
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
    for (tree, metadata) in candidates {
        if policy.past_deadline() {
            return Some(DEADLINE_REASON.to_string());
        }
        let reason = if metadata {
            match metadata_checkout(&tree, policy.deadline) {
                // Only a Git-confirmed bare repository skips dirty inspection.
                Ok(None) => unpushed_reason(&tree, Some(&tree), policy.deadline),
                Ok(Some(checkout)) => tree_work(&checkout, policy.deadline),
                Err(error) => Some(format!("cannot inspect repository metadata: {error:#}")),
            }
        } else {
            tree_work(&tree, policy.deadline)
        };
        if let Some(reason) = reason {
            return Some(format!("git tree {tree:?} {reason}"));
        }
        let Some(common) = git_common_dir(&tree) else {
            return Some(format!(
                "cannot inspect the backing repository of git tree {tree:?}"
            ));
        };
        if policy.past_deadline() {
            return Some(DEADLINE_REASON.to_string());
        }
        if common.starts_with(&root) {
            // Removing metadata also affects registered worktrees outside the
            // directory. Reuse execution storage's existing dependency rule.
            match crate::commands::recipe::run::run_checkout::exec_storage::repository_dependant(
                &root,
                &common,
                |tree| tree_work(tree, policy.deadline),
                policy.deadline,
            ) {
                Ok(Some(reason)) => return Some(reason),
                Ok(None) => {}
                Err(error) => {
                    return Some(format!("cannot inspect repository {common:?}: {error:#}"));
                }
            }
        }
    }
    policy.past_deadline().then(|| DEADLINE_REASON.to_string())
}

/// Every folder under `root`, `root` included, that holds a `.git` entry or
/// plausible repository metadata, sorted. Links are not followed and repository
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
        if policy.past_deadline() {
            return Err(DEADLINE_REASON.to_string());
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => return Err(cannot(&dir, error)),
        };
        let mut children = Vec::new();
        let (mut head, mut objects, mut refs, mut config) = (false, false, false, false);
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
            // Names identify candidates, including broken metadata links and
            // incomplete repositories. Types never authorize deletion.
            head |= name == "HEAD";
            objects |= name == "objects";
            refs |= name == "refs";
            config |= name == "config";
            // The type the folder listing already gives, so no entry costs a
            // stat of its own; like `symlink_metadata`, it does not follow a
            // link.
            match entry.file_type() {
                Ok(kind) => {
                    if kind.is_dir() {
                        children.push(entry.path());
                    }
                }
                Err(error) => return Err(cannot(&entry.path(), error)),
            }
        }
        if [head, objects, refs, config]
            .into_iter()
            .filter(|present| *present)
            .count()
            >= 2
        {
            // No classification here: Git must inspect even incomplete
            // metadata. Never walk object storage or linked directories.
            out.push((dir, true));
            children.retain(|path| {
                !path
                    .file_name()
                    .is_some_and(|name| name == "objects" || name == "refs" || name == "logs")
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
