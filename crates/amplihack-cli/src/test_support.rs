use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Single global lock for all environment-mutating tests.
///
/// Both HOME and CWD mutations must serialize through one lock to prevent
/// races. Tests that need both HOME and CWD should acquire `env_lock()` once.
pub(crate) fn env_lock() -> &'static Mutex<()> {
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
