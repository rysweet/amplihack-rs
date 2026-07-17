//! crates/amplihack-cli/tests/issue_754_scoped_monitor_contracts.rs
//!
//! TDD-red contracts for issue #754.
//!
//! Monitor notifications and closure decisions must be authorized by persisted
//! workflow/process scope, not by recent PR ordering, broad text search, or
//! PID-only process liveness.

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
fn process_scope_module_exists_with_fail_closed_api() {
    let path = crate_root().join("src/commands/multitask/process_scope.rs");
    assert!(
        path.exists(),
        "process_scope.rs must provide first-class process ownership validation for issue #754"
    );
    let module = std::fs::read_to_string(&path).expect("read process_scope.rs");

    for required in [
        "pub struct CurrentWorkflowScope",
        "pub struct ProcessSnapshot",
        "pub struct ProcessScopeConfig",
        "pub enum ProcessScopeValidation",
        "pub fn validate_process_scope",
        "ProcessScopeValidation::Valid",
        "ProcessScopeValidation::MissingScope",
        "ProcessScopeValidation::Dead",
        "ProcessScopeValidation::PidReused",
        "ProcessScopeValidation::TooOld",
        "ProcessScopeValidation::RepoMismatch",
        "ProcessScopeValidation::WorkdirMismatch",
        "ProcessScopeValidation::BranchMismatch",
        "ProcessScopeValidation::WorkstreamMismatch",
    ] {
        assert!(
            module.contains(required),
            "process_scope.rs must expose contract item `{required}`"
        );
    }
}

#[test]
fn process_scope_module_defines_deterministic_reason_strings() {
    let module = read_crate_file("src/commands/multitask/process_scope.rs");

    for reason in [
        "valid",
        "missing_scope",
        "dead",
        "pid_reused",
        "too_old",
        "repo_mismatch",
        "workdir_mismatch",
        "branch_mismatch",
        "workstream_mismatch",
    ] {
        assert!(
            module.contains(reason),
            "process validation must expose stable snake_case reason `{reason}` for monitor diagnostics"
        );
    }
}

#[test]
fn process_scope_module_contains_behavioral_unit_tests() {
    let module = read_crate_file("src/commands/multitask/process_scope.rs");

    for test_name in [
        "matching_live_process_with_full_scope_is_valid",
        "legacy_state_without_process_scope_is_non_authoritative",
        "dead_reused_or_too_old_process_records_are_rejected",
        "repo_workdir_branch_and_workstream_mismatches_are_rejected",
    ] {
        assert!(
            module.contains(test_name),
            "process_scope.rs must include unit test `{test_name}`"
        );
    }
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
            "PersistedState must carry serde-defaulted scope field `{required}` so legacy state deserializes but cannot authorize notifications"
        );
    }
}

#[test]
fn launcher_persists_workstream_and_process_scope_at_launch_time() {
    let launcher = read_crate_file("src/commands/multitask/launcher.rs");
    let state = read_crate_file("src/commands/multitask/state.rs");
    let persistence = read_crate_file("src/commands/multitask/persistence.rs");
    let surface = format!("{launcher}\n{state}\n{persistence}");

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
            "launcher/state/persistence must persist scoped launch metadata `{required}`"
        );
    }
}

#[test]
fn orchestrator_gates_notifications_and_closure_on_valid_process_scope() {
    let orchestrator = read_crate_file("src/commands/multitask/orchestrator.rs");
    let state = read_crate_file("src/commands/multitask/state.rs");
    let joined = format!("{orchestrator}\n{state}");

    for required in [
        "process_scope",
        "validate_process_scope",
        "ProcessScopeValidation::Valid",
        "MissingScope",
        "PidReused",
        "TooOld",
        "RepoMismatch",
        "WorkdirMismatch",
        "BranchMismatch",
        "WorkstreamMismatch",
    ] {
        assert!(
            joined.contains(required),
            "monitor and closure paths must gate on process scope validation item `{required}`"
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
