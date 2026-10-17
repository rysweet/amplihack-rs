//! Run worktrees recorded as gitlinks in the index (issue #1528).
//!
//! Auto-drive committed from the launch checkout, whose index ended up holding
//! the run worktrees under `worktrees/` as gitlinks (mode 160000). A gitlink
//! there is a pointer to a live checkout, not repository content, so it is
//! refused in every mode and can never be allowlisted.
//!
//! A gitlink is a run worktree when its first path component is `worktrees`
//! (ASCII case-insensitive: macOS folds case) or when it is, or sits under, a
//! worktree registered in `git worktree list`. Real submodules elsewhere are
//! never flagged.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{ArtifactGuardError, git_stdout, normalize_slashes};

/// The three fixed [`ArtifactGuardError::WorktreeList`] payloads. They never
/// carry Git's output, so a forged worktree path cannot inject text into the
/// CLI's error line.
const WL_EMPTY: &str = "no worktree records";
const WL_RELATIVE: &str = "a worktree record is not an absolute path";
const WL_AMBIGUOUS: &str = "ambiguous worktree records";

/// Repo-relative paths (sorted, deduplicated, `/`-separated, no trailing `/`)
/// of every worktree registered inside the scanned top level `top`, prunable
/// ones included. `top` itself and worktrees outside it are excluded.
///
/// The list decides whether the CLI may print a deleting command for a root,
/// so it is read fail-loud: a failed or untrustworthy `git worktree list` is
/// an error, never an empty list.
pub(super) fn registered_worktrees(top: &Path) -> Result<Vec<String>, ArtifactGuardError> {
    let stdout = git_stdout(top, &["worktree", "list", "--porcelain"])?;
    let mut registered = BTreeSet::new();
    for record in parse_worktree_porcelain(&String::from_utf8_lossy(&stdout))? {
        let path = canonical_best_effort(&record);
        let Ok(rel) = path.strip_prefix(top) else {
            continue;
        };
        let rel = normalize_slashes(rel.to_string_lossy().trim_end_matches('/'));
        if !rel.is_empty() {
            registered.insert(rel);
        }
    }
    Ok(registered.into_iter().collect())
}

/// The `worktree <abs>` paths of newline-separated porcelain output (never
/// `-z`, which older Git lacks). A `worktree` line that arrives before the
/// open record has shown `HEAD` or `bare` means a path held a newline and
/// forged a second record, so the whole list is refused rather than guessed.
fn parse_worktree_porcelain(porcelain: &str) -> Result<Vec<PathBuf>, ArtifactGuardError> {
    let ambiguous = || ArtifactGuardError::WorktreeList(WL_AMBIGUOUS.to_string());
    let mut records = Vec::new();
    // `Some(seen_head_or_bare)` while a record is open.
    let mut open: Option<bool> = None;
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if open.is_some() {
                return Err(ambiguous());
            }
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(ArtifactGuardError::WorktreeList(WL_RELATIVE.to_string()));
            }
            records.push(path);
            open = Some(false);
        } else if line.is_empty() {
            match open.take() {
                Some(true) | None => {}
                Some(false) => return Err(ambiguous()),
            }
        } else if line.starts_with("HEAD ") || line == "bare" {
            if open.is_none() {
                return Err(ambiguous());
            }
            open = Some(true);
        } else if open.is_none() {
            return Err(ambiguous());
        }
    }
    if open == Some(false) {
        return Err(ambiguous());
    }
    if records.is_empty() {
        return Err(ArtifactGuardError::WorktreeList(WL_EMPTY.to_string()));
    }
    Ok(records)
}

/// Canonicalize `path`, or its parent when a prunable worktree's directory is
/// gone, so it compares against the canonical top level.
fn canonical_best_effort(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => parent
            .canonicalize()
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// Repo-relative paths of index entries with mode exactly 160000, deduplicated
/// across conflict stages. `git ls-files -s -z` prints
/// `<mode> <sha> <stage>\t<path>`; a path may hold a tab, so only the first
/// tab separates the fields.
pub(super) fn index_gitlinks(top: &Path) -> Result<BTreeSet<String>, ArtifactGuardError> {
    let stdout = git_stdout(top, &["ls-files", "-s", "-z"])?;
    let mut gitlinks = BTreeSet::new();
    for record in stdout.split(|byte| *byte == 0) {
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        if record[..tab].starts_with(b"160000 ") {
            gitlinks.insert(normalize_slashes(&String::from_utf8_lossy(
                &record[tab + 1..],
            )));
        }
    }
    Ok(gitlinks)
}

/// Whether the gitlink at `path` records a run worktree rather than a real
/// submodule. See the module docs.
pub(super) fn is_run_worktree_gitlink(path: &str, registered: &[String]) -> bool {
    let first = path.split('/').next().unwrap_or_default();
    first.eq_ignore_ascii_case("worktrees")
        || registered
            .iter()
            .any(|worktree| is_at_or_under(path, worktree))
}

/// Whether `path` equals `root` or lies beneath it.
pub(super) fn is_at_or_under(path: &str, root: &str) -> bool {
    path.strip_prefix(root)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}
