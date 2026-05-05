//! Security tests for `transfer/sqlite_backend.rs`.
//!
//! Covers:
//! - `validate_agent_name` — path traversal protection
//! - `resolve_hierarchical_sqlite_path` — path construction
//! - `enforce_hierarchical_db_permissions` — symlink swap attack prevention
//! - Kuzu `resolve_hierarchical_db_path` — validate_agent_name parity

use crate::commands::memory::transfer::sqlite_backend::{
    enforce_hierarchical_db_permissions, resolve_hierarchical_sqlite_path, validate_agent_name,
};
use crate::test_support::home_env_lock;

// ---------------------------------------------------------------------------
// EnvGuard – isolate HOME and AMPLIHACK_MEMORY_BACKEND across tests
// ---------------------------------------------------------------------------

struct EnvGuard {
    prev_home: Option<std::ffi::OsString>,
    #[allow(dead_code)]
    lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn with_home(home: &std::path::Path) -> Self {
        let lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", home);
        }
        Self { prev_home, lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.prev_home.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// validate_agent_name – path traversal protection
// ---------------------------------------------------------------------------

/// An agent name containing ".." must be rejected to prevent path traversal.
#[test]
fn validate_agent_name_rejects_path_traversal() {
    let result = validate_agent_name("../evil");
    assert!(
        result.is_err(),
        "validate_agent_name('../evil') must return Err, got Ok"
    );
}

/// An absolute path as an agent name must be rejected.
#[test]
fn validate_agent_name_rejects_absolute_path() {
    let result = validate_agent_name("/etc/passwd");
    assert!(
        result.is_err(),
        "validate_agent_name('/etc/passwd') must return Err, got Ok"
    );
}

/// An empty string must be rejected.
#[test]
fn validate_agent_name_rejects_empty_string() {
    let result = validate_agent_name("");
    assert!(
        result.is_err(),
        "validate_agent_name('') must return Err, got Ok"
    );
}

/// A name exceeding 255 characters must be rejected.
#[test]
fn validate_agent_name_rejects_too_long_name() {
    let long_name = "a".repeat(256);
    let result = validate_agent_name(&long_name);
    assert!(
        result.is_err(),
        "validate_agent_name(<256-char name>) must return Err, got Ok"
    );
}

/// A plain lowercase name with hyphens is a valid agent name.
#[test]
fn validate_agent_name_accepts_valid_name() {
    let result = validate_agent_name("my-agent");
    assert!(
        result.is_ok(),
        "validate_agent_name('my-agent') must return Ok, got Err: {result:?}"
    );
}

/// An alphanumeric name with hyphens and digits is valid.
#[test]
fn validate_agent_name_accepts_alphanumeric_with_hyphens() {
    let result = validate_agent_name("agent-123");
    assert!(
        result.is_ok(),
        "validate_agent_name('agent-123') must return Ok, got Err: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// resolve_hierarchical_sqlite_path – path construction
// ---------------------------------------------------------------------------

/// Passing an agent name with ".." must be rejected by resolve.
#[test]
fn resolve_hierarchical_sqlite_path_rejects_traversal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let _guard = EnvGuard::with_home(dir.path());

    let result = resolve_hierarchical_sqlite_path("../../sneaky", None);
    assert!(
        result.is_err(),
        "resolve_hierarchical_sqlite_path with traversal agent name must return Err, got Ok"
    );
}

/// When `storage_path` is `Some(path)`, the resolved path must be based on
/// that override, not the default `~/.amplihack/hierarchical_memory/` tree.
#[test]
fn resolve_hierarchical_sqlite_path_uses_storage_override() {
    let dir = tempfile::tempdir().expect("tempdir");
    let override_dir = dir.path().join("custom_storage");
    std::fs::create_dir_all(&override_dir).expect("create override dir");

    let override_str = override_dir.to_string_lossy().into_owned();
    let result = resolve_hierarchical_sqlite_path("my-agent", Some(&override_str));

    assert!(
        result.is_ok(),
        "resolve with storage override must succeed, got Err: {result:?}"
    );
    let resolved = result.unwrap();
    // The resolved path must be rooted under the override, not HOME.
    assert!(
        resolved.starts_with(&override_dir),
        "resolved path {resolved:?} must be under override dir {override_dir:?}"
    );
}

/// When `storage_path` is `None`, the resolved path must fall under
/// `~/.amplihack/hierarchical_memory/<agent_name>`.
#[test]
fn resolve_hierarchical_sqlite_path_defaults_to_home() {
    let dir = tempfile::tempdir().expect("tempdir");
    let _guard = EnvGuard::with_home(dir.path());

    let result = resolve_hierarchical_sqlite_path("my-agent", None);
    assert!(
        result.is_ok(),
        "resolve_hierarchical_sqlite_path(None) must succeed, got Err: {result:?}"
    );
    let resolved = result.unwrap();
    let expected_prefix = dir.path().join(".amplihack").join("hierarchical_memory");
    assert!(
        resolved.starts_with(&expected_prefix),
        "resolved path {resolved:?} must be under ~/.amplihack/hierarchical_memory/, expected prefix {expected_prefix:?}"
    );
}

// ---------------------------------------------------------------------------
// enforce_hierarchical_db_permissions — symlink swap attack prevention
// ---------------------------------------------------------------------------

/// When the database path IS a symlink, `enforce_hierarchical_db_permissions`
/// must return `Err` rather than calling `set_permissions` on the symlink
/// target (which could be an attacker-controlled file).
#[cfg(unix)]
#[test]
fn enforce_permissions_rejects_symlink_at_db_path() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().expect("tempdir");

    // Create a real file that the symlink will point to.
    let real_file = dir.path().join("real.db");
    std::fs::write(&real_file, b"").expect("create real file");

    // Place a symlink where the DB path would be.
    let symlink_path = dir.path().join("agent.db");
    symlink(&real_file, &symlink_path).expect("create symlink");

    let result = enforce_hierarchical_db_permissions(&symlink_path);
    assert!(
        result.is_err(),
        "enforce_hierarchical_db_permissions must return Err when db path is a symlink, got Ok"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("symlink"),
        "error must mention 'symlink'; got: {err_msg}"
    );
}

/// When the database path is a regular file (not a symlink),
/// `enforce_hierarchical_db_permissions` must succeed.
#[cfg(unix)]
#[test]
fn enforce_permissions_accepts_regular_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("agent.db");
    std::fs::write(&db_path, b"").expect("create regular file");

    let result = enforce_hierarchical_db_permissions(&db_path);
    assert!(
        result.is_ok(),
        "enforce_hierarchical_db_permissions must succeed for a regular file, got: {result:?}"
    );
}

/// When the database path does not yet exist (new agent),
/// `enforce_hierarchical_db_permissions` must succeed without error (no-op).
#[cfg(unix)]
#[test]
fn enforce_permissions_accepts_nonexistent_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("new_agent.db");
    // Do NOT create the file — it doesn't exist yet.

    let result = enforce_hierarchical_db_permissions(&db_path);
    assert!(
        result.is_ok(),
        "enforce_hierarchical_db_permissions must succeed for non-existent path, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Kuzu resolve_hierarchical_db_path — validate_agent_name parity
// ---------------------------------------------------------------------------

/// The Kuzu `resolve_hierarchical_db_path` (in transfer.rs) must call
/// `validate_agent_name`, so that path-traversal agent names like `"../evil"`
/// are rejected just as they are in the SQLite backend.
#[test]
fn kuzu_resolve_hierarchical_db_path_rejects_traversal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let _guard = EnvGuard::with_home(dir.path());

    // Use the public transfer API which delegates through the Kuzu backend's
    // resolve_hierarchical_db_path.  An unrecognised format string is
    // intentionally used to trigger the path-resolution code path before any
    // actual DB I/O.  We exercise export so we can call with storage_path
    // pointing to a temp dir — this surfaces the validate_agent_name call.
    use crate::commands::memory::transfer;

    // Call run_export with a traversal agent name and a temp output path.
    // We set AMPLIHACK_MEMORY_BACKEND=kuzu so the legacy compatibility alias
    // still exercises the graph-db export path.
    let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
    unsafe {
        std::env::set_var("AMPLIHACK_MEMORY_BACKEND", "kuzu");
    }

    let output = dir.path().join("out.json").to_string_lossy().into_owned();
    let storage = dir.path().to_string_lossy().into_owned();
    let result = transfer::run_export("../evil", &output, "json", Some(&storage));

    unsafe {
        match prev_backend {
            Some(v) => std::env::set_var("AMPLIHACK_MEMORY_BACKEND", v),
            None => std::env::remove_var("AMPLIHACK_MEMORY_BACKEND"),
        }
    }

    assert!(
        result.is_err(),
        "run_export with traversal agent name '../evil' must return Err (Kuzu path), got Ok"
    );
}
