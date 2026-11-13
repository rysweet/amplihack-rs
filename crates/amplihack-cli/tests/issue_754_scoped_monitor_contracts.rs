//! crates/amplihack-cli/tests/issue_754_scoped_monitor_contracts.rs
//!
//! TDD-red contracts for issue #754.
//!
//! Multitask state keeps the scope metadata used to identify and resume a
//! workstream, and recipe selection does not infer work from recent PRs.

use std::path::{Path, PathBuf};

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> PathBuf {
    crate_root()
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

fn read_crate_file(rel: &str) -> String {
    let path = crate_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn read_repo_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn multitask_state_model_persists_scope_but_keeps_legacy_json_compatible() {
    let models = read_crate_file("src/commands/multitask/models.rs");

    for required in [
        "pub struct WorkstreamScope",
        "pub struct ProcessScope",
        "pub workstream_scope: WorkstreamScope",
        "pub process_scope: ProcessScope",
        "#[serde(default)]",
        "repository",
        "repo_root",
        "workdir",
        "branch",
        "base_ref",
        "issue_id",
        "work_item_id",
        "recipe_run_id",
        "tree_id",
        "workstream_id",
        "process_started_at",
    ] {
        assert!(
            models.contains(required),
            "PersistedState must carry serde-defaulted scope field `{required}` so legacy state remains readable"
        );
    }
}

#[test]
fn execution_persists_workstream_and_process_scope_at_launch_time() {
    let execution = read_crate_file("src/commands/multitask/execution.rs");
    let command_builder = read_crate_file("src/commands/multitask/command_builder.rs");
    let persistence = read_crate_file("src/commands/multitask/persistence.rs");
    let surface = format!("{execution}\n{command_builder}\n{persistence}");

    for required in [
        "workstream_scope",
        "process_scope",
        "repository",
        "repo_root",
        "workdir",
        "branch",
        "base_ref",
        "issue_id",
        "recipe_run_id",
        "tree_id",
        "workstream_id",
        "process_started_at",
    ] {
        assert!(
            surface.contains(required),
            "execution, command building, and persistence must retain scope metadata `{required}`"
        );
    }
}

#[test]
fn current_workflow_recipes_do_not_use_recent_author_pr_fallbacks() {
    for rel in [
        "amplifier-bundle/recipes/workflow-terminal-state.yaml",
        "amplifier-bundle/recipes/workflow-tdd.yaml",
        "amplifier-bundle/recipes/quality-loop.yaml",
    ] {
        let content = read_repo_file(rel);
        if rel.ends_with("quality-loop.yaml") {
            assert!(
                content.contains("survey-open-prs"),
                "quality-loop may keep broad PR survey behavior only in explicit survey steps"
            );
            continue;
        }
        for forbidden in [
            "gh pr list --author",
            "--author @me",
            "--author=@me",
            "sort:updated-desc",
            "sort:created-desc",
            ".[0] // {}",
            "head -1",
            "tail -1",
        ] {
            assert!(
                !content.contains(forbidden),
                "{rel} must not infer current-work PR identity via `{forbidden}`"
            );
        }
    }
}
