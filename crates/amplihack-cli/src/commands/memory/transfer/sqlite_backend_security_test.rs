//! TDD tests for S-2a security functions in `transfer/sqlite_backend.rs`.
//!
//! All referenced functions do NOT exist yet. These tests will fail to compile
//! (or fail at runtime) until the implementation is added — that is the
//! intended TDD state.

use crate::commands::memory::transfer::sqlite_backend::{
    resolve_hierarchical_sqlite_path, validate_agent_name,
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
        "validate_agent_name('my-agent') must return Ok, got Err: {:?}",
        result
    );
}

/// An alphanumeric name with hyphens and digits is valid.
#[test]
fn validate_agent_name_accepts_alphanumeric_with_hyphens() {
    let result = validate_agent_name("agent-123");
    assert!(
        result.is_ok(),
        "validate_agent_name('agent-123') must return Ok, got Err: {:?}",
        result
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
        "resolve with storage override must succeed, got Err: {:?}",
        result
    );
    let resolved = result.unwrap();
    // The resolved path must be rooted under the override, not HOME.
    assert!(
        resolved.starts_with(&override_dir),
        "resolved path {:?} must be under override dir {:?}",
        resolved,
        override_dir
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
        "resolve_hierarchical_sqlite_path(None) must succeed, got Err: {:?}",
        result
    );
    let resolved = result.unwrap();
    let expected_prefix = dir.path().join(".amplihack").join("hierarchical_memory");
    assert!(
        resolved.starts_with(&expected_prefix),
        "resolved path {:?} must be under ~/.amplihack/hierarchical_memory/, expected prefix {:?}",
        resolved,
        expected_prefix
    );
}
