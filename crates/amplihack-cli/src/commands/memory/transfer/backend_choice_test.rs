//! TDD tests for S-2c: `resolve_transfer_backend_choice()` in `transfer.rs`.
//!
//! The function does NOT exist yet. These tests will fail to compile until
//! S-2c is implemented.

use crate::commands::memory::BackendChoice;
use crate::commands::memory::transfer::resolve_transfer_backend_choice;
use crate::test_support::home_env_lock;

// ---------------------------------------------------------------------------
// EnvGuard – isolates AMPLIHACK_MEMORY_BACKEND env var
// ---------------------------------------------------------------------------

struct BackendEnvGuard {
    prev: Option<std::ffi::OsString>,
    #[allow(dead_code)]
    lock: std::sync::MutexGuard<'static, ()>,
}

impl BackendEnvGuard {
    fn set(value: &str) -> Self {
        let lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        unsafe {
            std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value);
        }
        Self { prev, lock }
    }

    fn unset() -> Self {
        let lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        unsafe {
            std::env::remove_var("AMPLIHACK_MEMORY_BACKEND");
        }
        Self { prev, lock }
    }
}

impl Drop for BackendEnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var("AMPLIHACK_MEMORY_BACKEND", v),
                None => std::env::remove_var("AMPLIHACK_MEMORY_BACKEND"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// `AMPLIHACK_MEMORY_BACKEND=sqlite` must resolve to `BackendChoice::Sqlite`.
#[test]
fn resolve_transfer_backend_choice_sqlite_from_env() {
    let _guard = BackendEnvGuard::set("sqlite");
    let choice = resolve_transfer_backend_choice();
    assert_eq!(
        choice,
        BackendChoice::Sqlite,
        "AMPLIHACK_MEMORY_BACKEND=sqlite must resolve to BackendChoice::Sqlite"
    );
}

/// `AMPLIHACK_MEMORY_BACKEND=kuzu` must resolve to `BackendChoice::GraphDb`.
#[test]
fn resolve_transfer_backend_choice_kuzu_from_env() {
    let _guard = BackendEnvGuard::set("kuzu");
    let choice = resolve_transfer_backend_choice();
    assert_eq!(
        choice,
        BackendChoice::GraphDb,
        "AMPLIHACK_MEMORY_BACKEND=kuzu must resolve to BackendChoice::GraphDb"
    );
}

/// `AMPLIHACK_MEMORY_BACKEND=graph-db` is a documented alias for Kuzu.
#[test]
fn resolve_transfer_backend_choice_graph_db_alias_from_env() {
    let _guard = BackendEnvGuard::set("graph-db");
    let choice = resolve_transfer_backend_choice();
    assert_eq!(
        choice,
        BackendChoice::GraphDb,
        "AMPLIHACK_MEMORY_BACKEND=graph-db must resolve to BackendChoice::GraphDb"
    );
}

/// An unrecognized value must default to the public `graph-db` backend.
#[test]
fn resolve_transfer_backend_choice_unknown_warns_and_defaults_graph_db_backend() {
    let _guard = BackendEnvGuard::set("ladybug-db-not-real");
    let choice = resolve_transfer_backend_choice();
    assert_eq!(
        choice,
        BackendChoice::GraphDb,
        "unknown AMPLIHACK_MEMORY_BACKEND value must default to the graph-db backend"
    );
}

/// When `AMPLIHACK_MEMORY_BACKEND` is not set, the default must be the public
/// `graph-db` backend.
#[test]
fn resolve_transfer_backend_choice_no_env_defaults_graph_db_backend() {
    let _guard = BackendEnvGuard::unset();
    let choice = resolve_transfer_backend_choice();
    assert_eq!(
        choice,
        BackendChoice::GraphDb,
        "absent AMPLIHACK_MEMORY_BACKEND must default to the graph-db backend"
    );
}
