//! Test-only helpers shared by this crate's unit tests.

use std::sync::{Mutex, OnceLock};

/// The one lock that serialises process-environment mutation in this crate.
///
/// Issue #1380: `cargo test` runs a crate's unit tests as threads in a single
/// process, so the environment is process-global across all of them. A mutex
/// declared inside one module excludes only the tests that reference it --
/// every test in a sibling module mutates the same variables concurrently, and
/// the resulting failure surfaces somewhere other than the racing test. There
/// is therefore exactly one of these per crate, and this is it.
///
/// Acquire it *before* the first mutation and hold it until after the last one,
/// cleanup included. Releasing early leaves the environment mutated with no
/// lock held, which is the original race wearing a disguise.
pub(crate) fn env_lock() -> &'static Mutex<()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}
