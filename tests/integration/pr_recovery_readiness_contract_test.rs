//! TDD contract tests for PR #579 recovery readiness.
//!
//! These tests are intentionally written before the implementation. They define
//! the required public contract for recovering an existing PR without manually
//! merging it, bypassing merge protection, launching nested `default-workflow`
//! instances, or claiming a no-op against the wrong head.

use std::path::PathBuf;

use amplihack_cli::pr_recovery_readiness::{
    AdditiveCopyEntry, AdditiveCopyPlan, CheckConclusion, CheckRollup, CheckStatus,
    HookReadinessInput, HookRegistration, InstalledHook, MergeState, NoOpReportInput,
    PrHeadSnapshot, inspect_additive_copy_plan, inspect_hook_readiness, render_no_op_report,
    verify_pr_head,
};

const PR_579_HEAD: &str = "4041d4b650a245501d8e381b1dfed95a94b65fca";

#[test]
fn head_verifier_accepts_only_exact_expected_local_and_pr_head_match() {
    let snapshot = PrHeadSnapshot {
        expected_head_sha: PR_579_HEAD.to_string(),
        local_head_sha: PR_579_HEAD.to_string(),
        pr_head_sha: PR_579_HEAD.to_string(),
    };

    let verified = verify_pr_head(&snapshot).expect("exact three-way head match must pass");

    assert_eq!(verified.head_sha, PR_579_HEAD);
    assert!(
        verified
            .message
            .contains("local HEAD == PR headRefOid == expected_head_sha")
    );
}

#[test]
fn head_verifier_blocks_no_op_when_local_head_differs_from_expected_head() {
    let snapshot = PrHeadSnapshot {
        expected_head_sha: PR_579_HEAD.to_string(),
        local_head_sha: "1111111111111111111111111111111111111111".to_string(),
        pr_head_sha: PR_579_HEAD.to_string(),
    };

    let error = verify_pr_head(&snapshot).expect_err("local HEAD mismatch must fail closed");
    let message = error.to_string();

    assert!(message.contains("local HEAD"));
    assert!(message.contains(PR_579_HEAD));
    assert!(message.contains("1111111111111111111111111111111111111111"));
    assert!(message.contains("blocked"));
}

#[test]
fn head_verifier_blocks_no_op_when_pr_head_differs_from_expected_head() {
    let snapshot = PrHeadSnapshot {
        expected_head_sha: PR_579_HEAD.to_string(),
        local_head_sha: PR_579_HEAD.to_string(),
        pr_head_sha: "2222222222222222222222222222222222222222".to_string(),
    };

    let error = verify_pr_head(&snapshot).expect_err("PR head mismatch must fail closed");
    let message = error.to_string();

    assert!(message.contains("PR headRefOid"));
    assert!(message.contains(PR_579_HEAD));
    assert!(message.contains("2222222222222222222222222222222222222222"));
    assert!(message.contains("blocked"));
}

#[test]
fn hook_readiness_accepts_complete_copilot_and_default_workflow_hook_wiring() {
    let input = HookReadinessInput {
        installed_hooks: vec![
            InstalledHook::new("amplihack", "PreToolUse.js"),
            InstalledHook::new("amplihack", "PostToolUse.js"),
            InstalledHook::new("amplihack", "Stop.js"),
            InstalledHook::new("amplihack", "SessionStart.js"),
            InstalledHook::new("amplihack", "SessionStop.js"),
            InstalledHook::new("amplihack", "UserPromptSubmit.js"),
            InstalledHook::new("amplihack", "PreCompact.js"),
            InstalledHook::new("xpia", "PreToolUse.js"),
        ],
        native_registrations: vec![
            HookRegistration::new("PreToolUse", "amplihack-hooks pre-tool-use"),
            HookRegistration::new("PostToolUse", "amplihack-hooks post-tool-use"),
            HookRegistration::new("Stop", "amplihack-hooks stop"),
            HookRegistration::new("SessionStart", "amplihack-hooks session-start"),
            HookRegistration::new("SessionStop", "amplihack-hooks session-stop"),
            HookRegistration::new(
                "UserPromptSubmit",
                "amplihack-hooks workflow-classification-reminder",
            ),
            HookRegistration::new("UserPromptSubmit", "amplihack-hooks user-prompt-submit"),
            HookRegistration::new("PreCompact", "amplihack-hooks pre-compact"),
        ],
    };

    let readiness = inspect_hook_readiness(&input);

    assert!(
        readiness.blockers.is_empty(),
        "complete hook wiring must not produce blockers: {:?}",
        readiness.blockers
    );
    assert!(readiness.workflow_ready);
}

#[test]
fn hook_readiness_reports_missing_workflow_classification_registration_as_blocker() {
    let input = HookReadinessInput {
        installed_hooks: vec![
            InstalledHook::new("amplihack", "PreToolUse.js"),
            InstalledHook::new("amplihack", "PostToolUse.js"),
            InstalledHook::new("amplihack", "Stop.js"),
            InstalledHook::new("amplihack", "SessionStart.js"),
            InstalledHook::new("amplihack", "SessionStop.js"),
            InstalledHook::new("amplihack", "UserPromptSubmit.js"),
            InstalledHook::new("amplihack", "PreCompact.js"),
            InstalledHook::new("xpia", "PreToolUse.js"),
        ],
        native_registrations: vec![
            HookRegistration::new("PreToolUse", "amplihack-hooks pre-tool-use"),
            HookRegistration::new("PostToolUse", "amplihack-hooks post-tool-use"),
            HookRegistration::new("Stop", "amplihack-hooks stop"),
            HookRegistration::new("SessionStart", "amplihack-hooks session-start"),
            HookRegistration::new("SessionStop", "amplihack-hooks session-stop"),
            HookRegistration::new("UserPromptSubmit", "amplihack-hooks user-prompt-submit"),
            HookRegistration::new("PreCompact", "amplihack-hooks pre-compact"),
        ],
    };

    let readiness = inspect_hook_readiness(&input);

    assert!(!readiness.workflow_ready);
    assert!(
        readiness
            .blockers
            .iter()
            .any(|b| b.code == "MISSING_NATIVE_HOOK_REGISTRATION"
                && b.message.contains("workflow-classification-reminder")),
        "missing workflow classification registration must be explicit: {:?}",
        readiness.blockers
    );
}

#[test]
fn additive_copy_plan_preserves_existing_user_owned_files() {
    let plan = AdditiveCopyPlan {
        destination_root: PathBuf::from("/home/user/.amplihack"),
        entries: vec![
            AdditiveCopyEntry::file("amplifier-bundle/CLAUDE.md", true),
            AdditiveCopyEntry::file("amplifier-bundle/recipes/default-workflow.yaml", false),
        ],
    };

    let readiness = inspect_additive_copy_plan(&plan).expect("safe relative paths should inspect");

    assert!(readiness.workflow_ready);
    assert_eq!(
        readiness.action_for("amplifier-bundle/CLAUDE.md"),
        Some("skip-existing"),
        "existing user-owned files must be skipped, not overwritten"
    );
    assert_eq!(
        readiness.action_for("amplifier-bundle/recipes/default-workflow.yaml"),
        Some("copy-new")
    );
    assert!(
        readiness
            .actions
            .iter()
            .all(|a| a != "delete" && a != "overwrite-existing"),
        "additive copy must never delete or overwrite existing user-owned files: {:?}",
        readiness.actions
    );
}

#[test]
fn additive_copy_plan_rejects_absolute_paths_and_parent_directory_escapes() {
    for entry in [
        AdditiveCopyEntry::file("/tmp/escape.txt", false),
        AdditiveCopyEntry::file("../escape.txt", false),
        AdditiveCopyEntry::file("amplifier-bundle/../../escape.txt", false),
    ] {
        let plan = AdditiveCopyPlan {
            destination_root: PathBuf::from("/home/user/.amplihack"),
            entries: vec![entry],
        };

        let error = inspect_additive_copy_plan(&plan).expect_err("unsafe path must be rejected");
        let message = error.to_string();
        assert!(message.contains("path traversal") || message.contains("absolute path"));
    }
}

#[test]
fn noop_report_allows_workflow_ready_while_test_is_in_progress_and_merge_is_blocked() {
    let report = render_no_op_report(NoOpReportInput {
        head: PrHeadSnapshot {
            expected_head_sha: PR_579_HEAD.to_string(),
            local_head_sha: PR_579_HEAD.to_string(),
            pr_head_sha: PR_579_HEAD.to_string(),
        },
        checks: vec![
            CheckRollup::new(
                "Lint & Format",
                CheckStatus::Completed,
                CheckConclusion::Success,
            ),
            CheckRollup::new("build", CheckStatus::Completed, CheckConclusion::Success),
            CheckRollup::new("Test", CheckStatus::InProgress, CheckConclusion::Pending),
        ],
        merge_state: MergeState::Blocked,
        hook_ready: true,
        additive_copy_ready: true,
        files_modified: vec![],
        manual_merge_performed: false,
        merge_bypass_performed: false,
        nested_default_workflow_launched: false,
    })
    .expect("exact-head no-op report with pending Test should be workflow-ready");

    assert!(report.workflow_ready);
    assert!(!report.merge_ready);
    assert_eq!(report.head_sha, PR_579_HEAD);
    assert!(report.no_op_justification.contains(PR_579_HEAD));
    assert!(report.no_op_justification.contains("Lint/Format green"));
    assert!(report.no_op_justification.contains("builds green"));
    assert!(report.no_op_justification.contains("Test in progress"));
    assert!(report.no_op_justification.contains("merge blocked"));
    assert!(!report.manual_merge_performed);
    assert!(!report.merge_bypass_performed);
    assert!(!report.nested_default_workflow_launched);
}

#[test]
fn noop_report_rejects_manual_merge_bypass_or_nested_default_workflow() {
    for (manual_merge, merge_bypass, nested_default_workflow) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let error = render_no_op_report(NoOpReportInput {
            head: PrHeadSnapshot {
                expected_head_sha: PR_579_HEAD.to_string(),
                local_head_sha: PR_579_HEAD.to_string(),
                pr_head_sha: PR_579_HEAD.to_string(),
            },
            checks: vec![
                CheckRollup::new(
                    "Lint & Format",
                    CheckStatus::Completed,
                    CheckConclusion::Success,
                ),
                CheckRollup::new("build", CheckStatus::Completed, CheckConclusion::Success),
                CheckRollup::new("Test", CheckStatus::InProgress, CheckConclusion::Pending),
            ],
            merge_state: MergeState::Blocked,
            hook_ready: true,
            additive_copy_ready: true,
            files_modified: vec![],
            manual_merge_performed: manual_merge,
            merge_bypass_performed: merge_bypass,
            nested_default_workflow_launched: nested_default_workflow,
        })
        .expect_err("prohibited recovery path must block no-op report");

        let message = error.to_string();
        assert!(
            message.contains("manual merge")
                || message.contains("merge bypass")
                || message.contains("nested default-workflow"),
            "error must name the prohibited recovery path: {message}"
        );
    }
}
