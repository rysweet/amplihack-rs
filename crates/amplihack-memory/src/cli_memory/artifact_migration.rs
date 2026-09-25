//! One-time migration of in-repo code-index artifacts into the per-project
//! cache (issue #1476).
//!
//! This exists so existing users do not pay a 600-second re-index on their
//! first upgraded session, and so the 8 MB `graph_db` already sitting in their
//! checkout stops being `git add -A` bait.
//!
//! It never deletes anything it did not move.

use super::artifact_root::{ensure_artifact_root, project_artifact_root};
use super::fs_move::move_path;
use anyhow::{Context, Result};
use fs4::fs_std::FileExt;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Filename of the advisory lock serialising concurrent migrations. Git
/// worktrees and parallel agent sessions make two migrations racing on one
/// project the normal case, not an exotic one.
const LOCK_FILE: &str = ".migration.lock";

/// One artifact that left the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedArtifact {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Why one artifact stayed where it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactSkipReason {
    /// Something already occupies the destination. The user decides.
    DestinationExists,
    /// Git tracks this file, so it is repository-supplied data rather than
    /// amplihack's own output. See [`migrate_in_repo_artifacts`].
    Tracked,
    /// The source is a symlink. Never followed.
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedArtifact {
    pub from: PathBuf,
    pub reason: ArtifactSkipReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedArtifact {
    pub from: PathBuf,
    pub error: String,
}

/// Why the whole migration stood down. Both values are normal outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationSkipReason {
    /// An indexer is writing right now. Nothing may move under a live writer.
    IndexerRunning,
    /// Another session holds the migration lock and is doing the work.
    LockUnavailable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub moved: Vec<MovedArtifact>,
    pub skipped: Vec<SkippedArtifact>,
    pub failed: Vec<FailedArtifact>,
    pub skipped_reason: Option<MigrationSkipReason>,
}

impl MigrationReport {
    fn stood_down(reason: MigrationSkipReason) -> Self {
        Self {
            skipped_reason: Some(reason),
            ..Self::default()
        }
    }

    /// Whether anything at all happened worth telling the user about.
    pub fn is_empty(&self) -> bool {
        self.moved.is_empty() && self.skipped.is_empty() && self.failed.is_empty()
    }
}

/// The artifact names the migration recognises, in the order they are moved,
/// paired with their destination filename under the artifact root.
///
/// The ordering is load-bearing in exactly one place: the repo-root
/// `index.scip` is the file the indexers actually wrote, so it claims
/// `<root>/index.scip`. A `.amplihack/index.scip` — a staging path amplihack
/// only ever read — is superseded by it and is preserved under a distinct name
/// rather than being left in the checkout or deleted.
const RECOGNISED_ARTIFACTS: &[(&str, &str)] = &[
    ("index.scip", "index.scip"),
    (".amplihack/graph_db", "graph_db"),
    (".amplihack/kuzu_db", "kuzu_db"),
    (".amplihack/indexes", "indexes"),
    (".amplihack/blarify.json", "blarify.json"),
    (".amplihack/blarify_stale", "blarify_stale"),
    (".amplihack/indexing.pid", "indexing.pid"),
    (".amplihack/index.scip", "index.scip.superseded"),
    (".amplihack/index.scip.backup", "index.scip.backup"),
];

/// Move a project's in-repo code-index artifacts into its cache directory.
///
/// `Err` is reserved for one condition: the artifact root cannot be resolved at
/// all. A live indexer, lock contention, a destination conflict, an unreadable
/// source and a Git-tracked source are all values in the report — a migration
/// that fails session start invites users to work around it by deleting the
/// cache.
///
/// Git-tracked sources are skipped rather than moved. That is not politeness
/// about the working tree: a committed file arrived through the repository and
/// its contents are attacker-influenced like any other checked-in file. Moving
/// those bytes into `~/.cache/amplihack/` would promote them into trusted,
/// agent-readable storage that later reads treat as amplihack's own output.
pub fn migrate_in_repo_artifacts(project_path: &Path) -> Result<MigrationReport> {
    // Triggered by the presence of a source, never by an empty cache: a
    // project that has genuinely never been indexed has nothing to migrate and
    // must not pay for a cache-root resolution to find that out.
    if collect_sources(project_path).is_empty() {
        return Ok(MigrationReport::default());
    }

    if legacy_or_current_indexer_is_running(project_path)? {
        return Ok(MigrationReport::stood_down(
            MigrationSkipReason::IndexerRunning,
        ));
    }

    let root = ensure_artifact_root(project_path)?.root;
    let Some(_lock) = try_take_migration_lock(&root)? else {
        return Ok(MigrationReport::stood_down(
            MigrationSkipReason::LockUnavailable,
        ));
    };

    let mut report = MigrationReport::default();
    let tracked = tracked_paths(project_path);

    // Re-collect under the lock: another session may have finished between the
    // trigger check and here.
    for relative in collect_sources(project_path) {
        let from = project_path.join(relative);
        let Some(destination) = RECOGNISED_ARTIFACTS
            .iter()
            .find(|(source, _)| *source == relative)
            .map(|(_, name)| root.join(name))
        else {
            continue;
        };

        if is_symlink(&from) {
            tracing::warn!(
                "artifact migration: refusing to follow the symlink {}",
                from.display()
            );
            report.skipped.push(SkippedArtifact {
                from,
                reason: ArtifactSkipReason::Symlink,
            });
            continue;
        }
        if is_tracked(&tracked, relative) {
            tracing::info!(
                "artifact migration: leaving the git-tracked {} in place; \
                 `git rm --cached` plus a .gitignore entry is your call",
                from.display()
            );
            report.skipped.push(SkippedArtifact {
                from,
                reason: ArtifactSkipReason::Tracked,
            });
            continue;
        }
        if fs::symlink_metadata(&destination).is_ok() {
            tracing::info!(
                "artifact migration: {} already exists, keeping {}",
                destination.display(),
                from.display()
            );
            report.skipped.push(SkippedArtifact {
                from,
                reason: ArtifactSkipReason::DestinationExists,
            });
            continue;
        }

        match move_path(&from, &destination) {
            Ok(()) => {
                tracing::info!(
                    "artifact migration: moved {} -> {}",
                    from.display(),
                    destination.display()
                );
                report.moved.push(MovedArtifact {
                    from,
                    to: destination,
                });
            }
            Err(err) => {
                tracing::warn!("artifact migration: {} failed: {err:#}", from.display());
                report.failed.push(FailedArtifact {
                    from,
                    error: format!("{err:#}"),
                });
            }
        }
    }

    remove_emptied_artifact_dir(project_path);
    Ok(report)
}

/// The in-repo artifacts that exist right now, as repo-relative strings.
fn collect_sources(project_path: &Path) -> Vec<&'static str> {
    RECOGNISED_ARTIFACTS
        .iter()
        .filter(|(relative, _)| fs::symlink_metadata(project_path.join(relative)).is_ok())
        .map(|(relative, _)| *relative)
        .collect()
}

/// Read both PID files.
///
/// Checking only the new location fails open on the single upgrade every
/// existing user performs: an unmigrated project has no
/// `<artifact_root>/indexing.pid`, because the artifact root did not exist when
/// the job started. The check would see no lock and move `graph_db` out from
/// under a live writer.
fn legacy_or_current_indexer_is_running(project_path: &Path) -> Result<bool> {
    let legacy = project_path.join(".amplihack").join("indexing.pid");
    if pid_file_is_live(&legacy) {
        return Ok(true);
    }
    // A root we cannot resolve yet is not a reason to claim an indexer is
    // running; the caller's `ensure_artifact_root` reports that failure.
    let Ok(root) = project_artifact_root(project_path) else {
        return Ok(false);
    };
    Ok(pid_file_is_live(&root.join("indexing.pid")))
}

fn pid_file_is_live(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .is_some_and(super::indexing_job::is_process_alive)
}

/// Guard whose drop releases the advisory lock.
struct MigrationLock(fs::File);

impl Drop for MigrationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn try_take_migration_lock(root: &Path) -> Result<Option<MigrationLock>> {
    let path = root.join(LOCK_FILE);
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(true) => Ok(Some(MigrationLock(file))),
        Ok(false) => Ok(None),
        Err(_) => Ok(None),
    }
}

/// Repository-relative paths Git tracks, or an empty set outside a repository.
fn tracked_paths(project_path: &Path) -> BTreeSet<String> {
    let Ok(output) = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(project_path)
        .output()
    else {
        return BTreeSet::new();
    };
    if !output.status.success() {
        return BTreeSet::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// A directory counts as tracked when Git tracks anything inside it.
fn is_tracked(tracked: &BTreeSet<String>, relative: &str) -> bool {
    let prefix = format!("{relative}/");
    tracked
        .iter()
        .any(|entry| entry == relative || entry.starts_with(&prefix))
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// Remove `.amplihack/` only when the migration emptied it. `remove_dir` fails
/// on a non-empty directory, which is exactly the behaviour wanted: anything a
/// human or another subsystem put there keeps the directory alive.
fn remove_emptied_artifact_dir(project_path: &Path) {
    let _ = fs::remove_dir(project_path.join(".amplihack"));
}

/// Human-readable summary of what moved, for the session-start context block.
///
/// `None` when nothing happened — a project that never had in-repo artifacts
/// must not gain a migration notice.
pub fn describe_migration(report: &MigrationReport) -> Option<String> {
    if report.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for moved in &report.moved {
        lines.push(format!(
            "Moved `{}` -> `{}`",
            moved.from.display(),
            moved.to.display()
        ));
    }
    for skipped in &report.skipped {
        let reason = match skipped.reason {
            ArtifactSkipReason::DestinationExists => "a file already exists at its new location",
            ArtifactSkipReason::Tracked => {
                "git tracks it; `git rm --cached` plus a .gitignore entry is your call"
            }
            ArtifactSkipReason::Symlink => "it is a symlink and was not followed",
        };
        lines.push(format!("Kept `{}`: {reason}", skipped.from.display()));
    }
    for failed in &report.failed {
        lines.push(format!(
            "Kept `{}`: {}",
            failed.from.display(),
            failed.error
        ));
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "Code-index artifacts now live outside your checkout (issue #1476).\n\n{}",
        lines.join("\n")
    ))
}

#[cfg(test)]
#[path = "artifact_migration_tests.rs"]
mod tests;
