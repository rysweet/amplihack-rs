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

/// Writes an executable stub at `path` (mode 0755) without this process
/// ever holding a write descriptor to it (issue #1973). A short-lived `sh`
/// writes a temporary name, sets the mode and renames it into place. If
/// this process wrote the file itself, a sibling test forking during the
/// write would inherit the descriptor, and a concurrent exec of the stub
/// would fail with `ETXTBSY`.
#[cfg(unix)]
pub(crate) fn write_executable(path: &std::path::Path, body: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let tmp = path.with_extension("stub-tmp");
    let mut child = Command::new("/bin/sh")
        .args([
            "-c",
            "cat > \"$1\" && chmod 755 \"$1\" && mv -f \"$1\" \"$2\"",
            "sh",
        ])
        .arg(&tmp)
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot start sh to write {}: {e}", path.display()));
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(body.as_bytes())
        .unwrap_or_else(|e| panic!("cannot stream {}: {e}", path.display()));
    let status = child.wait().expect("wait for sh");
    assert!(
        status.success(),
        "writing {} failed: {status}",
        path.display()
    );
}
