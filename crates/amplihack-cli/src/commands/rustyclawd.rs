//! Native RustyClawd launcher path.
//!
//! The Python implementation mostly detects a preferred Rust-native Claude
//! binary and then reuses the standard Claude launch flow. This module does
//! the same without delegating to `python3 -m amplihack.cli`.

use crate::commands::launch;
use anyhow::Result;
use std::env;
use std::path::{Path, PathBuf};

pub fn run_rustyclawd(args: Vec<String>) -> Result<()> {
    if let Some(path) = find_preferred_rustyclawd_binary() {
        unsafe { env::set_var("AMPLIHACK_CLAUDE_BINARY_PATH", &path) };
        println!("Using RustyClawd (Rust implementation)");
    }

    launch::run_launch("claude", false, false, true, false, args)
}

fn find_preferred_rustyclawd_binary() -> Option<PathBuf> {
    if let Ok(custom_path) = env::var("RUSTYCLAWD_PATH") {
        let path = PathBuf::from(custom_path);
        if is_executable_file(&path) {
            return Some(path);
        }
    }

    find_in_path(&["rustyclawd", "claude-code"])
}

fn find_in_path(names: &[&str]) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        for name in names {
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn finds_custom_rustyclawd_path() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("rustyclawd-custom");
        fs::write(&binary, "#!/usr/bin/env bash\n").unwrap();
        let mut perms = fs::metadata(&binary).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&binary, perms).unwrap();

        let previous = env::var_os("RUSTYCLAWD_PATH");
        unsafe { env::set_var("RUSTYCLAWD_PATH", &binary) };

        let found = find_preferred_rustyclawd_binary();

        match previous {
            Some(value) => unsafe { env::set_var("RUSTYCLAWD_PATH", value) },
            None => unsafe { env::remove_var("RUSTYCLAWD_PATH") },
        }

        assert_eq!(found.as_deref(), Some(binary.as_path()));
    }

    #[test]
    fn finds_rustyclawd_before_claude_code_on_path() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let rustyclawd = dir.path().join("rustyclawd");
        let claude_code = dir.path().join("claude-code");

        for binary in [&rustyclawd, &claude_code] {
            fs::write(binary, "#!/usr/bin/env bash\n").unwrap();
            let mut perms = fs::metadata(binary).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(binary, perms).unwrap();
        }

        let previous_path = env::var_os("PATH");
        let previous_custom = env::var_os("RUSTYCLAWD_PATH");
        unsafe {
            env::set_var("PATH", dir.path());
            env::remove_var("RUSTYCLAWD_PATH");
        }

        let found = find_preferred_rustyclawd_binary();

        match previous_path {
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        match previous_custom {
            Some(value) => unsafe { env::set_var("RUSTYCLAWD_PATH", value) },
            None => unsafe { env::remove_var("RUSTYCLAWD_PATH") },
        }

        assert_eq!(found.as_deref(), Some(rustyclawd.as_path()));
    }
}
