//! Tests for `SQLITE_TREE_BACKEND_NAME` constant.

use super::MemoryTreeBackend;
use super::sqlite::{SQLITE_TREE_BACKEND_NAME, SqliteBackend};

/// The constant itself must equal "sqlite", not "unknown".
#[test]
fn sqlite_tree_backend_name_constant_is_sqlite() {
    assert_eq!(
        SQLITE_TREE_BACKEND_NAME, "sqlite",
        "SQLITE_TREE_BACKEND_NAME must be 'sqlite', not '{SQLITE_TREE_BACKEND_NAME}'"
    );
}

/// `SqliteBackend::backend_name()` must return "sqlite".
///
/// This test exercises the trait method rather than the raw constant so that
/// a future refactor that wires the constant differently is also caught.
#[test]
fn sqlite_tree_backend_name_is_sqlite() {
    // We need a valid HOME so that open() can locate the memory.db path.
    let dir = tempfile::tempdir().expect("tempdir");
    let _guard = {
        let lock = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        // Return a struct that restores HOME on drop.
        struct Guard {
            prev: Option<std::ffi::OsString>,
            #[allow(dead_code)]
            lock: std::sync::MutexGuard<'static, ()>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                unsafe {
                    match self.prev.take() {
                        Some(v) => std::env::set_var("HOME", v),
                        None => std::env::remove_var("HOME"),
                    }
                }
            }
        }
        Guard {
            prev: prev_home,
            lock,
        }
    };

    let backend = SqliteBackend::open().expect("SqliteBackend::open must succeed with valid HOME");
    assert_eq!(
        backend.backend_name(),
        "sqlite",
        "SqliteBackend::backend_name() must return 'sqlite'"
    );
}
