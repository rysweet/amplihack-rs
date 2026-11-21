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
