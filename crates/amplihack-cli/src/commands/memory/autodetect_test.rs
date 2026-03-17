//! TDD tests for S-3: `resolve_backend_with_autodetect()` in
//! `commands/memory/mod.rs`.
//!
//! The function does NOT exist yet. These tests will fail to compile until S-3
//! is implemented.

use crate::commands::memory::{BackendChoice, resolve_backend_with_autodetect};
use crate::test_support::home_env_lock;
use std::fs;

// ---------------------------------------------------------------------------
// EnvGuard – isolates HOME and AMPLIHACK_MEMORY_BACKEND
// ---------------------------------------------------------------------------

struct AutodetectEnvGuard {
    prev_home: Option<std::ffi::OsString>,
    prev_backend: Option<std::ffi::OsString>,
    #[allow(dead_code)]
    lock: std::sync::MutexGuard<'static, ()>,
}

impl AutodetectEnvGuard {
    /// Set HOME to `home`; optionally set AMPLIHACK_MEMORY_BACKEND.
    fn setup(home: &std::path::Path, backend: Option<&str>) -> Self {
        let lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var_os("HOME");
        let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        unsafe {
            std::env::set_var("HOME", home);
            match backend {
                Some(b) => std::env::set_var("AMPLIHACK_MEMORY_BACKEND", b),
                None => std::env::remove_var("AMPLIHACK_MEMORY_BACKEND"),
            }
        }
        Self {
            prev_home,
            prev_backend,
            lock,
        }
    }
}

impl Drop for AutodetectEnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.prev_home.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match self.prev_backend.take() {
                Some(v) => std::env::set_var("AMPLIHACK_MEMORY_BACKEND", v),
                None => std::env::remove_var("AMPLIHACK_MEMORY_BACKEND"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// When `AMPLIHACK_MEMORY_BACKEND` is set, autodetect must honour it
/// immediately without inspecting the filesystem.
#[test]
fn autodetect_uses_env_var_first() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    // No .amplihack directory exists – autodetect must still return Sqlite.
    let _guard = AutodetectEnvGuard::setup(dir.path(), Some("sqlite"));

    let choice = resolve_backend_with_autodetect()?;
    assert_eq!(
        choice,
        BackendChoice::Sqlite,
        "AMPLIHACK_MEMORY_BACKEND=sqlite must take priority over filesystem probe"
    );
    Ok(())
}

/// When no env var is set but `~/.amplihack/hierarchical_memory` contains a
/// Kuzu-style graph DB directory, autodetect must return Kuzu.
#[test]
fn autodetect_falls_back_to_kuzu_if_graph_db_path_exists() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    // Create the probe path that signals an existing Kuzu installation.
    let kuzu_probe = dir.path().join(".amplihack").join("hierarchical_memory");
    fs::create_dir_all(&kuzu_probe)?;
    // Create a subdirectory that looks like an agent graph DB (kuzu marker).
    let agent_db = kuzu_probe.join("default-agent").join("graph_db");
    fs::create_dir_all(&agent_db)?;

    let _guard = AutodetectEnvGuard::setup(dir.path(), None);

    let choice = resolve_backend_with_autodetect()?;
    assert_eq!(
        choice,
        BackendChoice::GraphDb,
        "existing graph_db directory must cause autodetect to return Kuzu"
    );
    Ok(())
}

/// For a completely fresh install (no `.amplihack` directory at all),
/// autodetect must return `Sqlite` as the new default.
#[test]
fn autodetect_falls_back_to_sqlite_for_new_installs() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    // Deliberately do NOT create .amplihack – simulate a fresh install.
    let _guard = AutodetectEnvGuard::setup(dir.path(), None);

    let choice = resolve_backend_with_autodetect()?;
    assert_eq!(
        choice,
        BackendChoice::Sqlite,
        "fresh install (no .amplihack dir) must default to BackendChoice::Sqlite"
    );
    Ok(())
}

/// If the probe path contains a symlink, autodetect must return an error
/// rather than following the symlink (security: prevent traversal via symlinks
/// during backend resolution).
#[cfg(unix)]
#[test]
fn autodetect_rejects_symlinks_in_probe() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir()?;
    let hmem_dir = dir.path().join(".amplihack").join("hierarchical_memory");
    fs::create_dir_all(&hmem_dir)?;

    // Create a real target elsewhere.
    let real_target = dir.path().join("real_target");
    fs::create_dir_all(&real_target)?;

    // Place a symlink inside the probe directory.
    let symlink_path = hmem_dir.join("symlinked-agent");
    symlink(&real_target, &symlink_path).expect("create symlink for test");

    let _guard = AutodetectEnvGuard::setup(dir.path(), None);

    let result = resolve_backend_with_autodetect();
    assert!(
        result.is_err(),
        "autodetect must return Err when a symlink is found in the probe path, got: {:?}",
        result
    );
    Ok(())
}

/// When `HOME` is not set (simulated by pointing HOME to a non-existent path
/// that cannot be used), autodetect must return a structured Err.
#[test]
fn autodetect_errors_when_home_unavailable() -> anyhow::Result<()> {
    let lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let prev_home = std::env::var_os("HOME");
    let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");

    // Remove both so autodetect cannot rely on env var shortcut.
    unsafe {
        std::env::remove_var("HOME");
        std::env::remove_var("AMPLIHACK_MEMORY_BACKEND");
    }

    let result = resolve_backend_with_autodetect();

    // Restore.
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_backend {
            Some(v) => std::env::set_var("AMPLIHACK_MEMORY_BACKEND", v),
            None => std::env::remove_var("AMPLIHACK_MEMORY_BACKEND"),
        }
    }
    drop(lock);

    assert!(
        result.is_err(),
        "resolve_backend_with_autodetect must return Err when HOME is unavailable"
    );
    Ok(())
}
