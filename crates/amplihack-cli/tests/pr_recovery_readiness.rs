//! crates/amplihack-cli/tests/pr_recovery_readiness.rs
//!
//! TDD contract tests for PR recovery readiness. These tests intentionally
//! define the merge-ready gate before implementation: workflow-ready no-op
//! evidence is not enough to emit MERGE_READY.

use std::path::PathBuf;

use amplihack_cli::pr_recovery_readiness::{
    AdditiveCopyEntry, AdditiveCopyPlan, CheckConclusion, CheckRollup, CheckStatus, MergeState,
    NoOpReportInput, PrHeadSnapshot, inspect_additive_copy_plan, render_no_op_report,
};

const HEAD: &str = "8fb46865fb4412038b9313a62c02cc5aa0693132";

fn verified_head() -> PrHeadSnapshot {
    PrHeadSnapshot {
        expected_head_sha: HEAD.to_string(),
        local_head_sha: HEAD.to_string(),
        pr_head_sha: HEAD.to_string(),
    }
}

fn check(name: &str, status: CheckStatus, conclusion: CheckConclusion) -> CheckRollup {
    CheckRollup::new(name, status, conclusion)
}

fn green_required_checks() -> Vec<CheckRollup> {
    vec![
        check(
            "Lint & Format",
            CheckStatus::Completed,
            CheckConclusion::Success,
        ),
        check("build", CheckStatus::Completed, CheckConclusion::Success),
        check("Test", CheckStatus::Completed, CheckConclusion::Success),
    ]
}

fn no_op_input(checks: Vec<CheckRollup>, merge_state: MergeState) -> NoOpReportInput {
    NoOpReportInput {
        head: verified_head(),
        checks,
        merge_state,
        hook_ready: true,
        additive_copy_ready: true,
        files_modified: vec![],
        manual_merge_performed: false,
        merge_bypass_performed: false,
        nested_default_workflow_launched: false,
    }
}

#[test]
fn green_checks_and_clean_merge_without_independent_gate_evidence_is_not_merge_ready() {
    let report = render_no_op_report(no_op_input(green_required_checks(), MergeState::Clean))
        .expect("exact-head no-op report should render");

    assert!(
        !report.merge_ready,
        "MERGE_READY requires more than green GitHub Actions and a clean merge state; \
         missing runnable QA/scenario evidence, docs impact, three SEEK/VALIDATE/FIX \
         audit cycles, focused diff scope, and PR description evidence must keep the \
         report NOT_MERGE_READY"
    );
}

#[test]
fn skipped_or_neutral_actions_do_not_satisfy_green_github_actions_gate() {
    let checks = vec![
        check(
            "Lint & Format",
            CheckStatus::Completed,
            CheckConclusion::Success,
        ),
        check("build", CheckStatus::Completed, CheckConclusion::Success),
        check("Test", CheckStatus::Completed, CheckConclusion::Skipped),
    ];

    let report = render_no_op_report(no_op_input(checks, MergeState::Clean))
        .expect("skipped check should produce bounded readiness evidence");

    assert!(
        !report.merge_ready,
        "a skipped required Test check is not green GitHub Actions evidence for MERGE_READY"
    );
}

#[test]
fn pending_github_gate_renders_explicit_not_merge_ready_evidence() {
    let checks = vec![
        check(
            "Lint & Format",
            CheckStatus::Completed,
            CheckConclusion::Success,
        ),
        check("build", CheckStatus::Completed, CheckConclusion::Success),
        check("Test", CheckStatus::InProgress, CheckConclusion::Pending),
    ];

    let report = render_no_op_report(no_op_input(checks, MergeState::Blocked)).expect(
        "pending Test with blocked merge state should still render workflow no-op evidence",
    );

    assert!(
        !report.merge_ready,
        "pending Test and blocked merge state cannot be merge-ready"
    );
    assert!(
        report.no_op_justification.contains("NOT_MERGE_READY"),
        "readiness output must name the final bounded status; got: {}",
        report.no_op_justification
    );
}

#[test]
fn no_op_report_rejects_files_modified_evidence() {
    let mut input = no_op_input(green_required_checks(), MergeState::Clean);
    input.files_modified = vec![PathBuf::from(
        "crates/amplihack-cli/src/pr_recovery_readiness.rs",
    )];

    let err = render_no_op_report(input).expect_err("no-op report must reject modified files");

    assert!(
        err.to_string().contains("no-op report requires no"),
        "error must identify files_modified as the blocker, got: {err}"
    );
}

#[test]
fn additive_copy_plan_requires_absolute_destination_root() {
    let plan = AdditiveCopyPlan {
        destination_root: PathBuf::from("relative/.amplihack/.claude"),
        entries: vec![AdditiveCopyEntry::file("tools/amplihack/hook.js", false)],
    };

    let err = inspect_additive_copy_plan(&plan)
        .expect_err("additive-copy readiness must reject non-absolute destination roots");

    assert!(
        err.to_string().contains("destination root"),
        "error should name destination root validation, got: {err}"
    );
}

#[test]
fn additive_copy_plan_rejects_duplicate_relative_paths() {
    let plan = AdditiveCopyPlan {
        destination_root: PathBuf::from("/tmp/.amplihack/.claude"),
        entries: vec![
            AdditiveCopyEntry::file("tools/amplihack/hook.js", false),
            AdditiveCopyEntry::file("tools/amplihack/hook.js", true),
        ],
    };

    let err = inspect_additive_copy_plan(&plan)
        .expect_err("duplicate additive-copy entries must fail closed");

    assert!(
        err.to_string().contains("duplicate"),
        "error should identify duplicate relative paths, got: {err}"
    );
}
