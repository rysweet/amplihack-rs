//! crates/amplihack-memory/src/cli_memory/code_graph/paths_artifact_root_tests.rs
//!
//! Issue #1476 — contracts for code-graph path resolution once artifacts live
//! outside the checkout.
//!
//! Register from `code_graph/mod.rs`:
//! ```ignore
//! #[cfg(test)]
//! #[path = "paths_artifact_root_tests.rs"]
//! mod paths_artifact_root_tests;
//! ```
//!
//! Two things are being fixed here, and only one of them is the reported bug:
//!
//! 1. `graph_db`/`kuzu_db` move out of `<project>/.amplihack/`.
//! 2. `project_root_for_blarify_input` recovers the project by walking two
//!    parents up from `blarify.json`. Under the slug layout that can never
//!    match, so `infer_code_graph_db_path_from_input` falls through to
//!    `default_code_graph_db_path()` — **the current working directory**.
//!    `amplihack index-code <blarify.json>` would then write a graph database
//!    into whatever repository the user happens to be standing in. That is the
//!    same bug class, relocated, and the silent fallback is removed rather than
//!    repointed.

use super::paths::{
    ProjectCodeGraphPaths, code_graph_compatibility_notice_for_input,
    infer_code_graph_db_path_from_input, project_code_graph_paths, project_root_for_blarify_input,
    resolve_code_graph_db_path_for_project, resolve_project_code_graph_paths,
};
use crate::cli_memory::{ensure_artifact_root, project_artifact_paths, project_artifact_root};
use crate::test_support::{CwdGuard, HomeGuard, env_lock};
use std::fs;
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

fn lock() -> std::sync::MutexGuard<'static, ()> {
    env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Fixture {
    project: TempDir,
    cache: TempDir,
    _home_dir: TempDir,
    _home: HomeGuard,
    _xdg: EnvVarGuard,
    _artifact: EnvVarGuard,
    _graph: EnvVarGuard,
    _kuzu: EnvVarGuard,
}

fn fixture() -> Fixture {
    let home_dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    Fixture {
        _home: HomeGuard::set(home_dir.path()),
        _xdg: EnvVarGuard::set("XDG_CACHE_HOME", cache.path()),
        _artifact: EnvVarGuard::unset("AMPLIHACK_ARTIFACT_DIR"),
        _graph: EnvVarGuard::unset("AMPLIHACK_GRAPH_DB_PATH"),
        _kuzu: EnvVarGuard::unset("AMPLIHACK_KUZU_DB_PATH"),
        project: TempDir::new().unwrap(),
        cache,
        _home_dir: home_dir,
    }
}

// ── AC1' (graph half): exhaustive destructure ─────────────────────────────

/// The companion to the `ProjectArtifactPaths` destructure in
/// `cli_memory/tests.rs`. No `..`: adding a field must fail to compile until
/// somebody states where it lives.
#[test]
fn every_code_graph_path_resolves_under_the_artifact_root() {
    let _g = lock();
    let fx = fixture();

    let root = project_artifact_root(fx.project.path()).unwrap();
    let ProjectCodeGraphPaths {
        neutral,
        legacy,
        resolved,
    } = project_code_graph_paths(fx.project.path()).unwrap();

    assert_eq!(neutral, root.join("graph_db"));
    assert_eq!(legacy, root.join("kuzu_db"));
    assert_eq!(resolved, neutral);
    for path in [&neutral, &legacy, &resolved] {
        assert!(
            !path.starts_with(fx.project.path()),
            "{} is inside the checkout",
            path.display()
        );
    }
}

#[test]
fn resolved_graph_db_path_for_a_project_is_outside_the_checkout() {
    let _g = lock();
    let fx = fixture();

    let resolved = resolve_code_graph_db_path_for_project(fx.project.path()).unwrap();

    assert!(!resolved.starts_with(fx.project.path()));
    assert!(resolved.starts_with(fx.cache.path()));
}

// ── D12 / AC12: the containment guard is re-anchored, not removed ─────────

/// The guard at `paths.rs:41-57` rejects a legacy `kuzu_db` shim whose
/// canonical target escapes the project root. Once artifacts live outside the
/// checkout by design that anchor rejects every valid path — so it is
/// re-anchored to the **artifact** root. A symlink escaping *that* must still
/// be refused.
#[test]
#[cfg(unix)]
fn legacy_shim_symlink_escaping_the_artifact_root_is_still_rejected() {
    let _g = lock();
    let fx = fixture();
    let outside = TempDir::new().unwrap();
    let root = ensure_artifact_root(fx.project.path()).unwrap().root;
    std::os::unix::fs::symlink(outside.path(), root.join("kuzu_db")).unwrap();

    let err = resolve_project_code_graph_paths(fx.project.path()).unwrap_err();

    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("escapes"),
        "the guard's wording shape is preserved; got: {rendered}"
    );
}

/// The complement: a real legacy directory inside the artifact root still
/// activates the shim. Re-anchoring must not turn the guard into a wall.
#[test]
fn legacy_shim_inside_the_artifact_root_still_activates() {
    let _g = lock();
    let fx = fixture();
    let root = ensure_artifact_root(fx.project.path()).unwrap().root;
    fs::create_dir_all(root.join("kuzu_db")).unwrap();

    let paths = resolve_project_code_graph_paths(fx.project.path()).unwrap();

    assert_eq!(paths.resolved, root.join("kuzu_db"));
}

// ── D4 / R13 / AC15: the reverse mapping ──────────────────────────────────

/// AC15 — `blarify.json` now lives at `<artifact_root>/blarify.json`, one
/// parent hop from the root, and the project is recovered through the pointer
/// file rather than by counting `..`.
#[test]
fn blarify_input_round_trips_to_its_project_through_the_pointer_file() {
    let _g = lock();
    let fx = fixture();
    ensure_artifact_root(fx.project.path()).unwrap();
    let blarify_json = project_artifact_paths(fx.project.path())
        .unwrap()
        .blarify_json;
    fs::write(&blarify_json, "{}").unwrap();

    let recovered = project_root_for_blarify_input(&blarify_json).unwrap();

    assert_eq!(recovered, Some(fx.project.path().canonicalize().unwrap()));
}

/// AC15 — **security regression test; do not delete as redundant.**
///
/// With the pointer gone the project is unknown. The old code answered that
/// question with `std::env::current_dir()`, which is a confused-deputy write:
/// `amplihack index-code <some blarify.json>` creates a graph database inside
/// whatever repository the process happens to be standing in. An error is the
/// only acceptable answer.
#[test]
fn an_unresolvable_blarify_input_errors_and_never_falls_back_to_the_cwd() {
    let _g = lock();
    let fx = fixture();
    let standing_in = TempDir::new().unwrap();
    let _cwd = CwdGuard::set(standing_in.path()).unwrap();

    let orphan = fx.cache.path().join("orphan/blarify.json");
    fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    fs::write(&orphan, "{}").unwrap();

    let result = infer_code_graph_db_path_from_input(&orphan);

    match result {
        Err(_) => {}
        Ok(path) => panic!(
            "expected an error; instead resolved to {} while standing in {}",
            path.display(),
            standing_in.path().display()
        ),
    }
}

#[test]
fn an_unresolvable_blarify_input_yields_no_compatibility_notice_instead_of_guessing() {
    let _g = lock();
    let fx = fixture();
    let standing_in = TempDir::new().unwrap();
    let _cwd = CwdGuard::set(standing_in.path()).unwrap();

    let orphan = fx.cache.path().join("orphan2/blarify.json");
    fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    fs::write(&orphan, "{}").unwrap();

    assert_eq!(
        code_graph_compatibility_notice_for_input(&orphan, None).unwrap(),
        None,
        "a notice about a project we cannot identify is a guess about the cwd"
    );
}

/// The env override still has highest precedence — that ordering is what makes
/// `env_builder` the production write path, and it is deliberately unchanged.
#[test]
fn the_graph_db_env_override_still_wins() {
    let _g = lock();
    let fx = fixture();
    let explicit = TempDir::new().unwrap();
    let _graph = EnvVarGuard::set("AMPLIHACK_GRAPH_DB_PATH", explicit.path().join("graph_db"));

    let resolved = resolve_code_graph_db_path_for_project(fx.project.path()).unwrap();

    assert_eq!(resolved, explicit.path().join("graph_db"));
}
