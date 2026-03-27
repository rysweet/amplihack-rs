//! Shared test utilities for amplihack-hooks unit tests.
//!
//! Provides a process-wide environment lock (`env_lock`) that must be held
//! by any test that reads or writes process environment variables (HOME, PATH,
//! AMPLIHACK_*, etc.) or mutates/depends on the process current working
//! directory. Using a single shared lock across all test modules in this binary
//! prevents races when cargo runs tests in parallel.
//!
//! # Usage
//!
//! ```rust,ignore
//! #[test]
//! fn my_env_test() {
//!     let _guard = crate::test_support::env_lock()
//!         .lock()
//!         .unwrap_or_else(|p| p.into_inner());
//!     // … modify/read env vars safely …
//! }
//! ```

use std::sync::{Mutex, OnceLock};

/// Returns a reference to the process-wide environment lock.
///
/// All tests in this binary that mutate process environment variables
/// (HOME, PATH, AMPLIHACK_*, etc.) or call `set_current_dir()` must hold this
/// lock for the duration of the mutation. Tests that assert behavior derived
/// from `current_dir()` should also take this lock so they cannot race those
/// mutations during parallel execution.
pub(crate) fn env_lock() -> &'static Mutex<()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}
