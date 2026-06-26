use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, OnceLock};

/// Establish process-wide hermetic defaults for env-mutating tests, exactly
/// once per test binary.
///
/// Issue #828 added a best-effort `npm install -g @mermaid-js/mermaid-cli`
/// step at the end of `local_install`. Many install tests drive `local_install`
/// / `run_install` to completion while preserving the real `PATH` (so `npm` is
/// reachable) without a pre-existing `mmdc` — which would trigger a real,
/// network-bound Chromium download during `cargo test`. Defaulting
/// `AMPLIHACK_SKIP_MMDC` here makes that step a no-op for the whole suite.
///
/// This runs the first time any test acquires the env lock, before that test
/// can call `local_install`. Tests that specifically exercise the mermaid CLI
/// detection branches override `AMPLIHACK_SKIP_MMDC` themselves while holding
/// the lock, so this default never masks those assertions. We only set the
/// default when the var is unset, so an explicit value in the environment still
/// wins.
fn ensure_hermetic_env_defaults() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("AMPLIHACK_SKIP_MMDC").is_none() {
            // SAFETY: runs once, before any env-mutating test proceeds past
            // `env_lock()`; concurrent env mutation is serialised by that lock.
            unsafe {
                std::env::set_var("AMPLIHACK_SKIP_MMDC", "1");
            }
        }
    });
}

/// Single global lock for all environment-mutating tests.
///
/// Both HOME and CWD mutations must serialize through one lock to prevent
/// races. Tests that need both HOME and CWD should acquire `env_lock()` once.
pub(crate) fn env_lock() -> &'static Mutex<()> {
    ensure_hermetic_env_defaults();
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

/// Alias for `env_lock()` — all env locks use the same underlying mutex.
pub(crate) fn home_env_lock() -> &'static Mutex<()> {
    env_lock()
}

/// Alias for `env_lock()` — all env locks use the same underlying mutex.
pub(crate) fn cwd_env_lock() -> &'static Mutex<()> {
    env_lock()
}

pub(crate) fn set_home(path: &Path) -> Option<std::ffi::OsString> {
    let previous = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", path);
    }
    previous
}

pub(crate) fn restore_home(previous: Option<std::ffi::OsString>) {
    if let Some(value) = previous {
        unsafe { std::env::set_var("HOME", value) };
    } else {
        unsafe { std::env::remove_var("HOME") };
    }
}

pub(crate) fn set_cwd(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let previous = std::env::current_dir()?;
    std::env::set_current_dir(path)?;
    Ok(previous)
}

pub(crate) fn restore_cwd(previous: &Path) -> std::io::Result<()> {
    std::env::set_current_dir(previous)
}

/// RAII guard that sets the current working directory and restores the
/// previous value on drop, even when the test panics.
///
/// Tests must acquire `env_lock()` (or an alias like `cwd_env_lock()`) before
/// constructing this guard so concurrent tests don't observe the mutated cwd.
/// Restore-on-drop is best-effort: if the prior cwd was deleted, the
/// restoration is silently dropped — but the guard will never leak the test's
/// chosen cwd into a subsequent test that runs in the same process.
pub(crate) struct CwdGuard {
    previous: PathBuf,
}

impl CwdGuard {
    /// Switch the process cwd to `path`, returning a guard that restores the
    /// prior cwd on drop.
    pub(crate) fn set(path: &Path) -> std::io::Result<Self> {
        let previous = std::env::current_dir()?;
        std::env::set_current_dir(path)?;
        Ok(Self { previous })
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

/// RAII guard that sets `HOME` and restores the previous value on drop, even
/// when the test panics.
///
/// Tests must acquire `env_lock()` (or `home_env_lock()`) before constructing
/// this guard so concurrent tests don't observe the mutated HOME.
pub(crate) struct HomeGuard {
    previous: Option<std::ffi::OsString>,
}

impl HomeGuard {
    /// Set `HOME` to `path`, returning a guard that restores (or unsets) the
    /// prior value on drop.
    pub(crate) fn set(path: &Path) -> Self {
        let previous = std::env::var_os("HOME");
        // SAFETY: edition 2024 requires unsafe; tests serialise via env_lock().
        unsafe {
            std::env::set_var("HOME", path);
        }
        Self { previous }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // SAFETY: edition 2024 requires unsafe; tests serialise via env_lock().
        unsafe {
            match self.previous.take() {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

/// RAII guard that clears `AMPLIHACK_GRAPH_DB_PATH` and `AMPLIHACK_KUZU_DB_PATH`
/// for the duration of a test and restores the previous values on drop.
///
/// `EnvBuilder::with_project_graph_db` reads these env vars first and falls
/// back to a project-derived path only when neither is set. Tests that assert
/// against the project-derived path therefore must clear any ambient values
/// the developer's shell or CI runner may have set, otherwise the leaking
/// value wins and the assertion fails. Acquire the `env_lock()` before
/// constructing this guard so concurrent tests don't observe the cleared
/// state.
pub(crate) struct ClearedGraphDbEnv {
    previous_graph: Option<std::ffi::OsString>,
    previous_kuzu: Option<std::ffi::OsString>,
}

impl ClearedGraphDbEnv {
    pub(crate) fn new() -> Self {
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        }
        Self {
            previous_graph,
            previous_kuzu,
        }
    }
}

impl Drop for ClearedGraphDbEnv {
    fn drop(&mut self) {
        unsafe {
            match self.previous_graph.take() {
                Some(value) => std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value),
                None => std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH"),
            }
            match self.previous_kuzu.take() {
                Some(value) => std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value),
                None => std::env::remove_var("AMPLIHACK_KUZU_DB_PATH"),
            }
        }
    }
}
