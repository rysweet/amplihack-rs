//! crates/amplihack-hooks/src/session_start/tests_artifact_migration.rs
//!
//! Issue #1476 / AC19 — the migration must run *before* the staleness check.
//!
//! Register from `session_start/mod.rs`:
//! ```ignore
//! #[cfg(test)]
//! mod tests_artifact_migration;
//! ```
//!
//! The ordering is load-bearing, not tidiness. `setup_blarify_indexing` asks
//! `check_index_status` whether an index exists. If it runs first, it looks in
//! the new artifact root, finds it empty, and spawns a full rebuild — a 600
//! second job — for **every existing user on their first upgraded session**,
//! while a perfectly good index sits unmoved in their checkout.
//!
//! So the assertion is behavioural: a project whose artifacts are only in the
//! repository must come out of session start needing no indexing.

use super::*;
use crate::test_support::env_lock;
use std::fs;
use std::path::Path;

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

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// A checkout in the pre-#1476 layout with a *usable* index: sources first,
/// artifacts afterwards, so `check_index_status` would call it up to date if it
/// could find it.
fn legacy_indexed_project(project: &Path) {
    write(&project.join("src/app.py"), "print('hi')\n");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let amplihack = project.join(".amplihack");
    write(&amplihack.join("blarify.json"), "{}");
    write(&amplihack.join("indexes/python.scip"), "scip");
    write(&amplihack.join("graph_db/data.kz"), "graph");
}

fn run_session_start() -> serde_json::Value {
    SessionStartHook
        .process(HookInput::SessionStart {
            session_id: Some("issue-1476".to_string()),
            cwd: None,
            extra: serde_json::json!({}),
        })
        .unwrap()
}

fn additional_context(output: &serde_json::Value) -> String {
    output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// AC19 — the first upgraded session migrates, and therefore does not re-index.
#[test]
fn session_start_migrates_before_deciding_whether_to_index() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    legacy_indexed_project(project.path());

    let _home = EnvVarGuard::set("HOME", home.path());
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", cache.path());
    let _artifact = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");
    let _graph = EnvVarGuard::unset("AMPLIHACK_GRAPH_DB_PATH");
    let _kuzu = EnvVarGuard::unset("AMPLIHACK_KUZU_DB_PATH");
    // Skip mode makes "would have indexed" observable without spawning a job.
    let _mode = EnvVarGuard::set("AMPLIHACK_BLARIFY_MODE", "skip");
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(project.path()).unwrap();

    let output = run_session_start();

    std::env::set_current_dir(&original_cwd).unwrap();

    let context = additional_context(&output);
    assert!(
        !context.contains("Code-graph setup was needed"),
        "the staleness check ran before the migration and would have \
         re-indexed an already-indexed project:\n{context}"
    );
    assert_eq!(
        output["hookSpecificOutput"]["indexing_status"],
        serde_json::json!("complete")
    );
}

/// The migration's own effect, asserted at the session-start boundary: the
/// checkout comes out clean and the artifacts are in the cache.
#[test]
fn session_start_moves_in_repo_artifacts_out_of_the_checkout() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    legacy_indexed_project(project.path());
    write(&project.path().join("index.scip"), "root-scip");

    let _home = EnvVarGuard::set("HOME", home.path());
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", cache.path());
    let _artifact = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");
    let _graph = EnvVarGuard::unset("AMPLIHACK_GRAPH_DB_PATH");
    let _kuzu = EnvVarGuard::unset("AMPLIHACK_KUZU_DB_PATH");
    let _mode = EnvVarGuard::set("AMPLIHACK_BLARIFY_MODE", "skip");
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(project.path()).unwrap();

    run_session_start();

    std::env::set_current_dir(&original_cwd).unwrap();

    assert!(
        !project.path().join(".amplihack").exists(),
        ".amplihack must be gone from the checkout after session start"
    );
    assert!(!project.path().join("index.scip").exists());

    let root = amplihack_memory::cli_memory::project_artifact_root(project.path()).unwrap();
    assert_eq!(fs::read_to_string(root.join("blarify.json")).unwrap(), "{}");
    assert_eq!(
        fs::read_to_string(root.join("graph_db/data.kz")).unwrap(),
        "graph"
    );
}

/// A6 / D10 — one status line per moved path, naming both sides, so an
/// 8 MB directory does not move without the user being told.
#[test]
fn session_start_reports_what_the_migration_moved() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    legacy_indexed_project(project.path());

    let _home = EnvVarGuard::set("HOME", home.path());
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", cache.path());
    let _artifact = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");
    let _graph = EnvVarGuard::unset("AMPLIHACK_GRAPH_DB_PATH");
    let _kuzu = EnvVarGuard::unset("AMPLIHACK_KUZU_DB_PATH");
    let _mode = EnvVarGuard::set("AMPLIHACK_BLARIFY_MODE", "skip");
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(project.path()).unwrap();

    let output = run_session_start();

    std::env::set_current_dir(&original_cwd).unwrap();

    let context = additional_context(&output);
    assert!(
        context.contains("blarify.json"),
        "the migration must name what it moved:\n{context}"
    );
    assert!(
        context.contains(&cache.path().display().to_string()),
        "the migration must name where it moved things to:\n{context}"
    );
}

/// A project that never had in-repo artifacts must not gain a migration notice.
#[test]
fn a_fresh_project_produces_no_migration_notice() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write(&project.path().join("src/app.py"), "print('hi')\n");

    let _home = EnvVarGuard::set("HOME", home.path());
    let _xdg = EnvVarGuard::set("XDG_CACHE_HOME", cache.path());
    let _artifact = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");
    let _mode = EnvVarGuard::set("AMPLIHACK_BLARIFY_MODE", "skip");
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(project.path()).unwrap();

    let output = run_session_start();

    std::env::set_current_dir(&original_cwd).unwrap();

    assert!(!additional_context(&output).contains("Moved"));
    assert!(!project.path().join(".amplihack").exists());
}
