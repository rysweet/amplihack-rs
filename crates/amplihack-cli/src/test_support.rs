use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub(crate) fn home_env_lock() -> &'static Mutex<()> {
    static HOME_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    HOME_LOCK.get_or_init(|| Mutex::new(()))
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
