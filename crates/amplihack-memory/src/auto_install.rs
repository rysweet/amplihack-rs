//! Lazy memory library availability check.
//!
//! Provides `ensure_memory_lib_installed()` which caches whether the
//! memory backend (SQLite / LadybugDB) is available at runtime.

use std::sync::OnceLock;
use tracing::debug;

/// Cached result of the availability check.
static MEMORY_LIB_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Check (once) whether the memory backend library is available.
///
/// The result is cached for the lifetime of the process. Subsequent calls
/// return the cached value without re-checking.
///
/// Currently this checks whether the `sqlite` feature is compiled in and
/// whether a basic SQLite operation succeeds.
pub fn ensure_memory_lib_installed() -> bool {
    *MEMORY_LIB_AVAILABLE.get_or_init(check_backend_available)
}

/// Perform the actual availability probe.
fn check_backend_available() -> bool {
    // Compile-time feature check: if sqlite support is compiled in we
    // consider the backend available.
    #[cfg(feature = "sqlite")]
    {
        debug!("memory backend: sqlite feature enabled");
        match probe_sqlite() {
            true => {
                debug!("memory backend: sqlite probe succeeded");
                true
            }
            false => {
                debug!("memory backend: sqlite probe failed, falling back");
                false
            }
        }
    }

    #[cfg(not(feature = "sqlite"))]
    {
        debug!("memory backend: sqlite feature not compiled in");
        false
    }
}

/// Try to open an in-memory SQLite database as a smoke test.
#[cfg(feature = "sqlite")]
fn probe_sqlite() -> bool {
    match rusqlite::Connection::open_in_memory() {
        Ok(conn) => conn.execute_batch("SELECT 1;").is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_returns_consistent() {
        let first = ensure_memory_lib_installed();
        let second = ensure_memory_lib_installed();
        assert_eq!(first, second, "cached result should be stable");
    }

    #[test]
    fn result_is_cached() {
        // After the first call the OnceLock should be initialised.
        let _ = ensure_memory_lib_installed();
        assert!(MEMORY_LIB_AVAILABLE.get().is_some());
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_probe_succeeds() {
        assert!(probe_sqlite(), "in-memory SQLite should work");
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn backend_available_with_sqlite() {
        assert!(
            check_backend_available(),
            "backend should be available when sqlite feature is on"
        );
    }

    #[test]
    fn ensure_memory_lib_returns_bool() {
        // Just verify the return type — value depends on features.
        let _result: bool = ensure_memory_lib_installed();
    }
}
