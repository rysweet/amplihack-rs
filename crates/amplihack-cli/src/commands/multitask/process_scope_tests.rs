//! Process-ownership validation tests (issues #1642, #2014, #2553).
//!
//! Split from `process_scope.rs` so both stay inside the 400-line brick limit
//! that `multitask_orchestrator_spec_test` enforces.

use super::models::{ProcessScope, WorkstreamScope};
use super::process_scope::*;
use chrono::Utc;

fn full_scopes() -> (
    CurrentWorkflowScope,
    WorkstreamScope,
    ProcessScope,
    ProcessSnapshot,
    ProcessScopeConfig,
) {
    let current = CurrentWorkflowScope {
        repository: "owner/repo".to_string(),
        repo_root: "/repo".to_string(),
        workdir: "/repo/ws-754".to_string(),
        branch: "feat/issue-754".to_string(),
        issue_id: "754".to_string(),
        work_item_id: "754".to_string(),
        recipe_run_id: "run-1".to_string(),
        tree_id: "tree-1".to_string(),
        workstream_id: "ws-754".to_string(),
    };
    let workstream_scope = WorkstreamScope {
        repository: current.repository.clone(),
        repo_root: current.repo_root.clone(),
        workdir: current.workdir.clone(),
        branch: current.branch.clone(),
        base_ref: "main".to_string(),
        issue_id: current.issue_id.clone(),
        work_item_id: current.work_item_id.clone(),
        recipe: "default-workflow".to_string(),
        recipe_run_id: current.recipe_run_id.clone(),
        tree_id: current.tree_id.clone(),
        workstream_id: current.workstream_id.clone(),
        expected_title_prefix: "Fix scoped monitor closure".to_string(),
        started_at: Utc::now().to_rfc3339(),
    };
    let process_scope = ProcessScope {
        pid: Some(4242),
        repository: current.repository.clone(),
        repo_root: current.repo_root.clone(),
        workdir: current.workdir.clone(),
        branch: current.branch.clone(),
        issue_id: current.issue_id.clone(),
        work_item_id: current.work_item_id.clone(),
        recipe_run_id: current.recipe_run_id.clone(),
        tree_id: current.tree_id.clone(),
        workstream_id: current.workstream_id.clone(),
        process_started_at: "start-1".to_string(),
        recorded_at: Utc::now().to_rfc3339(),
    };
    let snapshot = ProcessSnapshot {
        pid: 4242,
        alive: true,
        workdir: current.workdir.clone(),
        process_started_at: "start-1".to_string(),
    };
    (
        current,
        workstream_scope,
        process_scope,
        snapshot,
        ProcessScopeConfig::default(),
    )
}

#[test]
fn matching_live_process_with_full_scope_is_valid() {
    let (current, workstream_scope, process_scope, snapshot, config) = full_scopes();
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &process_scope,
            &snapshot,
            &config
        ),
        ProcessScopeValidation::Valid
    );
    assert_eq!(ProcessScopeValidation::Valid.reason(), "valid");
}

#[test]
fn legacy_state_without_process_scope_is_non_authoritative() {
    let (current, workstream_scope, _process_scope, snapshot, config) = full_scopes();
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &ProcessScope::default(),
            &snapshot,
            &config
        ),
        ProcessScopeValidation::MissingScope
    );
}

#[test]
fn dead_reused_or_too_old_process_records_are_rejected() {
    let (current, workstream_scope, mut process_scope, mut snapshot, config) = full_scopes();
    snapshot.alive = false;
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &process_scope,
            &snapshot,
            &config
        ),
        ProcessScopeValidation::Dead
    );

    snapshot.alive = true;
    snapshot.process_started_at = "different-start".to_string();
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &process_scope,
            &snapshot,
            &config
        ),
        ProcessScopeValidation::PidReused
    );

    snapshot.process_started_at = process_scope.process_started_at.clone();
    process_scope.recorded_at = "2020-01-01T00:00:00Z".to_string();
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &process_scope,
            &snapshot,
            &config
        ),
        ProcessScopeValidation::TooOld
    );
}

#[test]
fn repo_workdir_branch_and_workstream_mismatches_are_rejected() {
    let (mut current, workstream_scope, process_scope, snapshot, config) = full_scopes();
    current.repository = "other/repo".to_string();
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &process_scope,
            &snapshot,
            &config
        ),
        ProcessScopeValidation::RepoMismatch
    );

    let (mut current, workstream_scope, process_scope, snapshot, config) = full_scopes();
    current.workdir = "/other".to_string();
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &process_scope,
            &snapshot,
            &config
        ),
        ProcessScopeValidation::WorkdirMismatch
    );

    let (mut current, workstream_scope, process_scope, snapshot, config) = full_scopes();
    current.branch = "other".to_string();
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &process_scope,
            &snapshot,
            &config
        ),
        ProcessScopeValidation::BranchMismatch
    );

    let (mut current, workstream_scope, process_scope, snapshot, config) = full_scopes();
    current.workstream_id = "other".to_string();
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &process_scope,
            &snapshot,
            &config
        ),
        ProcessScopeValidation::WorkstreamMismatch
    );
}
