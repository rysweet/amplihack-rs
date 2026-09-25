//! crates/amplihack-memory/src/cli_memory/artifact_migration_tests.rs
//!
//! Issue #1476 — contracts for `cli_memory::artifact_migration`.
//!
//! Wire up from `artifact_migration.rs` with:
//! ```ignore
//! #[cfg(test)]
//! #[path = "artifact_migration_tests.rs"]
//! mod tests;
//! ```
//!
//! The migration exists so existing users do not pay a 600-second re-index on
//! their first upgraded session, and so the 8 MB `graph_db` already sitting in
//! their checkout stops being `git add -A` bait. It never deletes anything it
//! did not move.

use super::{
    ArtifactSkipReason, MigrationSkipReason, ensure_artifact_root, migrate_in_repo_artifacts,
    project_artifact_root,
};
// Not via `super::`: `project_artifact_paths` lives in `types`, and the parent
// module has no use for it. Importing it there just so this child could reach
// it through `super::` left a dead import in non-test builds.
use crate::cli_memory::project_artifact_paths;
use crate::test_support::{HomeGuard, env_lock};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: serialised by env_lock().
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: serialised by env_lock().
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: serialised by env_lock().
        unsafe {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Fixture {
    project: TempDir,
    _home: TempDir,
    cache: TempDir,
    _home_guard: HomeGuard,
    _xdg: EnvVarGuard,
    _override: EnvVarGuard,
}

/// A checkout in the pre-#1476 layout: every artifact name the migration knows
/// about, in the repository, exactly where indexing used to drop them.
fn legacy_project() -> Fixture {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let _home_guard = HomeGuard::set(home.path());
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", cache.path());
    let _override = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");

    let amplihack = project.path().join(".amplihack");
    write(&amplihack.join("graph_db/data.kz"), "8mb-pretend");
    write(&amplihack.join("kuzu_db/legacy.kz"), "legacy");
    write(&amplihack.join("indexes/python.scip"), "python-scip");
    write(&amplihack.join("blarify.json"), "{}");
    write(&amplihack.join("index.scip"), "artifact-scip");
    write(&amplihack.join("index.scip.backup"), "backup-scip");
    write(&amplihack.join("blarify_stale"), "");
    write(&project.path().join("index.scip"), "root-scip");

    Fixture {
        project,
        _home: home,
        cache,
        _home_guard,
        _xdg,
        _override,
    }
}

#[allow(dead_code)]
fn moved_destinations(report: &super::MigrationReport) -> Vec<PathBuf> {
    report.moved.iter().map(|m| m.to.clone()).collect()
}

// ── the core outcome ──────────────────────────────────────────────────────

/// The reported bug, inverted into an assertion: after migration the checkout
/// holds none of it.
#[test]
fn migration_empties_the_checkout_of_known_artifacts() {
    let _g = lock();
    let fx = legacy_project();

    let report = migrate_in_repo_artifacts(fx.project.path()).unwrap();

    assert!(report.skipped_reason.is_none(), "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(
        !fx.project.path().join("index.scip").exists(),
        "bare index.scip must leave the repository root"
    );
    assert!(
        !fx.project.path().join(".amplihack").exists(),
        ".amplihack must be removed once it is empty"
    );

    let root = project_artifact_root(fx.project.path()).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("graph_db/data.kz")).unwrap(),
        "8mb-pretend"
    );
    assert_eq!(
        fs::read_to_string(root.join("indexes/python.scip")).unwrap(),
        "python-scip"
    );
    assert_eq!(fs::read_to_string(root.join("blarify.json")).unwrap(), "{}");
    assert_eq!(
        fs::read_to_string(root.join("index.scip")).unwrap(),
        "root-scip",
        "the repo-root index.scip is the one indexers actually wrote; it must \
         win the destination, not the stale .amplihack/index.scip"
    );
}

/// The migration must land the artifacts exactly where `project_artifact_paths`
/// will go looking for them — otherwise it succeeds and the next session
/// re-indexes from scratch anyway.
#[test]
fn migrated_artifacts_land_on_the_resolver_paths() {
    let _g = lock();
    let fx = legacy_project();

    migrate_in_repo_artifacts(fx.project.path()).unwrap();

    let paths = project_artifact_paths(fx.project.path()).unwrap();
    assert!(paths.blarify_json.exists());
    assert!(paths.indexes_dir.join("python.scip").exists());
    assert!(paths.blarify_stale.exists());
    for path in [
        &paths.artifact_dir,
        &paths.indexes_dir,
        &paths.blarify_json,
        &paths.index_scip,
        &paths.indexing_pid,
        &paths.blarify_stale,
    ] {
        assert!(
            !path.starts_with(fx.project.path()),
            "{} is inside the checkout",
            path.display()
        );
    }
}

#[test]
fn migration_report_names_every_moved_path_on_both_sides() {
    let _g = lock();
    let fx = legacy_project();

    let report = migrate_in_repo_artifacts(fx.project.path()).unwrap();

    assert!(!report.moved.is_empty());
    for moved in &report.moved {
        assert!(
            moved.from.starts_with(fx.project.path()),
            "source must be in the checkout: {}",
            moved.from.display()
        );
        assert!(
            moved.to.starts_with(fx.cache.path()),
            "destination must be in the cache: {}",
            moved.to.display()
        );
        assert!(!moved.from.exists());
        assert!(moved.to.exists());
    }
}

#[test]
fn migration_is_a_no_op_on_the_second_run() {
    let _g = lock();
    let fx = legacy_project();

    let first = migrate_in_repo_artifacts(fx.project.path()).unwrap();
    let second = migrate_in_repo_artifacts(fx.project.path()).unwrap();

    assert!(!first.moved.is_empty());
    assert!(second.moved.is_empty(), "{second:?}");
    assert!(second.failed.is_empty());
}

/// Triggered by source presence, never by pointer absence or an empty cache —
/// a project that never had in-repo artifacts must not be "migrated".
#[test]
fn migration_does_nothing_for_a_project_with_no_in_repo_artifacts() {
    let _g = lock();
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let _hg = HomeGuard::set(home.path());
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", cache.path());
    let _ov = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");
    write(&project.path().join("src/main.rs"), "fn main() {}\n");

    let report = migrate_in_repo_artifacts(project.path()).unwrap();

    assert!(report.moved.is_empty());
    assert!(report.failed.is_empty());
}

// ── "never deletes anything it did not move" ──────────────────────────────

#[test]
fn migration_leaves_unknown_entries_and_keeps_the_directory_holding_them() {
    let _g = lock();
    let fx = legacy_project();
    let stranger = fx.project.path().join(".amplihack/notes-from-a-human.md");
    write(&stranger, "do not eat");
    let session_state = fx.project.path().join(".amplihack/session-state/run.json");
    write(&session_state, "{}");

    migrate_in_repo_artifacts(fx.project.path()).unwrap();

    assert_eq!(fs::read_to_string(&stranger).unwrap(), "do not eat");
    assert!(session_state.exists());
    assert!(
        fx.project.path().join(".amplihack").exists(),
        ".amplihack must survive while it still holds entries the migration did not move"
    );
    assert!(
        !fx.project.path().join(".amplihack/graph_db").exists(),
        "known artifacts still move even when unknown entries stay"
    );
}

/// A destination conflict keeps the source. The user can then decide; the
/// migration records it and moves on to the remaining items.
#[test]
fn migration_keeps_the_source_when_the_destination_already_exists() {
    let _g = lock();
    let fx = legacy_project();
    let root = ensure_artifact_root(fx.project.path()).unwrap().root;
    write(&root.join("blarify.json"), "already here");

    let report = migrate_in_repo_artifacts(fx.project.path()).unwrap();

    assert_eq!(
        fs::read_to_string(root.join("blarify.json")).unwrap(),
        "already here",
        "an existing destination must not be clobbered"
    );
    assert_eq!(
        fs::read_to_string(fx.project.path().join(".amplihack/blarify.json")).unwrap(),
        "{}",
        "the source must be kept, not deleted"
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.reason == ArtifactSkipReason::DestinationExists
                && s.from.ends_with("blarify.json")),
        "conflict must be recorded: {report:?}"
    );
    assert!(
        report.moved.iter().any(|m| m.to.ends_with("data.kz")
            || m.to.file_name().is_some_and(|n| n == "graph_db")),
        "one conflict must not abort the rest: {report:?}"
    );
}

/// SEC-5 / AC23 — locally generated artifacts are untracked by construction.
/// Anything tracked came from whoever wrote the repository; moving a committed
/// `blarify.json` into `~/.cache` promotes attacker-influenced bytes into
/// trusted, agent-readable storage.
#[test]
fn migration_skips_git_tracked_sources() {
    let _g = lock();
    let fx = legacy_project();
    git(fx.project.path(), &["init", "-q"]);
    git(
        fx.project.path(),
        &["config", "user.email", "t@example.invalid"],
    );
    git(fx.project.path(), &["config", "user.name", "T"]);
    git(fx.project.path(), &["add", "-f", ".amplihack/blarify.json"]);
    git(fx.project.path(), &["commit", "-qm", "committed artifact"]);

    let report = migrate_in_repo_artifacts(fx.project.path()).unwrap();

    assert!(
        fx.project.path().join(".amplihack/blarify.json").exists(),
        "a tracked artifact must stay in the repository"
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.reason == ArtifactSkipReason::Tracked && s.from.ends_with("blarify.json")),
        "tracked skip must be recorded so the PR claim is observable: {report:?}"
    );
    assert!(
        !fx.project.path().join(".amplihack/graph_db").exists(),
        "untracked artifacts still migrate alongside a tracked one"
    );
}

/// SEC-4 / AC21 — a repository-planted symlink must never be followed. Same
/// device: rename moves the link and every later graph write escapes. Cross
/// device: the remove step deletes outside the checkout.
#[test]
#[cfg(unix)]
fn migration_refuses_a_symlinked_source() {
    let _g = lock();
    let fx = legacy_project();
    let outside = TempDir::new().unwrap();
    write(&outside.path().join("secret"), "not yours");
    let planted = fx.project.path().join(".amplihack/indexes");
    fs::remove_dir_all(&planted).unwrap();
    std::os::unix::fs::symlink(outside.path(), &planted).unwrap();

    let report = migrate_in_repo_artifacts(fx.project.path()).unwrap();

    assert!(
        outside.path().join("secret").exists(),
        "the migration must not reach outside the checkout"
    );
    assert!(
        planted.symlink_metadata().unwrap().file_type().is_symlink(),
        "the planted link must be left exactly as found"
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.reason == ArtifactSkipReason::Symlink),
        "symlink refusal must be recorded: {report:?}"
    );
    assert!(
        !moved_destinations(&report)
            .iter()
            .any(|p| p.ends_with("indexes")),
        "a symlinked source must not appear as moved"
    );
}

// ── SEC-9 / AC22: liveness ────────────────────────────────────────────────

/// SEC-9 — the blocking one. A liveness check that only reads
/// `<artifact_root>/indexing.pid` cannot find anything for an unmigrated
/// project, so it fails open on the one upgrade every user performs and moves
/// `graph_db` out from under a live writer. Both locations must be read.
#[test]
fn migration_skips_when_the_legacy_in_repo_pid_names_a_live_process() {
    let _g = lock();
    let fx = legacy_project();
    write(
        &fx.project.path().join(".amplihack/indexing.pid"),
        &format!("{}\n", std::process::id()),
    );

    let report = migrate_in_repo_artifacts(fx.project.path()).unwrap();

    assert_eq!(
        report.skipped_reason,
        Some(MigrationSkipReason::IndexerRunning),
        "{report:?}"
    );
    assert!(report.moved.is_empty());
    assert!(
        fx.project
            .path()
            .join(".amplihack/graph_db/data.kz")
            .exists(),
        "nothing may move while an indexer is writing"
    );
}

#[test]
fn migration_proceeds_when_the_legacy_pid_is_dead() {
    let _g = lock();
    let fx = legacy_project();
    // PID 0 is never a live process for `kill(0)` purposes.
    write(&fx.project.path().join(".amplihack/indexing.pid"), "0\n");

    let report = migrate_in_repo_artifacts(fx.project.path()).unwrap();

    assert_eq!(report.skipped_reason, None, "{report:?}");
    assert!(!report.moved.is_empty());
}

/// AC20 — git worktrees and parallel agent sessions are the normal case here,
/// so two migrations racing on one project is expected, not exotic. Exactly one
/// does the work; the other reports contention. Neither loses bytes.
#[test]
fn concurrent_migrations_serialise_without_losing_artifacts() {
    let _g = lock();
    let fx = legacy_project();
    let project = fx.project.path().to_path_buf();

    let reports = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let project = project.clone();
                scope.spawn(move || migrate_in_repo_artifacts(&project).unwrap())
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });

    let total_moved: usize = reports.iter().map(|r| r.moved.len()).sum();
    assert!(total_moved > 0, "somebody must do the work");
    for report in &reports {
        assert!(
            report.failed.is_empty(),
            "contention must not fail: {report:?}"
        );
        if !report.moved.is_empty() {
            assert_eq!(report.skipped_reason, None);
        }
    }

    let root = project_artifact_root(fx.project.path()).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("graph_db/data.kz")).unwrap(),
        "8mb-pretend"
    );
    assert!(!fx.project.path().join(".amplihack").exists());

    // No path may be reported moved twice — that would mean two threads both
    // believed they owned it.
    let mut all: Vec<_> = reports
        .iter()
        .flat_map(|r| r.moved.iter().map(|m| m.to.clone()))
        .collect();
    let before = all.len();
    all.sort();
    all.dedup();
    assert_eq!(before, all.len(), "a destination was claimed twice");
}

// ── error contract ────────────────────────────────────────────────────────

/// `Err` is reserved for "artifact root unresolvable". Everything else — a live
/// indexer, lock contention, a conflict, an unreadable source — is a value.
/// A migration that fails session start invites users to work around it by
/// deleting the cache.
#[test]
fn migration_errs_only_when_the_artifact_root_is_unresolvable() {
    let _g = lock();
    let fx = legacy_project();
    let _home = EnvVarGuard::unset("HOME");
    let _xdg = EnvVarGuard::unset("XDG_CACHE_HOME");

    assert!(migrate_in_repo_artifacts(fx.project.path()).is_err());
}

#[test]
fn migration_records_an_unreadable_source_as_a_failure_and_continues() {
    let _g = lock();
    let fx = legacy_project();
    // A directory the process cannot traverse stands in for any per-item IO
    // failure; the remaining items must still migrate.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = fx.project.path().join(".amplihack/graph_db");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let report = migrate_in_repo_artifacts(fx.project.path()).unwrap();

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            report.moved.iter().any(|m| m.to.ends_with("blarify.json")),
            "one bad item must not abort the rest: {report:?}"
        );
    }
}
