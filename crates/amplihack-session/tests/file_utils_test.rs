//! Integration tests for `amplihack-session::file_utils` (TDD: failing).
//!
//! Ports `tests/test_file_utils.py` to Rust and adds Rust-only invariants
//! (UTF-8 atomicity, OOM cap, path-escape rejection).

use amplihack_session::{
    BatchFileOperations, ChecksumAlgorithm, MAX_JSON_FILE_BYTES, SessionError, cleanup_temp_files,
    get_file_checksum, safe_copy_file, safe_move_file, safe_read_file, safe_read_json,
    safe_write_file, safe_write_json,
};
use serde_json::json;
use std::fs;
use std::io::Write;
use tempfile::TempDir;

fn td() -> TempDir {
    tempfile::tempdir_in(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).unwrap()
}

// ---------- safe_read_file / safe_write_file ----------

#[test]
fn write_then_read_roundtrip_utf8() {
    let dir = td();
    let p = dir.path().join("hello.txt");
    safe_write_file(&p, "héllo 🌍\n", true, false, true).unwrap();
    let got = safe_read_file(&p).unwrap().expect("file present");
    assert_eq!(got, "héllo 🌍\n");
}

#[test]
fn read_missing_returns_none() {
    let dir = td();
    let p = dir.path().join("nope.txt");
    let got = safe_read_file(&p).unwrap();
    assert!(got.is_none(), "missing file should return Ok(None)");
}

#[test]
fn write_with_backup_creates_backup_file() {
    let dir = td();
    let p = dir.path().join("data.txt");
    safe_write_file(&p, "v1", true, false, true).unwrap();
    safe_write_file(&p, "v2", true, true, true).unwrap();
    let backup = p.with_extension("txt.backup");
    assert!(
        backup.exists(),
        "backup file should exist after backup=true"
    );
    assert_eq!(fs::read_to_string(&backup).unwrap(), "v1");
    assert_eq!(fs::read_to_string(&p).unwrap(), "v2");
}

#[test]
fn write_atomic_does_not_leave_temp_files() {
    let dir = td();
    let p = dir.path().join("atomic.txt");
    safe_write_file(&p, "atomic-content", true, false, true).unwrap();
    // No leftover .tmp files in the parent directory.
    let strays: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(
        strays.is_empty(),
        "atomic write must clean up its temp file"
    );
}

// ---------- safe_read_json / safe_write_json ----------

#[test]
fn json_roundtrip_preserves_value() {
    let dir = td();
    let p = dir.path().join("data.json");
    let data = json!({"name": "test", "count": 42, "items": [1, 2, 3]});
    safe_write_json(&p, &data).unwrap();
    let got: serde_json::Value = safe_read_json(&p, json!(null)).unwrap();
    assert_eq!(got, data);
}

#[test]
fn json_read_missing_returns_default() {
    let dir = td();
    let p = dir.path().join("missing.json");
    let got: serde_json::Value = safe_read_json(&p, json!({"default": true})).unwrap();
    assert_eq!(got, json!({"default": true}));
}

#[test]
fn json_read_invalid_returns_default() {
    let dir = td();
    let p = dir.path().join("bad.json");
    fs::write(&p, "{not valid json").unwrap();
    let got: serde_json::Value = safe_read_json(&p, json!({"fallback": 1})).unwrap();
    assert_eq!(got, json!({"fallback": 1}));
}

#[test]
fn json_read_rejects_oversize_file() {
    let dir = td();
    let p = dir.path().join("huge.json");
    // Create a sparse file just over the cap.
    let f = fs::File::create(&p).unwrap();
    f.set_len(MAX_JSON_FILE_BYTES + 1).unwrap();
    let err = safe_read_json::<serde_json::Value>(&p, json!(null)).unwrap_err();
    matches!(err, SessionError::TooLarge { .. });
}

// ---------- checksum / copy / move ----------

#[test]
fn checksum_md5_matches_known_value() {
    let dir = td();
    let p = dir.path().join("known.bin");
    fs::write(&p, b"hello").unwrap();
    let got = get_file_checksum(&p, ChecksumAlgorithm::Md5).unwrap();
    assert_eq!(got, "5d41402abc4b2a76b9719d911017c592");
}

#[test]
fn checksum_sha256_matches_known_value() {
    let dir = td();
    let p = dir.path().join("known.bin");
    fs::write(&p, b"hello").unwrap();
    let got = get_file_checksum(&p, ChecksumAlgorithm::Sha256).unwrap();
    assert_eq!(
        got,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[test]
fn copy_file_then_verify_matches() {
    let dir = td();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    fs::write(&src, "payload").unwrap();
    safe_copy_file(&src, &dst, true).unwrap();
    assert_eq!(fs::read_to_string(&dst).unwrap(), "payload");
    assert!(src.exists());
}

#[test]
fn move_file_removes_source() {
    let dir = td();
    let src = dir.path().join("from.txt");
    let dst = dir.path().join("to.txt");
    fs::write(&src, "moveme").unwrap();
    safe_move_file(&src, &dst, true).unwrap();
    assert!(!src.exists(), "source must be gone after move");
    assert_eq!(fs::read_to_string(&dst).unwrap(), "moveme");
}

// ---------- cleanup_temp_files ----------

#[test]
fn cleanup_temp_files_removes_only_old_matching_files() {
    let dir = td();
    let old = dir.path().join("old.tmp");
    let young = dir.path().join("young.tmp");
    let keep = dir.path().join("keep.log");
    fs::write(&old, "x").unwrap();
    fs::write(&young, "x").unwrap();
    fs::write(&keep, "x").unwrap();

    // Backdate `old` to 48 hours ago.
    let two_days = std::time::SystemTime::now() - std::time::Duration::from_secs(48 * 3600);
    filetime_set(&old, two_days);

    let n = cleanup_temp_files(dir.path(), 24.0, "*.tmp").unwrap();
    assert_eq!(n, 1, "only the 48h-old *.tmp file should be cleaned");
    assert!(!old.exists());
    assert!(young.exists(), "fresh *.tmp survives");
    assert!(keep.exists(), "non-matching pattern survives");
}

fn filetime_set(p: &std::path::Path, t: std::time::SystemTime) {
    let ft = filetime::FileTime::from_system_time(t);
    filetime::set_file_mtime(p, ft).expect("set mtime");
}

// ---------- BatchFileOperations + path-escape ----------

#[test]
fn batch_write_executes_all_operations() {
    let dir = td();
    let mut batch = BatchFileOperations::new(dir.path(), true);
    batch.add_write("a.txt", "A").unwrap();
    batch.add_write("b.txt", "B").unwrap();
    let results = batch.execute();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.is_ok()));
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "A");
    assert_eq!(fs::read_to_string(dir.path().join("b.txt")).unwrap(), "B");
}

#[test]
fn batch_rejects_dotdot_path() {
    let dir = td();
    let mut batch = BatchFileOperations::new(dir.path(), true);
    let err = batch.add_write("../escape.txt", "x").unwrap_err();
    matches!(err, SessionError::PathEscape(_));
}

#[test]
fn batch_rejects_absolute_path() {
    let dir = td();
    let mut batch = BatchFileOperations::new(dir.path(), true);
    let err = batch.add_write("/etc/passwd", "x").unwrap_err();
    matches!(err, SessionError::PathEscape(_));
}

#[test]
fn batch_len_tracks_queued_ops() {
    let dir = td();
    let mut batch = BatchFileOperations::new(dir.path(), false);
    assert!(batch.is_empty());
    batch.add_write("a.txt", "A").unwrap();
    batch.add_write("b.txt", "B").unwrap();
    assert_eq!(batch.len(), 2);
}

// suppress unused-warning for helper
#[allow(dead_code)]
fn _force_link(_w: &mut dyn Write) {}
