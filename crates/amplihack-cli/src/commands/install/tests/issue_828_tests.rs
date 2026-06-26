//! Hermetic tests for issue #828:
//! "Add best-effort installation of the mermaid CLI (mmdc) to `amplihack install`".
//!
//! The mermaid CLI provisioning is an OPTIONAL component: it must never fail or
//! block the overall install. These tests exercise every non-network branch of
//! [`super::super::mermaid_cli::ensure_mermaid_cli`] by manipulating PATH and
//! the `AMPLIHACK_SKIP_MMDC` opt-out env var under the shared env lock. The
//! real `npm install -g` path is intentionally NOT exercised (no network).
//!
//! Covered contracts:
//!   * `AMPLIHACK_SKIP_MMDC` set      → skip entirely (`SkippedByEnv`).
//!   * `mmdc` already on PATH         → skip (`AlreadyPresent`).
//!   * `npm` absent                   → graceful skip (`SkippedNpmAbsent`).
//!   * hostile env (both absent)      → never errors; returns an outcome.

use super::super::mermaid_cli::{Outcome, ensure_mermaid_cli};
use crate::test_support::home_env_lock;
use std::sync::MutexGuard;

/// Acquire the global env lock (shared with the other env-mutating tests in
/// this crate) so PATH / env mutations here don't race.
fn lock_env() -> MutexGuard<'static, ()> {
    home_env_lock().lock().unwrap_or_else(|p| p.into_inner())
}

/// Saved env values restored on drop so a test never leaks PATH / the opt-out
/// var into a sibling test running in the same process.
struct EnvSnapshot {
    path: Option<std::ffi::OsString>,
    skip: Option<std::ffi::OsString>,
}

impl EnvSnapshot {
    fn capture() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            skip: std::env::var_os("AMPLIHACK_SKIP_MMDC"),
        }
    }
}

impl Drop for EnvSnapshot {
    fn drop(&mut self) {
        // SAFETY: edition 2024 requires unsafe; tests serialise via the env lock.
        unsafe {
            match self.path.take() {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
            match self.skip.take() {
                Some(v) => std::env::set_var("AMPLIHACK_SKIP_MMDC", v),
                None => std::env::remove_var("AMPLIHACK_SKIP_MMDC"),
            }
        }
    }
}

/// Write an executable `mmdc` stub into `dir` that succeeds on `--version`, so
/// the PATH probe treats mermaid as already installed without a real install.
fn write_mmdc_stub(dir: &std::path::Path) {
    let stub = dir.join("mmdc");
    std::fs::write(&stub, "#!/bin/sh\necho '11.0.0'\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn opt_out_env_skips_entirely() {
    let _guard = lock_env();
    let _snap = EnvSnapshot::capture();
    // SAFETY: serialised via env lock; restored by EnvSnapshot on drop.
    unsafe {
        std::env::set_var("AMPLIHACK_SKIP_MMDC", "1");
    }
    assert_eq!(ensure_mermaid_cli(), Outcome::SkippedByEnv);
}

#[test]
fn already_present_mmdc_is_skipped() {
    let _guard = lock_env();
    let _snap = EnvSnapshot::capture();
    let temp = tempfile::tempdir().unwrap();
    write_mmdc_stub(temp.path());
    // PATH has only the stub dir: `mmdc --version` resolves to the stub, and we
    // must short-circuit before ever probing npm.
    unsafe {
        std::env::remove_var("AMPLIHACK_SKIP_MMDC");
        std::env::set_var("PATH", temp.path());
    }
    assert_eq!(ensure_mermaid_cli(), Outcome::AlreadyPresent);
}

#[test]
fn npm_absent_skips_without_error() {
    let _guard = lock_env();
    let _snap = EnvSnapshot::capture();
    let temp = tempfile::tempdir().unwrap();
    // Empty PATH dir: neither mmdc nor npm resolvable. Must skip gracefully and
    // never attempt an install.
    unsafe {
        std::env::remove_var("AMPLIHACK_SKIP_MMDC");
        std::env::set_var("PATH", temp.path());
    }
    assert_eq!(ensure_mermaid_cli(), Outcome::SkippedNpmAbsent);
}

#[test]
fn never_propagates_error_in_hostile_env() {
    // `ensure_mermaid_cli` returns `Outcome`, not `Result`: non-fatality is
    // structural. This asserts it behaviorally — a hostile env yields a skip,
    // not an install or a panic.
    let _guard = lock_env();
    let _snap = EnvSnapshot::capture();
    let temp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::remove_var("AMPLIHACK_SKIP_MMDC");
        std::env::set_var("PATH", temp.path());
    }
    let outcome = ensure_mermaid_cli();
    assert_ne!(outcome, Outcome::Installed);
    assert_eq!(outcome, Outcome::SkippedNpmAbsent);
}
