//! Issue #2019 (race A): `support::amplihack_bin()` hands every caller a
//! private copy of the `amplihack` binary, so a `cargo build` that re-links
//! `target/debug/amplihack` while the suite runs cannot take a binary away
//! from a test that is running it.
//!
//! Contract:
//! - With `AMPLIHACK_TEST_BINARY` set, the path is returned as given and
//!   nothing is copied.
//! - Otherwise `cargo build` runs once, then `amplihack` (and
//!   `amplihack-hooks`, when the build left a regular file) is copied into
//!   a private temporary directory (mode 0700, not under `target/debug`).
//!   Each copy is a regular file of mode 0755; no `.tmp` file is left.
//! - Every call in the process returns the same path.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn overridden() -> Option<PathBuf> {
    std::env::var_os("AMPLIHACK_TEST_BINARY").map(PathBuf::from)
}

/// `target/debug`, where cargo links the binary.
fn target_dir() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent() // deps
        .and_then(Path::parent) // debug
        .unwrap()
        .to_path_buf()
}

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn every_call_returns_the_same_path() {
    assert_eq!(amplihack_bin(), amplihack_bin());
}

#[test]
fn the_override_is_returned_as_given() {
    let Some(p) = overridden() else { return };
    assert_eq!(amplihack_bin(), p);
}

#[test]
fn the_binary_is_a_private_copy_outside_the_target_dir() {
    if overridden().is_some() {
        return;
    }
    let bin = amplihack_bin();
    let dir = bin.parent().unwrap().to_path_buf();
    assert_eq!(bin.file_name().unwrap(), "amplihack");
    assert_ne!(
        fs::canonicalize(&dir).unwrap(),
        fs::canonicalize(target_dir()).unwrap(),
        "a test must not run the binary cargo re-links"
    );
    let meta = fs::symlink_metadata(&bin).unwrap();
    assert!(meta.file_type().is_file(), "a regular file, not a symlink");
    assert_eq!(mode(&bin), 0o755);
    assert_eq!(mode(&dir), 0o700, "the copy's directory is private");
    let built = fs::symlink_metadata(target_dir().join("amplihack")).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert!(
        meta.ino() != built.ino() || meta.dev() != built.dev(),
        "a copy, not a hard link to the linked binary"
    );
}

#[test]
fn the_hooks_binary_is_copied_beside_it_when_built() {
    if overridden().is_some() {
        return;
    }
    let bin = amplihack_bin();
    let built = target_dir().join("amplihack-hooks");
    let built_is_file = fs::symlink_metadata(&built).is_ok_and(|m| m.file_type().is_file());
    let copy = bin.parent().unwrap().join("amplihack-hooks");
    if built_is_file {
        let meta = fs::symlink_metadata(&copy).expect("amplihack-hooks copied beside amplihack");
        assert!(meta.file_type().is_file());
        assert_eq!(mode(&copy), 0o755);
    } else {
        assert!(
            fs::symlink_metadata(&copy).is_err(),
            "nothing is copied for a hooks binary that is not a regular file"
        );
    }
}

#[test]
fn no_temporary_file_is_left_beside_the_copy() {
    if overridden().is_some() {
        return;
    }
    let bin = amplihack_bin();
    let mut names: Vec<String> = fs::read_dir(bin.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(
        names
            .iter()
            .all(|n| n == "amplihack" || n == "amplihack-hooks"),
        "{names:?}"
    );
}

#[test]
fn the_copy_runs() {
    let out = crate::isolated_amplihack::isolated(amplihack_bin())
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// Issue #2932: under cargo-nextest every test is its own process, and each
// one ran `cargo build` and copied the binary. With the run id nextest gives
// them all, the first process prepares the copy and the rest reuse it.
// These tests give `run_copy` a directory they made and a builder that
// counts its calls, so nothing is built and the host's cargo is not used.

use crate::support::amplihack_bin::{run_copy, run_id_is_plain};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

/// A directory standing in for `target/debug`, holding a small `amplihack`
/// and, when asked, an `amplihack-hooks`.
fn fake_built_dir(root: &Path, with_hooks: bool) -> PathBuf {
    let built = root.join("built");
    fs::create_dir_all(&built).unwrap();
    fs::write(built.join("amplihack"), b"#!/bin/sh\nexit 0\n").unwrap();
    if with_hooks {
        fs::write(built.join("amplihack-hooks"), b"#!/bin/sh\nexit 0\n").unwrap();
    }
    built
}

#[test]
fn one_run_builds_and_copies_once_however_many_processes_ask() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("tmp");
    let built = fake_built_dir(root.path(), true);
    let builds = AtomicUsize::new(0);
    let now = SystemTime::now();
    let ask = || {
        run_copy(&base, "run-a", now, || {
            builds.fetch_add(1, Ordering::SeqCst);
            built.clone()
        })
        .unwrap()
    };
    // Eight callers at once stand in for eight test processes of one run.
    let paths: Vec<PathBuf> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..8).map(|_| s.spawn(ask)).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(builds.load(Ordering::SeqCst), 1, "the build ran once");
    let first = &paths[0];
    assert!(
        paths.iter().all(|p| p == first),
        "every caller got one path"
    );
    assert_eq!(
        first,
        &base.join("amplihack-bin-run-run-a").join("amplihack")
    );
    assert_eq!(ask(), *first, "a later caller reuses the copy");
    assert_eq!(builds.load(Ordering::SeqCst), 1, "and builds nothing");
}

#[test]
fn the_run_copy_is_private_complete_and_holds_only_the_binaries() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("tmp");
    let built = fake_built_dir(root.path(), true);
    let bin = run_copy(&base, "run-b", SystemTime::now(), || built.clone()).unwrap();
    let dir = bin.parent().unwrap();
    assert_eq!(mode(dir), 0o700, "the run directory is private");
    for name in ["amplihack", "amplihack-hooks"] {
        let copy = dir.join(name);
        assert!(fs::symlink_metadata(&copy).unwrap().file_type().is_file());
        assert_eq!(mode(&copy), 0o755);
        assert_eq!(
            fs::read(&copy).unwrap(),
            fs::read(built.join(name)).unwrap()
        );
    }
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["amplihack", "amplihack-hooks"],
        "no lock or .tmp file inside"
    );
}

#[test]
fn a_build_without_a_hooks_binary_copies_the_binary_alone() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("tmp");
    let built = fake_built_dir(root.path(), false);
    let bin = run_copy(&base, "run-c", SystemTime::now(), || built.clone()).unwrap();
    assert!(bin.is_file());
    assert!(fs::symlink_metadata(bin.parent().unwrap().join("amplihack-hooks")).is_err());
}

#[test]
fn two_runs_get_separate_copies() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("tmp");
    let built = fake_built_dir(root.path(), false);
    let now = SystemTime::now();
    let a = run_copy(&base, "run-d", now, || built.clone()).unwrap();
    // The second run starts after a rebuild changed the binary.
    fs::write(built.join("amplihack"), b"#!/bin/sh\nexit 3\n").unwrap();
    let b = run_copy(&base, "run-e", now, || built.clone()).unwrap();
    assert_ne!(a, b);
    assert_eq!(
        fs::read(&a).unwrap(),
        b"#!/bin/sh\nexit 0\n",
        "the first run keeps what it copied"
    );
    assert_eq!(
        fs::read(&b).unwrap(),
        b"#!/bin/sh\nexit 3\n",
        "the second run copies what was built for it"
    );
}

#[test]
fn a_later_run_removes_run_directories_more_than_a_day_old_and_keeps_recent_ones() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("tmp");
    let built = fake_built_dir(root.path(), false);
    let start = SystemTime::now();
    let old = run_copy(&base, "run-old", start, || built.clone()).unwrap();
    let other = base.join("kept-by-someone-else");
    fs::create_dir_all(&other).unwrap();
    // An hour later a second run starts: the first run's copy is recent.
    let hour = start + Duration::from_secs(60 * 60);
    run_copy(&base, "run-next", hour, || built.clone()).unwrap();
    assert!(
        old.is_file(),
        "a recent run's copy is kept: that run may still be going"
    );
    // Two days later a third run starts: both earlier copies are stale.
    let later = start + Duration::from_secs(2 * 24 * 60 * 60);
    let fresh = run_copy(&base, "run-new", later, || built.clone()).unwrap();
    assert!(fresh.is_file());
    for stale in ["run-old", "run-next"] {
        assert!(
            fs::symlink_metadata(base.join(format!("amplihack-bin-run-{stale}"))).is_err(),
            "{stale}'s directory is removed"
        );
        assert!(
            fs::symlink_metadata(base.join(format!("amplihack-bin-run-{stale}.lock"))).is_err(),
            "{stale}'s lock file is removed"
        );
    }
    assert!(
        other.is_dir(),
        "a directory that is not a run copy is left alone"
    );
}

#[test]
fn only_a_plain_run_id_names_a_directory() {
    for plain in ["0c8f2e1a-7b3d-4f6a-9c1e-2d5b8a7f4e30", "run-1", "A"] {
        assert!(run_id_is_plain(plain), "{plain:?}");
    }
    let long = "a".repeat(65);
    for odd in ["", "..", "a/b", "a b", "a\n", "é", long.as_str()] {
        assert!(!run_id_is_plain(odd), "{odd:?}");
    }
}

#[test]
fn under_a_test_run_with_an_id_the_binary_is_that_runs_copy() {
    if overridden().is_some() {
        return;
    }
    let Ok(id) = std::env::var("NEXTEST_RUN_ID") else {
        return;
    };
    if !run_id_is_plain(&id) {
        return;
    }
    let bin = amplihack_bin();
    assert_eq!(
        bin.parent().unwrap().file_name().unwrap().to_string_lossy(),
        format!("amplihack-bin-run-{id}")
    );
}

#[test]
fn a_run_directory_open_to_other_users_or_replaced_by_a_link_is_not_used() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("tmp");
    let built = fake_built_dir(root.path(), false);
    let now = SystemTime::now();
    // A complete copy whose directory has since been opened to others.
    let bin = run_copy(&base, "run-f", now, || built.clone()).unwrap();
    let dir = bin.parent().unwrap().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let err = run_copy(&base, "run-f", now, || built.clone()).unwrap_err();
    assert!(err.to_string().contains("open to other users"), "{err}");
    // A link where the run directory should be, pointing at a directory
    // that holds an `amplihack`.
    let elsewhere = root.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("amplihack"), b"not the build").unwrap();
    std::os::unix::fs::symlink(&elsewhere, base.join("amplihack-bin-run-run-g")).unwrap();
    let err = run_copy(&base, "run-g", now, || built.clone()).unwrap_err();
    assert!(err.to_string().contains("is not a directory"), "{err}");
}

#[test]
fn a_run_directory_left_half_made_is_finished_by_the_next_process() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("tmp");
    let built = fake_built_dir(root.path(), true);
    // What a first process stopped part-way leaves: the directory, the
    // hooks binary and a temporary file, but no `amplihack`.
    let dir = base.join("amplihack-bin-run-run-h");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(dir.join("amplihack-hooks"), b"old").unwrap();
    fs::write(dir.join(".amplihack.tmp"), b"partial").unwrap();
    let builds = AtomicUsize::new(0);
    let bin = run_copy(&base, "run-h", SystemTime::now(), || {
        builds.fetch_add(1, Ordering::SeqCst);
        built.clone()
    })
    .unwrap();
    assert_eq!(
        builds.load(Ordering::SeqCst),
        1,
        "the next process builds, since no copy was complete"
    );
    assert_eq!(
        fs::read(&bin).unwrap(),
        fs::read(built.join("amplihack")).unwrap()
    );
    assert_eq!(
        fs::read(dir.join("amplihack-hooks")).unwrap(),
        fs::read(built.join("amplihack-hooks")).unwrap(),
        "the hooks binary is the one built with it"
    );
    assert!(
        fs::symlink_metadata(dir.join(".amplihack.tmp")).is_err(),
        "no temporary file is left"
    );
}
