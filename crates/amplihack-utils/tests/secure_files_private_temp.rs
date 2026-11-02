//! Issue #1844 (SR-10): `PrivateTemp`, a private temporary file opened
//! beside its target before the bytes are known.
//!
//! A token refresh must know that it can save the new login before it sends
//! the refresh request, because a refresh may retire the old refresh token.
//! `PrivateTemp::create` makes the temporary file first; `commit` writes,
//! syncs, renames it over the target and syncs the directory.
//!
//! Contract:
//! - `PrivateTemp::create(dir: &Path, name: &str) -> io::Result<PrivateTemp>`
//!   refuses a target that is a symlink, not a regular file or owned by
//!   someone else, as `write_private_atomic` does, and creates a new file in
//!   `dir` with mode `0600`, without following symlinks.
//! - `PrivateTemp::path(&self) -> &Path` is the temporary file.
//! - `PrivateTemp::commit(self, bytes: &[u8]) -> io::Result<()>` leaves
//!   `dir/name` holding `bytes` with mode `0600` and removes nothing else.
//! - Dropping a `PrivateTemp` that was not committed removes its file.
//! - `write_private_atomic` keeps its behaviour.

#![cfg(unix)]

use amplihack_utils::secure_files::{PrivateTemp, write_private_atomic};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

fn private_dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}

fn entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn create_makes_a_private_file_in_the_same_directory() {
    let d = private_dir();
    let t = PrivateTemp::create(d.path(), "login.json").unwrap();
    assert_eq!(t.path().parent(), Some(d.path()));
    assert!(t.path().is_file());
    assert_eq!(mode(t.path()), 0o600);
    assert_ne!(t.path(), d.path().join("login.json"));
}

#[test]
fn commit_replaces_the_target_with_a_private_file() {
    let d = private_dir();
    let target = d.path().join("login.json");
    fs::write(&target, b"old").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    let t = PrivateTemp::create(d.path(), "login.json").unwrap();
    t.commit(b"new").unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"new");
    assert_eq!(mode(&target), 0o600);
    assert_eq!(entries(d.path()), ["login.json"], "no temporary file left");
}

#[test]
fn commit_creates_a_missing_target() {
    let d = private_dir();
    PrivateTemp::create(d.path(), "fresh.json")
        .unwrap()
        .commit(b"{}")
        .unwrap();
    assert_eq!(fs::read(d.path().join("fresh.json")).unwrap(), b"{}");
    assert_eq!(entries(d.path()), ["fresh.json"]);
}

#[test]
fn dropping_without_commit_removes_the_temporary_file() {
    let d = private_dir();
    fs::write(d.path().join("login.json"), b"keep").unwrap();
    let t = PrivateTemp::create(d.path(), "login.json").unwrap();
    let tmp = t.path().to_path_buf();
    assert!(tmp.exists());
    drop(t);
    assert!(!tmp.exists());
    assert_eq!(entries(d.path()), ["login.json"]);
    assert_eq!(fs::read(d.path().join("login.json")).unwrap(), b"keep");
}

#[test]
fn create_refuses_a_symlinked_target() {
    let d = private_dir();
    fs::write(d.path().join("real"), b"x").unwrap();
    symlink(d.path().join("real"), d.path().join("login.json")).unwrap();
    assert!(PrivateTemp::create(d.path(), "login.json").is_err());
    assert_eq!(entries(d.path()), ["login.json", "real"]);
}

#[test]
fn create_refuses_a_directory_target() {
    let d = private_dir();
    fs::create_dir(d.path().join("login.json")).unwrap();
    assert!(PrivateTemp::create(d.path(), "login.json").is_err());
}

#[test]
fn create_fails_in_a_read_only_directory_and_leaves_nothing() {
    let d = private_dir();
    fs::write(d.path().join("login.json"), b"keep").unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o500)).unwrap();
    // Root ignores the mode bits; the check means nothing there.
    let writable = fs::write(d.path().join("probe"), b"").is_ok();
    let result = PrivateTemp::create(d.path(), "login.json");
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    if writable {
        return;
    }
    assert!(result.is_err());
    assert_eq!(entries(d.path()), ["login.json"]);
}

#[test]
fn write_private_atomic_keeps_its_behaviour() {
    let d = private_dir();
    write_private_atomic(d.path(), "a.json", b"one").unwrap();
    write_private_atomic(d.path(), "a.json", b"two").unwrap();
    assert_eq!(fs::read(d.path().join("a.json")).unwrap(), b"two");
    assert_eq!(mode(&d.path().join("a.json")), 0o600);
    assert_eq!(entries(d.path()), ["a.json"]);
    symlink(d.path().join("a.json"), d.path().join("b.json")).unwrap();
    assert!(write_private_atomic(d.path(), "b.json", b"x").is_err());
}
