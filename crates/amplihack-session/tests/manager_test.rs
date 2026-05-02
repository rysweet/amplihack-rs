//! Integration tests for `amplihack-session::manager` (TDD: failing).
//!
//! Ports `tests/test_session_manager.py` to Rust.

use amplihack_session::{SessionConfig, SessionError, SessionManager};
use serde_json::json;
use std::path::PathBuf;
use tempfile::TempDir;

fn td() -> TempDir {
    tempfile::tempdir_in(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).unwrap()
}

#[test]
fn new_creates_runtime_dir() {
    let dir = td();
    let runtime: PathBuf = dir.path().join("sessions");
    assert!(!runtime.exists());
    let _mgr = SessionManager::new(&runtime).unwrap();
    assert!(runtime.is_dir(), "runtime_dir must be created");
}

#[test]
fn create_session_returns_unique_id() {
    let dir = td();
    let mut mgr = SessionManager::new(dir.path()).unwrap();
    let a = mgr.create_session("task-a", None, None).unwrap();
    let b = mgr.create_session("task-b", None, None).unwrap();
    assert_ne!(a, b);
    assert_eq!(mgr.active_count(), 2);
}

#[test]
fn save_then_resume_roundtrips_session() {
    let dir = td();
    let mut mgr = SessionManager::new(dir.path()).unwrap();
    let id = mgr.create_session("persisted", None, None).unwrap();

    // Mutate so we can detect roundtrip.
    {
        let s = mgr.get_session(&id).unwrap();
        s.start();
        let _ = s.execute_command("touch", None, json!({})).unwrap();
    }
    assert!(mgr.save_session(&id, true).unwrap());
    assert!(mgr.session_file_path(&id).exists());

    // Throw away in-memory state by dropping mgr; reload from disk.
    drop(mgr);
    let mut mgr2 = SessionManager::new(dir.path()).unwrap();
    let resumed = mgr2.resume_session(&id).unwrap().expect("resumed session");
    assert_eq!(resumed.state.command_count, 1);
}

#[test]
fn save_session_returns_false_for_unknown_id() {
    let dir = td();
    let mut mgr = SessionManager::new(dir.path()).unwrap();
    assert!(!mgr.save_session("does-not-exist", true).unwrap());
}

#[test]
fn list_sessions_includes_active_and_saved() {
    let dir = td();
    let mut mgr = SessionManager::new(dir.path()).unwrap();
    let _id1 = mgr.create_session("active-only", None, None).unwrap();
    let id2 = mgr.create_session("will-be-saved", None, None).unwrap();
    mgr.save_session(&id2, true).unwrap();
    let listed = mgr.list_sessions(false, true).unwrap();
    assert!(listed.len() >= 2);
}

#[test]
fn list_sessions_active_only_excludes_disk() {
    let dir = td();
    let mut mgr = SessionManager::new(dir.path()).unwrap();
    let id = mgr.create_session("foo", None, None).unwrap();
    mgr.save_session(&id, true).unwrap();
    drop(mgr);
    let mgr2 = SessionManager::new(dir.path()).unwrap();
    let listed = mgr2.list_sessions(true, false).unwrap();
    assert!(
        listed.is_empty(),
        "active_only=true must omit disk-only sessions"
    );
}

#[test]
fn archive_session_moves_file_to_archive_dir() {
    let dir = td();
    let mut mgr = SessionManager::new(dir.path()).unwrap();
    let id = mgr.create_session("archivable", None, None).unwrap();
    mgr.save_session(&id, true).unwrap();
    let session_file = mgr.session_file_path(&id);
    assert!(session_file.exists());
    assert!(mgr.archive_session(&id).unwrap());
    assert!(
        !session_file.exists(),
        "session file must move out of runtime"
    );
    let archive_dir = dir.path().join("archive");
    assert!(archive_dir.is_dir());
    let entries: Vec<_> = std::fs::read_dir(&archive_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(!entries.is_empty(), "archive dir should contain the file");
}

#[test]
fn cleanup_old_sessions_archives_only_old_files() {
    let dir = td();
    let mut mgr = SessionManager::new(dir.path()).unwrap();
    let young = mgr.create_session("young", None, None).unwrap();
    let old = mgr.create_session("old", None, None).unwrap();
    mgr.save_session(&young, true).unwrap();
    mgr.save_session(&old, true).unwrap();

    // Backdate old session file by 60 days; cutoff is 30 days.
    let path = mgr.session_file_path(&old);
    let sixty_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 86400);
    set_mtime(&path, sixty_days_ago);

    let n = mgr.cleanup_old_sessions(30).unwrap();
    assert_eq!(n, 1, "only the 60-day-old session should be archived");
    assert!(mgr.session_file_path(&young).exists());
    assert!(!mgr.session_file_path(&old).exists());
}

fn set_mtime(p: &std::path::Path, t: std::time::SystemTime) {
    let ft = filetime::FileTime::from_system_time(t);
    filetime::set_file_mtime(p, ft).expect("set mtime");
}

#[test]
fn validate_session_id_accepts_valid_charset() {
    SessionManager::validate_session_id("abc_DEF-123").unwrap();
}

#[test]
fn validate_session_id_rejects_path_traversal() {
    let err = SessionManager::validate_session_id("../etc/passwd").unwrap_err();
    matches!(err, SessionError::InvalidSessionId(_));
}

#[test]
fn validate_session_id_rejects_empty_and_too_long() {
    matches!(
        SessionManager::validate_session_id("").unwrap_err(),
        SessionError::InvalidSessionId(_)
    );
    let too_long = "a".repeat(129);
    matches!(
        SessionManager::validate_session_id(&too_long).unwrap_err(),
        SessionError::InvalidSessionId(_)
    );
}

#[test]
fn save_all_active_persists_every_session() {
    let dir = td();
    let mut mgr = SessionManager::new(dir.path()).unwrap();
    let a = mgr.create_session("a", None, None).unwrap();
    let b = mgr.create_session("b", None, None).unwrap();
    mgr.save_all_active().unwrap();
    assert!(mgr.session_file_path(&a).exists());
    assert!(mgr.session_file_path(&b).exists());
    assert!(mgr.registry_path().exists());
}

#[test]
fn create_with_explicit_config_propagates_id() {
    let dir = td();
    let mut mgr = SessionManager::new(dir.path()).unwrap();
    let cfg = SessionConfig {
        session_id: Some("fixed-id-007".into()),
        ..SessionConfig::default()
    };
    let id = mgr.create_session("named", Some(cfg), None).unwrap();
    assert_eq!(id, "fixed-id-007");
}
