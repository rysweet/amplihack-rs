//! TDD contract tests for stale/superseded workflow change-request cleanup.

use amplihack_workflows::stale_cleanup::{
    CleanupAction, CleanupMode, CleanupPlan, CleanupPolicy, StaleChangeRequest,
};
use amplihack_workflows::workflow_contract::{
    ChangeRequestKind, ChangeRequestStatus, RepositoryProvider,
};

#[test]
fn dry_run_reports_superseded_workflow_owned_prs_without_mutation() {
    let policy = CleanupPolicy {
        provider: RepositoryProvider::GitHub,
        mode: CleanupMode::DryRun,
        workflow_label: "amplihack-workflow".into(),
        superseded_by_label_prefix: "superseded-by:".into(),
        minimum_age_hours: 48,
    };
    let candidates = vec![StaleChangeRequest {
        kind: ChangeRequestKind::PullRequest,
        id: "812".into(),
        title: "Provider abstraction PR 1".into(),
        state: ChangeRequestStatus::Open,
        labels: vec!["amplihack-workflow".into(), "superseded-by:834".into()],
        age_hours: 96,
        has_unmerged_meaningful_diff: false,
    }];

    let plan = CleanupPlan::build(policy, candidates).expect("cleanup plan should build");

    assert_eq!(plan.mode, CleanupMode::DryRun);
    assert_eq!(plan.actions.len(), 1);
    assert_eq!(
        plan.actions[0].action,
        CleanupAction::WouldCloseAsSuperseded
    );
    assert_eq!(plan.actions[0].change_request_id, "812");
    assert_eq!(plan.mutations_executed, 0);
}

#[test]
fn cleanup_refuses_unlabeled_or_meaningful_diff_candidates() {
    let policy = CleanupPolicy {
        provider: RepositoryProvider::GitHub,
        mode: CleanupMode::Apply,
        workflow_label: "amplihack-workflow".into(),
        superseded_by_label_prefix: "superseded-by:".into(),
        minimum_age_hours: 48,
    };
    let candidates = vec![
        StaleChangeRequest {
            kind: ChangeRequestKind::PullRequest,
            id: "101".into(),
            title: "User-owned PR".into(),
            state: ChangeRequestStatus::Open,
            labels: vec!["superseded-by:834".into()],
            age_hours: 96,
            has_unmerged_meaningful_diff: false,
        },
        StaleChangeRequest {
            kind: ChangeRequestKind::PullRequest,
            id: "102".into(),
            title: "Workflow PR with remaining diff".into(),
            state: ChangeRequestStatus::Open,
            labels: vec!["amplihack-workflow".into(), "superseded-by:834".into()],
            age_hours: 96,
            has_unmerged_meaningful_diff: true,
        },
    ];

    let plan = CleanupPlan::build(policy, candidates).expect("cleanup plan should build");

    assert!(
        plan.actions
            .iter()
            .all(|action| action.action == CleanupAction::Skip)
    );
    assert_eq!(plan.mutations_executed, 0);
}
