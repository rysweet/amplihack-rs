//! crates/amplihack-memory/src/cli_memory/fs_move_tests.rs
//!
//! Issue #1476 — contracts for `cli_memory::fs_move::move_path`.
//!
//! Wire up from `fs_move.rs` with:
//! ```ignore
//! #[cfg(test)]
//! #[path = "fs_move_tests.rs"]
//! mod tests;
//! ```
//!
//! Why this helper exists at all: the migration moves a multi-megabyte
//! `graph_db` **directory** from a checkout (often on `/`) to `~/.cache`, which
//! can be a different filesystem. `fs::rename` returns `EXDEV` there, and std
//! has no directory fallback.
//!
//! `copy_then_remove` is the EXDEV branch exposed as its own entry point so the
//! cross-device path is testable without mounting a second filesystem (AC16).
//! `move_path` must call exactly this function when `rename` reports `EXDEV`.

use super::{copy_then_remove, move_path};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

// ── happy paths ───────────────────────────────────────────────────────────

#[test]
fn move_path_moves_a_file() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("index.scip");
    let dst = tmp.path().join("cache/indexes/python.scip");
    write(&src, "scip-bytes");
    fs::create_dir_all(dst.parent().unwrap()).unwrap();

    move_path(&src, &dst).unwrap();

    assert!(!src.exists(), "source must be gone after a move");
    assert_eq!(fs::read_to_string(&dst).unwrap(), "scip-bytes");
}

#[test]
fn move_path_moves_a_non_empty_directory() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("graph_db");
    write(&src.join("catalog/nodes.kz"), "node-bytes");
    write(&src.join("data.kz"), "data-bytes");
    let dst = tmp.path().join("cache/graph_db");
    fs::create_dir_all(dst.parent().unwrap()).unwrap();

    move_path(&src, &dst).unwrap();

    assert!(!src.exists());
    assert_eq!(
        fs::read_to_string(dst.join("catalog/nodes.kz")).unwrap(),
        "node-bytes"
    );
    assert_eq!(
        fs::read_to_string(dst.join("data.kz")).unwrap(),
        "data-bytes"
    );
}

/// AC16 — the cross-device path, exercised directly. A rename-only
/// implementation passes every same-filesystem test and fails the one migration
/// every user actually performs.
#[test]
fn copy_then_remove_moves_a_non_empty_directory_across_devices() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("graph_db");
    write(&src.join("a/b/c.kz"), "deep");
    write(&src.join("top.kz"), "top");
    let dst = tmp.path().join("cache/graph_db");
    fs::create_dir_all(dst.parent().unwrap()).unwrap();

    copy_then_remove(&src, &dst).unwrap();

    assert!(
        !src.exists(),
        "source must be removed only after a verified copy"
    );
    assert_eq!(fs::read_to_string(dst.join("a/b/c.kz")).unwrap(), "deep");
    assert_eq!(fs::read_to_string(dst.join("top.kz")).unwrap(), "top");
}

/// AC28 / R18 — `fs::copy` does not preserve mtime. Without `set_modified`,
/// every cross-device migration makes a genuinely stale index look freshly
/// built, and `latest_scip_artifact` picks the wrong file.
#[test]
fn copy_then_remove_preserves_modification_times() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("indexes");
    write(&src.join("python.scip"), "old");
    let old = SystemTime::now() - Duration::from_secs(60 * 60 * 24 * 30);
    fs::File::options()
        .write(true)
        .open(src.join("python.scip"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    let recorded = fs::metadata(src.join("python.scip"))
        .unwrap()
        .modified()
        .unwrap();

    let dst = tmp.path().join("cache/indexes");
    copy_then_remove(&src, &dst).unwrap();

    let moved = fs::metadata(dst.join("python.scip"))
        .unwrap()
        .modified()
        .unwrap();
    let drift = moved
        .duration_since(recorded)
        .or_else(|e| Ok::<_, std::time::SystemTimeError>(e.duration()))
        .unwrap();
    assert!(
        drift < Duration::from_secs(2),
        "mtime not preserved: recorded {recorded:?}, moved {moved:?}"
    );
}

// ── SEC-4: symlinks ───────────────────────────────────────────────────────

/// SEC-4 / AC21 — sources come from a repository. A same-device rename moves
/// the *link*, and every later graph write then follows it outside the cache;
/// a cross-device copy reads through it and the remove step can delete outside
/// the checkout. Refuse, never follow.
#[test]
#[cfg(unix)]
fn move_path_refuses_a_symlinked_source() {
    let tmp = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    write(&outside.path().join("secret"), "not yours");
    let src = tmp.path().join("graph_db");
    std::os::unix::fs::symlink(outside.path(), &src).unwrap();
    let dst = tmp.path().join("cache/graph_db");
    fs::create_dir_all(dst.parent().unwrap()).unwrap();

    let err = move_path(&src, &dst).unwrap_err();

    assert!(
        format!("{err:#}").to_lowercase().contains("symlink"),
        "refusal must say why; got: {err:#}"
    );
    assert!(src.symlink_metadata().is_ok(), "source must be untouched");
    assert!(outside.path().join("secret").exists());
    assert!(!dst.exists());
}

#[test]
#[cfg(unix)]
fn move_path_refuses_a_symlink_nested_inside_a_source_directory() {
    let tmp = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    write(&outside.path().join("secret"), "not yours");
    let src = tmp.path().join("graph_db");
    write(&src.join("data.kz"), "data");
    std::os::unix::fs::symlink(outside.path(), src.join("escape")).unwrap();
    let dst = tmp.path().join("cache/graph_db");

    let err = copy_then_remove(&src, &dst).unwrap_err();

    assert!(format!("{err:#}").to_lowercase().contains("symlink"));
    assert!(
        outside.path().join("secret").exists(),
        "the remove step must never reach through a nested symlink"
    );
    assert!(src.join("data.kz").exists(), "source must stay intact");
}

// ── failure behaviour ─────────────────────────────────────────────────────

/// A migration that clobbers is worse than one that stops. Destination-exists
/// keeps both sides untouched so the caller can record a conflict and continue.
#[test]
fn move_path_refuses_an_existing_destination_and_touches_neither_side() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("blarify.json");
    let dst = tmp.path().join("cache/blarify.json");
    write(&src, "new");
    write(&dst, "existing");

    let err = move_path(&src, &dst).unwrap_err();

    assert!(format!("{err:#}").contains(&dst.display().to_string()));
    assert_eq!(fs::read_to_string(&src).unwrap(), "new");
    assert_eq!(fs::read_to_string(&dst).unwrap(), "existing");
}

/// The source is removed only after the copy verifies. A failure mid-copy must
/// leave the source whole and the partial destination cleaned up.
#[test]
fn copy_then_remove_leaves_the_source_intact_when_the_copy_fails() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("graph_db");
    write(&src.join("data.kz"), "data");
    // A destination whose parent is a regular file cannot be created.
    let blocker = tmp.path().join("blocker");
    write(&blocker, "file, not a directory");
    let dst = blocker.join("graph_db");

    assert!(copy_then_remove(&src, &dst).is_err());

    assert_eq!(fs::read_to_string(src.join("data.kz")).unwrap(), "data");
}

#[test]
fn move_path_errors_when_the_source_does_not_exist() {
    let tmp = TempDir::new().unwrap();

    let err = move_path(&tmp.path().join("missing"), &tmp.path().join("dst")).unwrap_err();

    assert!(format!("{err:#}").contains("missing"));
}

/// The destination's parent is created on demand — callers move into a freshly
/// ensured artifact root whose subdirectories may not exist yet.
#[test]
fn move_path_creates_the_destination_parent() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("index.scip");
    write(&src, "scip");
    let dst = tmp.path().join("a/b/c/index.scip");

    move_path(&src, &dst).unwrap();

    assert_eq!(fs::read_to_string(&dst).unwrap(), "scip");
}
