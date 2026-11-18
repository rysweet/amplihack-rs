//! Unit tests for `private_dir.rs` (issue #2346, the design's M5).
//!
//! Wire with, at the end of `private_dir.rs`:
//! `#[cfg(test)] #[path = "private_dir_tests.rs"] mod tests;`
//! and register the module in `lib.rs` with `pub(crate) mod private_dir;`.
//!
//! Contract:
//! `pub(crate) fn ensure_private_leaf_dir(dir: &Path) -> std::io::Result<()>`
//! creates `dir` alone, never its parents, with mode 0700 whatever the umask.
//! When anything already exists at `dir` (a folder at any mode, a file, a
//! symbolic link) it returns `Ok` and leaves it exactly as it was: it does
//! not follow, inspect or change it. When the parent is missing it returns
//! `Ok` and creates nothing, so `HOME` is never created. Any other error is
//! returned.
//!
//! Every fixture folder is given its mode explicitly, so these tests give the
//! same answer under umask 0002 and 0022. No test changes the umask.

use super::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn private_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = tmp.path().to_path_buf();
    (tmp, root)
}

/// Octal text, so a failure reads `0775` rather than `509`.
fn mode_of(path: &Path) -> String {
    octal(fs::symlink_metadata(path).unwrap().permissions().mode())
}

fn octal(mode: u32) -> String {
    format!("{:04o}", mode & 0o7777)
}

#[test]
fn a_missing_leaf_is_created_0700() {
    let (_tmp, home) = private_root();
    let leaf = home.join(".amplihack");
    ensure_private_leaf_dir(&leaf).unwrap();
    assert!(leaf.is_dir());
    assert_eq!(mode_of(&leaf), octal(0o700));
}

#[test]
fn an_existing_folder_keeps_its_mode() {
    let (_tmp, home) = private_root();
    for mode in [0o775, 0o755, 0o700, 0o2770] {
        let leaf = home.join(format!("leaf{mode:o}"));
        fs::create_dir(&leaf).unwrap();
        fs::set_permissions(&leaf, fs::Permissions::from_mode(mode)).unwrap();
        ensure_private_leaf_dir(&leaf).unwrap();
        assert_eq!(
            mode_of(&leaf),
            octal(mode),
            "an existing folder is never changed"
        );
    }
}

#[test]
fn an_existing_link_or_file_is_left_alone() {
    let (_tmp, home) = private_root();
    let target = home.join("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o775)).unwrap();
    let link = home.join("link");
    symlink(&target, &link).unwrap();
    ensure_private_leaf_dir(&link).unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(mode_of(&target), octal(0o775), "a link is never followed");

    let file = home.join("file");
    fs::write(&file, b"keep").unwrap();
    ensure_private_leaf_dir(&file).unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"keep");
}

#[test]
fn a_missing_parent_creates_nothing() {
    let (_tmp, root) = private_root();
    let home = root.join("home");
    ensure_private_leaf_dir(&home.join(".amplihack")).unwrap();
    assert!(!home.exists(), "HOME is never created");
}

#[test]
fn other_errors_are_returned() {
    let (_tmp, root) = private_root();
    let file = root.join("file");
    fs::write(&file, b"not a folder").unwrap();
    assert!(
        ensure_private_leaf_dir(&file.join("leaf")).is_err(),
        "a parent that is a file is an error, not a missing parent"
    );
}
