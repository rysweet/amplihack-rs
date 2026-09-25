//! crates/amplihack-utils/src/tests/artifact_guard_issue_1476_tests.rs
//!
//! Issue #1476 / AC11 + AC13 — the guard had a gap; it did not become stale.
//!
//! Register from `artifact_guard.rs`:
//! ```ignore
//! #[cfg(test)]
//! #[path = "tests/artifact_guard_issue_1476_tests.rs"]
//! mod issue_1476_tests;
//! ```
//!
//! Worth stating plainly, because the reported incident invites the opposite
//! conclusion: the guard flags `index.scip` at any depth, and prohibits
//! `.amplihack/` only for `session-state`. It never covered
//! `.amplihack/graph_db`, so it would **not** have caught the 8 MB file that
//! `git add -A` staged in `rysweet/amplihack-recipe-runner`.
//!
//! Nothing here weakens the guard. The `index.scip` rule stays — it still
//! protects checkouts that have not migrated yet — and three names are added.

use super::{ArtifactGuardConfig, ArtifactGuardMode, ArtifactSource, scan_artifacts};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn run_git(repo: &Path, args: &[&str]) {
    let output = amplihack_git::command()
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", repo.display()));
    assert!(
        output.status.success(),
        "git {args:?} failed in {}\nstderr:\n{}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn repo() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    run_git(tmp.path(), &["init", "-q"]);
    run_git(
        tmp.path(),
        &["config", "user.email", "guard@example.invalid"],
    );
    run_git(tmp.path(), &["config", "user.name", "Issue 1476"]);
    write_file(&tmp.path().join("README.md"), "# fixture\n");
    run_git(tmp.path(), &["add", "README.md"]);
    run_git(tmp.path(), &["commit", "-qm", "initial"]);
    tmp
}

fn scan(repo: &Path) -> Vec<String> {
    scan_artifacts(&ArtifactGuardConfig::new(repo).with_mode(ArtifactGuardMode::All))
        .unwrap()
        .violations
        .into_iter()
        .map(|v| v.path)
        .collect()
}

/// AC11 — the incident, replayed. An 8 MB Kuzu database under `.amplihack/`,
/// untracked in a repository with no `.gitignore` entry for it, is exactly what
/// `git add -A` staged. The guard must now say so.
#[test]
fn an_in_repo_graph_db_is_reported() {
    let tmp = repo();
    write_file(
        &tmp.path().join(".amplihack/graph_db/data.kz"),
        "pretend 8 megabytes",
    );

    let paths = scan(tmp.path());

    assert!(
        paths.iter().any(|p| p.starts_with(".amplihack/graph_db")),
        "the guard must flag .amplihack/graph_db; got {paths:?}"
    );
}

#[test]
fn the_legacy_kuzu_db_and_the_indexes_directory_are_reported() {
    let tmp = repo();
    write_file(&tmp.path().join(".amplihack/kuzu_db/legacy.kz"), "legacy");
    write_file(&tmp.path().join(".amplihack/indexes/python.scip"), "scip");

    let paths = scan(tmp.path());

    assert!(
        paths.iter().any(|p| p.starts_with(".amplihack/kuzu_db")),
        "got {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.starts_with(".amplihack/indexes")),
        "got {paths:?}"
    );
}

/// AC13 — nothing is weakened. `index.scip` at the repository root and at any
/// depth is still a violation; un-migrated checkouts still depend on it.
#[test]
fn the_existing_index_scip_rule_is_unchanged() {
    let tmp = repo();
    write_file(&tmp.path().join("index.scip"), "scip");
    write_file(&tmp.path().join("nested/pkg/index.scip"), "scip");

    let paths = scan(tmp.path());

    assert!(paths.iter().any(|p| p == "index.scip"), "got {paths:?}");
    assert!(
        paths.iter().any(|p| p == "nested/pkg/index.scip"),
        "got {paths:?}"
    );
}

/// The widening is narrow. `.amplihack/` is not blanket-prohibited: entries a
/// human put there must not start failing the guard.
#[test]
fn unrelated_amplihack_entries_are_not_newly_prohibited() {
    let tmp = repo();
    write_file(&tmp.path().join(".amplihack/config.json"), "{}");
    write_file(&tmp.path().join(".amplihack/notes.md"), "# notes\n");

    let paths = scan(tmp.path());

    assert!(
        !paths
            .iter()
            .any(|p| p.ends_with("config.json") || p.ends_with("notes.md")),
        "widening must not become a blanket .amplihack/ ban; got {paths:?}"
    );
}

/// A staged graph DB is the exact `git add -A` case, and must be caught before
/// the commit rather than after the push.
#[test]
fn a_staged_graph_db_is_caught_at_pre_commit_time() {
    let tmp = repo();
    write_file(&tmp.path().join(".amplihack/graph_db/data.kz"), "8mb");
    run_git(tmp.path(), &["add", "-A"]);

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PreCommit),
    )
    .unwrap();

    assert!(
        report.violations.iter().any(
            |v| v.path.starts_with(".amplihack/graph_db") && v.source == ArtifactSource::Staged
        ),
        "got {:#?}",
        report.violations
    );
}

/// The guard reasons over repository-relative strings only. It must not gain a
/// dependency on `amplihack-memory` or learn anything about cache directories —
/// a path outside the repository is not its business.
#[test]
fn the_guard_says_nothing_about_artifacts_in_the_cache() {
    let tmp = repo();
    let cache = TempDir::new().unwrap();
    write_file(
        &cache
            .path()
            .join("amplihack/projects/fixture-0123456789abcdef/graph_db/data.kz"),
        "8mb",
    );

    let paths = scan(tmp.path());

    assert!(
        paths.is_empty(),
        "artifacts outside the repository are not violations; got {paths:?}"
    );
}
