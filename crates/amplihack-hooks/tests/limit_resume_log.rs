//! Issue #1932 (D1): `log::append_existing`, the append-only writer used
//! while auto-switch is off. It never creates the lock, `resume.log` or the
//! rotated file, never renames, and drops a line that would pass
//! `LOG_MAX_BYTES`. `log::append` keeps its creating, rotating behaviour.
//!
//! Every test works in its own temporary directory; none touches the real
//! home, `~/.claude`, `~/.amplihack` or a profile root.

use amplihack_hooks::limit_resume::log::{LOG_MAX_BYTES, append, append_existing};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const LINE: &str = "2026-11-10T00:00:00Z resume stage=receipt disposition=received\n";

fn private_resume_dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let dir = tmp.path().join("resume");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    (tmp, dir)
}

fn private_file(p: &Path, bytes: &[u8]) {
    fs::write(p, bytes).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn append_existing_creates_nothing_in_an_empty_resume_dir() {
    let (_tmp, dir) = private_resume_dir();
    append_existing(&dir, LINE);
    assert!(entries(&dir).is_empty(), "{:?}", entries(&dir));
}

#[test]
fn append_existing_without_a_log_leaves_only_the_lock() {
    let (_tmp, dir) = private_resume_dir();
    private_file(&dir.join("lock"), b"");
    append_existing(&dir, LINE);
    assert_eq!(entries(&dir), vec!["lock"]);
}

#[test]
fn append_existing_without_a_lock_creates_no_lock_and_writes_nothing() {
    let (_tmp, dir) = private_resume_dir();
    private_file(&dir.join("resume.log"), b"before\n");
    append_existing(&dir, LINE);
    assert_eq!(entries(&dir), vec!["resume.log"]);
    assert_eq!(fs::read(dir.join("resume.log")).unwrap(), b"before\n");
}

#[test]
fn append_existing_appends_to_an_existing_log_under_an_existing_lock() {
    let (_tmp, dir) = private_resume_dir();
    private_file(&dir.join("lock"), b"");
    private_file(&dir.join("resume.log"), b"before\n");
    append_existing(&dir, LINE);
    append_existing(&dir, LINE);
    assert_eq!(
        fs::read_to_string(dir.join("resume.log")).unwrap(),
        format!("before\n{LINE}{LINE}")
    );
    assert_eq!(entries(&dir), vec!["lock", "resume.log"]);
}

#[test]
fn append_existing_drops_a_line_past_the_limit_and_never_rotates() {
    let (_tmp, dir) = private_resume_dir();
    private_file(&dir.join("lock"), b"");
    let full = vec![b'x'; LOG_MAX_BYTES as usize];
    private_file(&dir.join("resume.log"), &full);
    append_existing(&dir, LINE);
    assert_eq!(fs::read(dir.join("resume.log")).unwrap(), full);
    assert!(!dir.join("resume.log.1").exists());
    assert_eq!(entries(&dir), vec!["lock", "resume.log"]);
}

#[test]
fn append_existing_refuses_a_symlinked_log() {
    let (tmp, dir) = private_resume_dir();
    private_file(&dir.join("lock"), b"");
    let target = tmp.path().join("elsewhere");
    private_file(&target, b"untouched\n");
    std::os::unix::fs::symlink(&target, dir.join("resume.log")).unwrap();
    append_existing(&dir, LINE);
    assert_eq!(fs::read(&target).unwrap(), b"untouched\n");
}

#[test]
fn append_existing_refuses_a_group_writable_dir() {
    let (_tmp, dir) = private_resume_dir();
    private_file(&dir.join("lock"), b"");
    private_file(&dir.join("resume.log"), b"");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o770)).unwrap();
    append_existing(&dir, LINE);
    assert_eq!(fs::read(dir.join("resume.log")).unwrap(), b"");
}

#[test]
fn append_existing_on_a_missing_dir_creates_nothing() {
    let (tmp, dir) = private_resume_dir();
    fs::remove_dir(&dir).unwrap();
    append_existing(&dir, LINE);
    assert!(!dir.exists());
    assert!(entries(tmp.path()).is_empty());
}

#[test]
fn append_still_creates_the_lock_and_log() {
    let (_tmp, dir) = private_resume_dir();
    append(&dir, LINE);
    assert_eq!(entries(&dir), vec!["lock", "resume.log"]);
    assert_eq!(fs::read_to_string(dir.join("resume.log")).unwrap(), LINE);
}

#[test]
fn append_still_rotates_past_the_limit() {
    let (_tmp, dir) = private_resume_dir();
    let full = vec![b'x'; LOG_MAX_BYTES as usize];
    private_file(&dir.join("resume.log"), &full);
    append(&dir, LINE);
    assert_eq!(fs::read(dir.join("resume.log.1")).unwrap(), full);
    assert_eq!(fs::read_to_string(dir.join("resume.log")).unwrap(), LINE);
}
