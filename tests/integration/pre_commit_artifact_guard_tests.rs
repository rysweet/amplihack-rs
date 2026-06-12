//! tests/integration/pre_commit_artifact_guard_tests.rs
//!
//! Contracts for local pre-commit Artifact Guard wiring.
//!
//! The hook must scan repository state rather than only the filenames passed by
//! pre-commit, because issue #755 is about ignored/untracked generated artifacts
//! left in the parent worktree.

use serde_yaml::Value;
use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn pre_commit_config() -> Value {
    let path = workspace_root().join(".pre-commit-config.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn local_hooks(config: &Value) -> Vec<&Value> {
    config
        .get("repos")
        .and_then(Value::as_sequence)
        .expect("pre-commit config must contain repos")
        .iter()
        .filter(|repo| repo.get("repo").and_then(Value::as_str) == Some("local"))
        .flat_map(|repo| {
            repo.get("hooks")
                .and_then(Value::as_sequence)
                .expect("local repo must contain hooks")
        })
        .collect()
}

fn hook<'a>(hooks: &'a [&Value], id: &str) -> &'a Value {
    hooks
        .iter()
        .copied()
        .find(|hook| hook.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("missing local pre-commit hook `{id}`"))
}

#[test]
fn pre_commit_config_has_full_repo_artifact_guard_hook() {
    let config = pre_commit_config();
    let hooks = local_hooks(&config);
    let hook = hook(&hooks, "artifact-guard");

    assert_eq!(
        hook.get("pass_filenames").and_then(Value::as_bool),
        Some(false),
        "Artifact Guard must scan full repo state, not only pre-commit filenames"
    );
    assert_eq!(
        hook.get("language").and_then(Value::as_str),
        Some("system"),
        "Artifact Guard should use the repo's system-command hook convention"
    );
    assert_eq!(
        hook.get("always_run").and_then(Value::as_bool),
        Some(true),
        "Artifact Guard must run even when only ignored/untracked artifacts are present"
    );
}

#[test]
fn pre_commit_artifact_guard_entry_uses_repo_cli_and_pre_commit_mode() {
    let config = pre_commit_config();
    let hooks = local_hooks(&config);
    let hook = hook(&hooks, "artifact-guard");
    let entry = hook
        .get("entry")
        .and_then(Value::as_str)
        .expect("artifact guard hook must declare an entry");

    assert!(
        entry.contains("amplihack hygiene artifact-guard")
            || entry.contains("cargo run --bin amplihack -- hygiene artifact-guard"),
        "hook must invoke the Artifact Guard CLI or repo-local cargo fallback; entry was `{entry}`"
    );
    assert!(
        entry.contains("--repo .") && entry.contains("--mode pre-commit"),
        "hook must pass explicit repo and pre-commit mode; entry was `{entry}`"
    );
    assert!(
        entry.contains("CARGO_TARGET_DIR") && entry.contains("/tmp"),
        "cargo fallback must isolate build output outside the parent worktree; entry was `{entry}`"
    );
}

#[test]
fn pre_commit_artifact_guard_hook_is_not_limited_by_files_filter() {
    let config = pre_commit_config();
    let hooks = local_hooks(&config);
    let hook = hook(&hooks, "artifact-guard");

    assert!(
        hook.get("files").is_none() && hook.get("types").is_none(),
        "Artifact Guard hook must not use files/types filters because ignored-present artifacts may not be in the commit file list"
    );
}

#[test]
fn pre_commit_hook_order_runs_artifact_guard_before_format_lint_and_tests() {
    let config = pre_commit_config();
    let hooks = local_hooks(&config);
    let artifact_index = hooks
        .iter()
        .position(|hook| hook.get("id").and_then(Value::as_str) == Some("artifact-guard"))
        .expect("artifact guard hook must exist");

    for later_hook in ["cargo-fmt", "cargo-clippy", "cargo-test"] {
        let later_index = hooks
            .iter()
            .position(|hook| hook.get("id").and_then(Value::as_str) == Some(later_hook))
            .unwrap_or_else(|| panic!("missing expected hook `{later_hook}`"));
        assert!(
            artifact_index < later_index,
            "Artifact Guard should fail fast before `{later_hook}`"
        );
    }
}

#[test]
fn pre_commit_build_hooks_use_isolated_target_dir() {
    let config = pre_commit_config();
    let hooks = local_hooks(&config);

    for id in ["cargo-clippy", "cargo-test"] {
        let hook = hook(&hooks, id);
        let entry = hook
            .get("entry")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{id} must declare an entry"));
        assert!(
            entry.contains("CARGO_TARGET_DIR") && entry.contains("/tmp"),
            "{id} must isolate Cargo build output outside the parent worktree; entry was `{entry}`"
        );
    }
}
