//! Issue #869 regression tests: corrupt persisted state must surface an error
//! and must never be silently discarded then overwritten.
//!
//! These live as integration tests (public API only) so the `amplihack-remote`
//! source modules stay within the issue #536 500-line budget while still
//! covering the manager-level wiring around `state_io::read_json_state`.

use amplihack_remote::{Orchestrator, SessionManager, VMPoolManager};

// ---- VM pool (load side) ----

#[test]
fn vm_pool_schema_mismatch_surfaces_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    // Valid outer JSON, but `vm_pool` is the wrong shape (not a map of entries).
    std::fs::write(&path, r#"{"vm_pool": "not-a-map"}"#).unwrap();

    let result = VMPoolManager::new(Some(path), Orchestrator::with_username("tester"));
    assert!(
        result.is_err(),
        "vm_pool schema mismatch must surface an error, not silently forget all tracked VMs"
    );
}

#[test]
fn vm_pool_fully_corrupt_file_surfaces_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, "}{ broken").unwrap();

    let result = VMPoolManager::new(Some(path), Orchestrator::with_username("tester"));
    let err = result.err().expect("corrupt pool state must fail loudly");
    assert!(
        err.to_string().to_lowercase().contains("corrupt"),
        "error must mention 'corrupt' to distinguish from a missing file: {err}"
    );
}

#[test]
fn vm_pool_missing_file_starts_empty_ok() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nope.json");
    let mgr = VMPoolManager::new(Some(path), Orchestrator::with_username("tester")).unwrap();
    assert_eq!(
        mgr.get_pool_status().total_vms,
        0,
        "missing file → empty pool"
    );
}

#[test]
fn vm_pool_empty_file_starts_empty_ok() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, "  \n ").unwrap();
    let mgr = VMPoolManager::new(Some(path), Orchestrator::with_username("tester")).unwrap();
    assert_eq!(
        mgr.get_pool_status().total_vms,
        0,
        "empty/whitespace file is not corruption → empty pool, Ok"
    );
}

#[test]
fn vm_pool_corrupt_file_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let corrupt = r#"{"sessions": {"s1": BROKEN"#;
    std::fs::write(&path, corrupt).unwrap();

    // Loading fails; the on-disk bytes (holding co-resident session state) must
    // be preserved so the user can recover.
    let _ = VMPoolManager::new(Some(path.clone()), Orchestrator::with_username("tester"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        corrupt,
        "corrupt pool file must be preserved on disk, never overwritten"
    );
}

// ---- Sessions (load side) ----

#[test]
fn session_schema_mismatch_surfaces_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, r#"{"sessions": "not-a-map"}"#).unwrap();

    let result = SessionManager::new(Some(path));
    assert!(
        result.is_err(),
        "sessions schema mismatch must surface an error, not silently drop all sessions"
    );
}

#[test]
fn session_fully_corrupt_file_surfaces_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, "}{ broken").unwrap();

    let err = SessionManager::new(Some(path))
        .err()
        .expect("corrupt state must fail loudly");
    assert!(
        err.to_lowercase().contains("corrupt"),
        "error must mention 'corrupt' to distinguish from a missing file: {err}"
    );
}

#[test]
fn session_missing_file_starts_empty_ok() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nope.json");
    let mgr = SessionManager::new(Some(path)).unwrap();
    assert!(
        mgr.list_sessions(None).is_empty(),
        "missing file → empty session set"
    );
}

#[test]
fn session_empty_file_starts_empty_ok() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, "  \n ").unwrap();
    let mgr = SessionManager::new(Some(path)).unwrap();
    assert!(
        mgr.list_sessions(None).is_empty(),
        "empty/whitespace file is not corruption → empty session set, Ok"
    );
}

// ---- Sessions (save side) ----

#[test]
fn session_save_refuses_to_overwrite_corrupt_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    // Loads cleanly from a missing file.
    let mut mgr = SessionManager::new(Some(path.clone())).unwrap();

    // The on-disk file becomes corrupt (e.g. a partial write / co-resident
    // vm_pool state we cannot parse). The next persisting operation must abort
    // rather than merge onto an empty object and wipe it.
    let corrupt = r#"{"vm_pool": {"x": BROKEN"#;
    std::fs::write(&path, corrupt).unwrap();

    let result = mgr.create_session("vm1", "do stuff", None, None, None);
    assert!(
        result.is_err(),
        "a persisting op must not silently discard a corrupt existing file during merge"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        corrupt,
        "corrupt file must be preserved so co-resident vm_pool state is not wiped"
    );
}

#[test]
fn kill_session_returns_false_when_save_fails() {
    let dir = tempfile::tempdir().unwrap();
    let state_file = dir.path().join("state.json");
    let mut mgr = SessionManager::new(Some(state_file.clone())).unwrap();

    let session = mgr
        .create_session("vm1", "do stuff", None, None, None)
        .unwrap();
    mgr.start_session(&session.session_id).unwrap();

    // Force the next save_state() to fail deterministically: replace the
    // advisory lock path with a *directory*. Opening a directory for writing
    // fails regardless of privilege level, so the lock (and thus save) errors.
    let lock_path = state_file.with_extension("lock");
    let _ = std::fs::remove_file(&lock_path);
    std::fs::create_dir_all(&lock_path).unwrap();

    assert!(
        !mgr.kill_session(&session.session_id),
        "kill_session must return false when the underlying save_state fails"
    );
}
