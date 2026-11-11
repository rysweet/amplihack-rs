//! Issue #2279 (C1, SR-1/2/13/16/19): the file functions of `hook_block`.
//!
//! `update_file`, `update_file_with_backup` and `merge_file` read with
//! `read_bounded_nofollow` (1 MiB), refuse symlinks, non-regular files,
//! hard-linked targets and foreign uids, never create a file, write only when
//! the content changes (mode 0600), and give reasons that hold no file bytes.
//! `drift_file` is read-only. Every file lives in a temp directory.

#![cfg(unix)]

use super::*;
use hook_block::{FileDrift, FileMerge, HookTarget};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const BIN: &str = "/opt/amplihack-test/bin/amplihack-hooks";
const SECRET: &str = "sk-test-do-not-print";

fn target() -> HookTarget {
    HookTarget {
        bin: PathBuf::from(BIN),
        limit_resume: true,
    }
}

fn private(path: &Path, body: &[u8]) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn stripped() -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({
        "env": {"XPIA_ENABLED": "false", "SENTINEL_SECRET": SECRET},
        "hooks": {}
    }))
    .unwrap()
}

fn current() -> Vec<u8> {
    let mut s = json!({"env": {"XPIA_ENABLED": "false"}});
    hook_block::merge(&mut s, &target());
    let mut bytes = serde_json::to_vec_pretty(&s).unwrap();
    bytes.push(b'\n');
    bytes
}

fn snapshot(path: &Path) -> (Vec<u8>, i64, i64) {
    let meta = fs::symlink_metadata(path).unwrap();
    (
        fs::read(path).unwrap_or_default(),
        meta.mtime(),
        meta.mtime_nsec(),
    )
}

fn assert_no_content(reason: &str) {
    assert!(!reason.contains('{'), "reason holds JSON: {reason}");
    assert!(!reason.contains(SECRET), "reason holds a secret: {reason}");
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

// ── merge_file ────────────────────────────────────────────────────────────

#[test]
fn merge_file_heals_a_stripped_file_with_mode_0600() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    private(&path, &stripped());
    assert_eq!(hook_block::merge_file(&path, &target()), FileMerge::Written);
    let s: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(hook_block::drift(&s, &target()).is_clean());
    assert_eq!(s["env"]["SENTINEL_SECRET"], json!(SECRET));
    assert_eq!(s["env"]["XPIA_ENABLED"], json!("false"));
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn merge_file_on_a_current_file_is_already_current_and_not_rewritten() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    private(&path, &current());
    let before = snapshot(&path);
    assert_eq!(
        hook_block::merge_file(&path, &target()),
        FileMerge::AlreadyCurrent
    );
    assert_eq!(snapshot(&path), before, "bytes and mtime unchanged");
    assert_eq!(entries(d.path()), vec![path], "no temp or backup left");
}

#[test]
fn merge_file_never_creates_an_absent_file() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    assert_eq!(hook_block::merge_file(&path, &target()), FileMerge::Absent);
    assert!(!path.exists());
}

#[test]
fn a_symlink_is_refused_and_its_target_left_unchanged() {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real.json");
    private(&real, &stripped());
    let link = d.path().join("settings.json");
    symlink(&real, &link).unwrap();
    let before = snapshot(&real);
    let result = hook_block::merge_file(&link, &target());
    let FileMerge::Refused(reason) = result else {
        panic!("expected Refused, got {result:?}");
    };
    assert_no_content(&reason);
    assert_eq!(snapshot(&real), before);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn a_fifo_is_refused_without_blocking() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path; mkfifo has no other preconditions.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let result = hook_block::merge_file(&path, &target());
    assert!(
        matches!(result, FileMerge::Refused(_)),
        "expected Refused, got {result:?}"
    );
}

#[test]
fn a_file_over_one_mebibyte_is_refused_and_unchanged() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    let mut big = b"{\"pad\":\"".to_vec();
    big.extend(std::iter::repeat_n(b'a', 1024 * 1024 + 16));
    big.extend(b"\"}");
    private(&path, &big);
    let before = snapshot(&path);
    let result = hook_block::merge_file(&path, &target());
    assert!(
        matches!(result, FileMerge::Refused(_)),
        "expected Refused, got {result:?}"
    );
    assert_eq!(snapshot(&path), before);
}

#[test]
fn a_hard_linked_target_is_refused_and_both_names_unchanged() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    private(&path, &stripped());
    let other = d.path().join("other-name.json");
    fs::hard_link(&path, &other).unwrap();
    let before = snapshot(&other);
    let result = hook_block::merge_file(&path, &target());
    let FileMerge::Refused(reason) = result else {
        panic!("expected Refused, got {result:?}");
    };
    assert_no_content(&reason);
    assert_eq!(snapshot(&other), before);
}

#[test]
fn a_foreign_uid_is_refused() {
    // The metadata branch: the check compares the file's uid with the euid.
    // A file owned by another user cannot be made without root, so the check
    // is exercised through its pure helper.
    assert!(hook_block::owner_refusal(0, 1000).is_some());
    assert!(hook_block::owner_refusal(1000, 1000).is_none());
    let reason = hook_block::owner_refusal(0, 1000).unwrap();
    assert_no_content(&reason);
}

#[test]
fn invalid_json_is_unparseable_with_no_file_bytes_in_the_reason() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    let body = format!("{{\"env\": {{\"SENTINEL_SECRET\": \"{SECRET}\"}}, oops");
    private(&path, body.as_bytes());
    let before = snapshot(&path);
    let result = hook_block::merge_file(&path, &target());
    let FileMerge::Unparseable(reason) = result else {
        panic!("expected Unparseable, got {result:?}");
    };
    assert_no_content(&reason);
    assert!(
        reason.contains("line") && reason.contains("column"),
        "reason gives a position only: {reason}"
    );
    assert_eq!(snapshot(&path), before);
}

#[test]
fn a_non_object_root_is_unparseable_and_unchanged() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    private(&path, b"[1, 2]\n");
    let result = hook_block::merge_file(&path, &target());
    assert!(
        matches!(result, FileMerge::Unparseable(_)),
        "expected Unparseable, got {result:?}"
    );
    assert_eq!(fs::read(&path).unwrap(), b"[1, 2]\n");
}

// ── update_file ───────────────────────────────────────────────────────────

#[test]
fn update_file_applies_the_closure_and_writes_once() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    private(&path, b"{\"a\": 1}\n");
    let r = hook_block::update_file(&path, &|s: &mut Value| s["b"] = json!(2));
    assert_eq!(r, FileMerge::Written);
    let r = hook_block::update_file(&path, &|s: &mut Value| s["b"] = json!(2));
    assert_eq!(r, FileMerge::AlreadyCurrent);
    let s: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(s, json!({"a": 1, "b": 2}));
}

#[test]
fn update_file_never_creates_a_file() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    let r = hook_block::update_file(&path, &|s: &mut Value| s["b"] = json!(2));
    assert_eq!(r, FileMerge::Absent);
    assert!(!path.exists());
}

// ── update_file_with_backup ───────────────────────────────────────────────

#[test]
fn backup_is_written_only_on_change_from_the_bytes_read_at_mode_0600() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    let original = stripped();
    private(&path, &original);
    let backup = d.path().join("settings.json.backup.1");

    let r = hook_block::update_file_with_backup(&path, &backup, &|s: &mut Value| {
        hook_block::merge(s, &target());
    });
    assert_eq!(r, FileMerge::Written);
    assert_eq!(
        fs::read(&backup).unwrap(),
        original,
        "backup is the old bytes"
    );
    assert_eq!(fs::metadata(&backup).unwrap().mode() & 0o777, 0o600);

    let second = d.path().join("settings.json.backup.2");
    let r = hook_block::update_file_with_backup(&path, &second, &|s: &mut Value| {
        hook_block::merge(s, &target());
    });
    assert_eq!(r, FileMerge::AlreadyCurrent);
    assert!(!second.exists(), "no backup when nothing changes");
}

#[test]
fn backup_never_follows_a_planted_symlink() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    private(&path, &stripped());
    let victim = d.path().join("victim");
    private(&victim, b"keep\n");
    let backup = d.path().join("settings.json.backup.1");
    symlink(&victim, &backup).unwrap();
    let r = hook_block::update_file_with_backup(&path, &backup, &|s: &mut Value| {
        hook_block::merge(s, &target());
    });
    assert!(matches!(r, FileMerge::Refused(_)), "got {r:?}");
    assert_eq!(fs::read(&victim).unwrap(), b"keep\n");
    assert_eq!(
        fs::read(&path).unwrap(),
        stripped(),
        "no write without a backup"
    );
}

// ── drift_file ────────────────────────────────────────────────────────────

#[test]
fn drift_file_classifies_each_state_and_never_writes() {
    let d = tempfile::tempdir().unwrap();

    let absent = d.path().join("absent.json");
    assert_eq!(
        hook_block::drift_file(&absent, &target()),
        FileDrift::Absent
    );
    assert!(!absent.exists());

    let clean = d.path().join("clean.json");
    private(&clean, &current());
    assert_eq!(hook_block::drift_file(&clean, &target()), FileDrift::Clean);

    let stripped_path = d.path().join("stripped.json");
    private(&stripped_path, &stripped());
    let before = snapshot(&stripped_path);
    match hook_block::drift_file(&stripped_path, &target()) {
        FileDrift::Drift(drift) => assert_eq!(drift.missing.len(), 8),
        other => panic!("expected Drift, got {other:?}"),
    }
    assert_eq!(snapshot(&stripped_path), before, "read-only");

    let bad = d.path().join("bad.json");
    private(&bad, b"{ not json");
    assert!(matches!(
        hook_block::drift_file(&bad, &target()),
        FileDrift::Unparseable(_)
    ));

    let link = d.path().join("link.json");
    symlink(&clean, &link).unwrap();
    assert!(matches!(
        hook_block::drift_file(&link, &target()),
        FileDrift::Refused(_)
    ));
}
