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

use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

/// The five profile-redirecting variables and the guards that keep a test's
/// settings writes inside its own temporary home (issue #2922). One
/// definition, shared with the other crates' `test_support`.
#[path = "../../../tests/support/claude_profile_isolation.rs"]
pub(crate) mod claude_profile_isolation;

/// Returns a reference to the process-wide environment lock.
///
/// All tests in this binary that mutate process environment variables
/// (HOME, PATH, AMPLIHACK_*, etc.) or call `set_current_dir()` must hold this
/// lock for the duration of the mutation. Tests that assert behavior derived
/// from `current_dir()` should also take this lock so they cannot race those
/// mutations during parallel execution.
///
/// Creating the lock also fires [`claude_profile_isolation::purge_inherited`]
/// (issue #2922), so a `CLAUDE_CONFIG_DIR` or login profile inherited from the
/// session that started `cargo test` is gone before the first test that holds
/// this lock mutates anything. It runs exactly once per process.
pub(crate) fn env_lock() -> &'static Mutex<()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| {
        claude_profile_isolation::purge_inherited();
        Mutex::new(())
    })
}

/// Restores a process environment variable when dropped.
pub(crate) struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    pub(crate) fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }

    /// Set `key` to `value` for the guard's lifetime, restoring the prior value
    /// (or unsetting it if it was previously absent) on drop. Callers must hold
    /// [`env_lock`] for the duration.
    #[allow(dead_code)]
    pub(crate) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// Clears `AMPLIHACK_DISABLE_BLARIFY` for the guard's lifetime.
///
/// `setup_blarify_indexing` short-circuits when that variable is `1`, before it
/// ever consults `AMPLIHACK_BLARIFY_MODE`. An ambient value in the shell that
/// launched the test run (a developer shell, or the environment a pre-push hook
/// inherits) therefore silences every mode-specific assertion. Tests that
/// exercise the blarify setup own the variable instead of inheriting it. The
/// guard takes no lock of its own: the caller must already hold [`env_lock`]
/// and keep it for the guard's lifetime.
pub(crate) fn blarify_not_disabled() -> EnvVarGuard {
    EnvVarGuard::unset("AMPLIHACK_DISABLE_BLARIFY")
}

/// Points the hook state root (issue #1963) at a throwaway home directory.
///
/// Sets `HOME` to `home` and clears every
/// [`claude_profile_isolation::CLAUDE_PROFILE_ENV_VARS`] entry, so the state
/// root resolves to `<home>/.amplihack`, no test can reach the real
/// `~/.amplihack`, and no settings write can follow an inherited
/// `CLAUDE_CONFIG_DIR` out of `home` (issue #2922). Every variable is restored
/// on drop. The guard takes no lock of its own: the caller must already hold
/// [`env_lock`] and keep it for the guard's lifetime.
///
/// `AMPLIHACK_HOME` has exactly one owner here, the `_profile` field. Two
/// guards saving one variable restore it in the wrong order, which is why the
/// separate `AMPLIHACK_HOME` guard this type used to carry is gone.
///
/// Field order is drop order: `HOME` is restored first, then the five.
#[allow(dead_code)]
pub(crate) struct IsolatedStateHome {
    _home: EnvVarGuard,
    _profile: claude_profile_isolation::ClaudeProfileIsolation,
}

/// See [`IsolatedStateHome`]. The caller must already hold [`env_lock`].
#[allow(dead_code)]
pub(crate) fn isolate_state_home(home: &std::path::Path) -> IsolatedStateHome {
    // Taken before `HOME` moves, so a settings writer never sees the new
    // `HOME` alongside an inherited profile.
    let profile = claude_profile_isolation::ClaudeProfileIsolation::new();
    IsolatedStateHome {
        _home: EnvVarGuard::set("HOME", home),
        _profile: profile,
    }
}

/// CWD restoration must finish while the caller still owns `env_lock()`.
pub(crate) struct CwdGuard {
    previous: std::path::PathBuf,
}
impl CwdGuard {
    pub(crate) fn set(path: &std::path::Path) -> std::io::Result<Self> {
        let previous = std::env::current_dir()?;
        std::env::set_current_dir(path)?;
        Ok(Self { previous })
    }
}
impl Drop for CwdGuard {
    fn drop(&mut self) {
        if let Err(error) = std::env::set_current_dir(&self.previous) {
            eprintln!("test CWD restoration failed: {error}");
        }
    }
}

/// The scratch base no framework lookup can see past; the rule is shared
/// with `amplihack-cli` and `amplihack-types` (see the file's documentation).
#[path = "../../amplihack-cli/tests/support/scratch_base.rs"]
#[allow(dead_code)]
pub(crate) mod scratch_base;

/// A temporary fixture outside installed framework ancestors and real homes.
/// Callers hold [`env_lock`] while selecting the scratch base.
pub(crate) fn isolated_tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(scratch_base::scratch_base()).expect("isolated hook fixture directory")
}

#[cfg(unix)]
#[path = "../../amplihack-cli/tests/support/exec_stub.rs"]
#[allow(dead_code)]
mod exec_stub;

/// Writes an executable stub at `path` (mode 0755) through the shared
/// `exec_stub` writer, so no fork of this process can inherit a write
/// descriptor to it and make a concurrent exec fail with `ETXTBSY` (issue
/// #1973).
#[cfg(unix)]
pub(crate) fn write_executable(path: &std::path::Path, body: &str) {
    exec_stub::write_executable(path, body, 0o755);
}
