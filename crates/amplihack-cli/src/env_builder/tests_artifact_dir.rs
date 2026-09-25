//! crates/amplihack-cli/src/env_builder/tests_artifact_dir.rs
//!
//! Issue #1476 / AC17 — contracts for `EnvBuilder::with_project_artifact_dir`.
//!
//! Register from `env_builder/mod.rs`:
//! ```ignore
//! #[cfg(test)]
//! mod tests_artifact_dir;
//! ```
//!
//! **This is the production write path.** `AMPLIHACK_GRAPH_DB_PATH` is the
//! highest-precedence input to `resolve_code_graph_db_path_for_project`, and
//! `EnvBuilder` exports it to every launched child — including the
//! `amplihack index-scip` that session start spawns. Fixing the resolver and
//! the hooks while leaving `with_project_graph_db` writing
//! `<project>/.amplihack/graph_db` puts the 8 MB database straight back in the
//! repository through the normal path, with every other test green.
//!
//! The seven existing assertions in `tests_builder.rs` (lines 64, 99, 136, 176,
//! 206, 260, 332 at the time of writing) pin the old in-repo value. They are to
//! be rewritten to this contract during implementation, not deleted — each one
//! encodes a real precedence rule from issue #250.

use super::builder::EnvBuilder;
use crate::test_support::env_lock;
use amplihack_memory::cli_memory::ensure_artifact_root;
use std::path::Path;
use tempfile::TempDir;

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

struct Fixture {
    project: TempDir,
    cache: TempDir,
    _home_dir: TempDir,
    _home: EnvVarGuard,
    _xdg: EnvVarGuard,
    _artifact: EnvVarGuard,
    _graph: EnvVarGuard,
    _kuzu: EnvVarGuard,
}

fn fixture() -> Fixture {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let home_dir = TempDir::new().unwrap();
    Fixture {
        _home: EnvVarGuard::set("HOME", home_dir.path()),
        _xdg: EnvVarGuard::set("XDG_CACHE_HOME", cache.path()),
        _artifact: EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR"),
        _graph: EnvVarGuard::unset("AMPLIHACK_GRAPH_DB_PATH"),
        _kuzu: EnvVarGuard::unset("AMPLIHACK_KUZU_DB_PATH"),
        project,
        cache,
        _home_dir: home_dir,
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// AC17 — the assertion that actually fixes the reported bug. Neither variable
/// the builder exports may name a path inside the project.
#[test]
fn exported_artifact_paths_are_outside_the_project_checkout() {
    let _g = lock();
    let fx = fixture();

    let env = EnvBuilder::new()
        .with_project_artifact_dir(fx.project.path())
        .unwrap()
        .build();

    for key in ["AMPLIHACK_ARTIFACT_DIR", "AMPLIHACK_GRAPH_DB_PATH"] {
        let value = env
            .get(key)
            .unwrap_or_else(|| panic!("{key} must be exported"));
        assert!(
            !Path::new(value).starts_with(fx.project.path()),
            "{key}={value} points inside the checkout"
        );
        assert!(
            Path::new(value).starts_with(fx.cache.path()),
            "{key}={value} is not under the per-project cache root"
        );
    }
}

/// One call, one `ensure_artifact_root`, both variables derived from it — so
/// they cannot disagree. Two independent resolutions is how the graph DB ends
/// up somewhere the indexer never looks.
#[test]
fn the_graph_db_path_is_derived_from_the_exported_artifact_dir() {
    let _g = lock();
    let fx = fixture();

    let env = EnvBuilder::new()
        .with_project_artifact_dir(fx.project.path())
        .unwrap()
        .build();

    let artifact_dir = Path::new(env.get("AMPLIHACK_ARTIFACT_DIR").unwrap());
    let graph_db = Path::new(env.get("AMPLIHACK_GRAPH_DB_PATH").unwrap());
    assert_eq!(graph_db, artifact_dir.join("graph_db"));
}

/// The legacy alias must not propagate forward — otherwise a stale
/// `AMPLIHACK_KUZU_DB_PATH` in the parent environment outlives the fix.
#[test]
fn the_legacy_kuzu_alias_is_unset_in_the_child_environment() {
    let _g = lock();
    let fx = fixture();
    let _inherited = EnvVarGuard::set("AMPLIHACK_KUZU_DB_PATH", "/inherited/legacy");

    let env = EnvBuilder::new()
        .with_project_artifact_dir(fx.project.path())
        .unwrap()
        .build();

    assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH"), None);
}

/// R15 / D3 — the worktree hazard. `EnvBuilder` exports these to every child,
/// and this repo runs agents in git worktrees. A child launched for project B
/// that inherits project A's values would write B's index into A's cache. The
/// explicit `project_root` re-resolves and overwrites all three variables; this
/// extends the issue #250 contract, it does not replace it.
#[test]
fn an_explicit_project_root_overwrites_every_inherited_artifact_variable() {
    let _g = lock();
    let fx = fixture();
    let other_project = TempDir::new().unwrap();
    let other_root = ensure_artifact_root(other_project.path()).unwrap().root;

    let _inherited_artifact = EnvVarGuard::set("AMPLIHACK_ARTIFACT_DIR", &other_root);
    let _inherited_graph = EnvVarGuard::set("AMPLIHACK_GRAPH_DB_PATH", other_root.join("graph_db"));
    let _inherited_kuzu = EnvVarGuard::set("AMPLIHACK_KUZU_DB_PATH", "/inherited/legacy");

    let env = EnvBuilder::new()
        .with_project_artifact_dir(fx.project.path())
        .unwrap()
        .build();

    let exported = Path::new(env.get("AMPLIHACK_ARTIFACT_DIR").unwrap());
    assert_ne!(
        exported, other_root,
        "project B inherited project A's artifact dir"
    );
    assert_ne!(
        Path::new(env.get("AMPLIHACK_GRAPH_DB_PATH").unwrap()),
        other_root.join("graph_db")
    );
    assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH"), None);
}

/// Distinct projects get distinct exports, including the same-basename case
/// that every git worktree in this repo produces.
#[test]
fn two_projects_with_the_same_basename_export_different_paths() {
    let _g = lock();
    let fx = fixture();
    let parent = TempDir::new().unwrap();
    let a = parent.path().join("wt-a/amplihack-rs");
    let b = parent.path().join("wt-b/amplihack-rs");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let _ = &fx;

    let env_a = EnvBuilder::new()
        .with_project_artifact_dir(&a)
        .unwrap()
        .build();
    let env_b = EnvBuilder::new()
        .with_project_artifact_dir(&b)
        .unwrap()
        .build();

    assert_ne!(
        env_a.get("AMPLIHACK_GRAPH_DB_PATH"),
        env_b.get("AMPLIHACK_GRAPH_DB_PATH")
    );
}

/// The builder resolves; it does not index. Creating the artifact root is
/// harmless before the consent gate, but nothing may be written into it here.
#[test]
fn building_the_environment_creates_an_empty_artifact_root_and_no_index() {
    let _g = lock();
    let fx = fixture();

    let env = EnvBuilder::new()
        .with_project_artifact_dir(fx.project.path())
        .unwrap()
        .build();

    let root = Path::new(env.get("AMPLIHACK_ARTIFACT_DIR").unwrap());
    assert!(root.is_dir());
    assert!(
        !root.join("graph_db").exists(),
        "no graph DB may exist before the consent gate runs"
    );
    assert!(!fx.project.path().join(".amplihack").exists());
}

/// `HOME` and `XDG_CACHE_HOME` both unset is unresolvable. The builder returns
/// the error rather than silently exporting a cwd-relative path.
#[test]
fn an_unresolvable_artifact_root_is_an_error_not_a_cwd_relative_export() {
    let _g = lock();
    let project = TempDir::new().unwrap();
    let _home = EnvVarGuard::unset("HOME");
    let _xdg = EnvVarGuard::unset("XDG_CACHE_HOME");
    let _artifact = EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR");

    assert!(
        EnvBuilder::new()
            .with_project_artifact_dir(project.path())
            .is_err()
    );
}
