//! crates/amplihack-hooks/src/post_tool_use/tests_blarify_stale_location.rs
//!
//! Issue #1476 — the staleness marker moves with everything else.
//!
//! Register from `post_tool_use/mod.rs`:
//! ```ignore
//! #[cfg(test)]
//! mod tests_blarify_stale_location;
//! ```
//!
//! `mark_blarify_stale_if_needed` open-codes `<root>/.amplihack/blarify_stale`.
//! Without this, the very first code edit after a successful migration
//! re-creates `.amplihack/` in the checkout, and the next `git add -A` picks it
//! up again — a fix that undoes itself.

use super::validation::mark_blarify_stale_if_needed;
use crate::test_support::env_lock;
use std::fs;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: serialised by env_lock().
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: serialised by env_lock().
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: serialised by env_lock().
        unsafe {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
fn the_staleness_marker_is_written_outside_the_checkout() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set("HOME", home.path());
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", cache.path());
    let _artifact = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(project.path()).unwrap();

    mark_blarify_stale_if_needed("Write", &serde_json::json!({ "file_path": "src/app.py" }));

    std::env::set_current_dir(&original_cwd).unwrap();

    assert!(
        !project.path().join(".amplihack").exists(),
        "editing a code file must not re-create .amplihack in the checkout"
    );
    let marker = amplihack_memory::cli_memory::project_artifact_paths(project.path())
        .unwrap()
        .blarify_stale;
    assert!(
        marker.exists(),
        "the marker must still be written, just somewhere else: {}",
        marker.display()
    );
}

/// The function returns `()`. An unresolvable artifact root must warn and
/// return, not panic a hook that runs after every tool call.
#[test]
fn an_unresolvable_artifact_root_does_not_panic_the_hook() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let project = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::unset("HOME");
    let _xdg = EnvVarGuard::unset("XDG_CACHE_HOME");
    let _artifact = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(project.path()).unwrap();

    mark_blarify_stale_if_needed("Write", &serde_json::json!({ "file_path": "src/app.py" }));

    std::env::set_current_dir(&original_cwd).unwrap();

    assert!(!project.path().join(".amplihack").exists());
}
