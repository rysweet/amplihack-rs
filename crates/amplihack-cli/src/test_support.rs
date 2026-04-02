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
