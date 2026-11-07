// File: crates/amplihack-workflows/tests/canonical_staging_runner_test.rs
//! Actual external runner tests. Opt in explicitly; ignored cases are NOT acceptance.
#![cfg(unix)]
#[path = "support/canonical_staging.rs"]
mod support;
use support::*;

#[test]
#[ignore = "requires actual runner; adversarial Git selectors and a task-private alternate index"]
fn inherited_git_selectors_cannot_change_caller_or_alternate_indexes() {
    let case = inventory()
        .into_iter()
        .find(|c| c.step_id == "step-04-address-concerns")
        .unwrap();
    let result = run(&case, Variant::HostileSelectors, true);
    executed(&result);
    assert!(
        result.violations.is_empty(),
        "index-integrity: {:?}",
        result.violations
    );
    assert!(!result.receipt["inherited_alternate_index"].is_null());
}

#[test]
#[ignore = "requires actual runner; parsed output must reach a second conditional agent invocation"]
fn repeated_agent_sequence_preserves_indexes_after_context_propagation() {
    let case = inventory()
        .into_iter()
        .find(|c| c.step_id == "step-02-crusty-review")
        .unwrap();
    let result = run(&case, Variant::RepeatedSequence, false);
    executed(&result);
    assert_eq!(
        result.receipt["adapter_execution"]["executions"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "second conditional invocation did not execute"
    );
    assert!(
        result.violations.is_empty(),
        "index-integrity: {:?}",
        result.violations
    );
}

macro_rules! behavioural {
    ($name:ident, $id:literal) => {
        #[test]
        #[ignore = "requires an authorised actual runner and durable evidence directory; no real models"]
        fn $name() {
            let case = inventory().into_iter().find(|c| c.step_id == $id).unwrap();
            let actual = run(&case, Variant::Actual, false);
            executed(&actual);
            assert!(actual.violations.is_empty(), "index-integrity: {:?}; receipt {}",
                actual.violations, actual.f.evidence.display());
            assert_eq!(step(&case)["auto_stage"].as_bool(), Some(false));
            // A separate fixture removes ONLY this step's opt-out. No test code
            // calls git add after adapter return; the real runner makes the decision.
            let control = run(&case, Variant::RemovedOptOut, false);
            executed(&control);
            assert!(control.violations.iter().any(|v| v.starts_with("index-integrity:")),
                "removal control did not fail the intended assertion: {}", control.f.evidence.display());
        }
    };
}

#[test]
#[ignore = "requires actual runner; isolates byte-level bookkeeping from adapter writes"]
fn disabled_staging_noop_preserves_raw_index_bytes() {
    let case = inventory()
        .into_iter()
        .find(|c| c.step_id == "step-02-crusty-review")
        .unwrap();
    let result = run(&case, Variant::DisabledNoOp, true);
    executed(&result);
    assert!(
        result.violations.is_empty(),
        "strict no-op contract failed: {:?}",
        result.violations
    );
}

#[test]
#[ignore = "requires actual runner; linked worktree, mode, deletion, rename, symlink and literal-path matrix"]
fn deferred_mutation_preserves_partial_staging_and_linked_indexes() {
    let case = inventory()
        .into_iter()
        .find(|c| c.step_id == "step-04-address-concerns")
        .unwrap();
    let result = run(&case, Variant::DisabledMutation, true);
    executed(&result);
    assert!(
        result.violations.is_empty(),
        "index-integrity: {:?}",
        result.violations
    );
}

#[test]
#[ignore = "requires actual runner; exercises review_dir independently of caller and task"]
fn crusty_review_preserves_three_independent_indexes_with_edge_entries() {
    let case = inventory()
        .into_iter()
        .find(|c| c.step_id == "step-02-crusty-review")
        .unwrap();
    let result = run(&case, Variant::Actual, true);
    executed(&result);
    assert!(
        result.violations.is_empty(),
        "index-integrity: {:?}",
        result.violations
    );
}

behavioural!(
    step_02_clarify_requirements_integrity_and_removal_control,
    "step-02-clarify-requirements"
);
behavioural!(
    step_02b_analyze_codebase_integrity_and_removal_control,
    "step-02b-analyze-codebase"
);
behavioural!(
    step_02c_resolve_ambiguity_integrity_and_removal_control,
    "step-02c-resolve-ambiguity"
);
behavioural!(
    step_05_architecture_integrity_and_removal_control,
    "step-05-architecture"
);
behavioural!(
    step_05b_api_design_integrity_and_removal_control,
    "step-05b-api-design"
);
behavioural!(
    step_05c_database_design_integrity_and_removal_control,
    "step-05c-database-design"
);
behavioural!(
    step_05d_security_review_integrity_and_removal_control,
    "step-05d-security-review"
);
behavioural!(
    step_05e_design_consolidation_integrity_and_removal_control,
    "step-05e-design-consolidation"
);
behavioural!(
    step_06_documentation_integrity_and_removal_control,
    "step-06-documentation"
);
behavioural!(
    step_06b_documentation_review_integrity_and_removal_control,
    "step-06b-documentation-review"
);
behavioural!(
    step_06c_documentation_refinement_integrity_and_removal_control,
    "step-06c-documentation-refinement"
);
behavioural!(
    step_07_write_tests_integrity_and_removal_control,
    "step-07-write-tests"
);
behavioural!(
    step_08_implement_integrity_and_removal_control,
    "step-08-implement"
);
behavioural!(
    step_08c_work_verifier_integrity_and_removal_control,
    "step-08c-work-verifier"
);
behavioural!(
    step_08b_integration_integrity_and_removal_control,
    "step-08b-integration"
);
behavioural!(
    step_09_refactor_integrity_and_removal_control,
    "step-09-refactor"
);
behavioural!(
    step_09b_optimize_integrity_and_removal_control,
    "step-09b-optimize"
);
behavioural!(
    step_10_pre_commit_review_integrity_and_removal_control,
    "step-10-pre-commit-review"
);
behavioural!(
    step_10b_security_review_integrity_and_removal_control,
    "step-10b-security-review"
);
behavioural!(
    step_10c_philosophy_check_integrity_and_removal_control,
    "step-10c-philosophy-check"
);
behavioural!(
    step_11_incorporate_feedback_integrity_and_removal_control,
    "step-11-incorporate-feedback"
);
behavioural!(
    step_11b_implement_feedback_integrity_and_removal_control,
    "step-11b-implement-feedback"
);
behavioural!(
    step_12_run_precommit_integrity_and_removal_control,
    "step-12-run-precommit"
);
behavioural!(
    step_13_local_testing_integrity_and_removal_control,
    "step-13-local-testing"
);
behavioural!(
    step_14_bump_version_integrity_and_removal_control,
    "step-14-bump-version"
);
behavioural!(
    step_16b_outside_in_fix_loop_integrity_and_removal_control,
    "step-16b-outside-in-fix-loop"
);
behavioural!(
    step_17b_reviewer_agent_integrity_and_removal_control,
    "step-17b-reviewer-agent"
);
behavioural!(
    step_17c_security_review_integrity_and_removal_control,
    "step-17c-security-review"
);
behavioural!(
    step_17d_philosophy_guardian_integrity_and_removal_control,
    "step-17d-philosophy-guardian"
);
behavioural!(
    step_17e_address_blocking_issues_integrity_and_removal_control,
    "step-17e-address-blocking-issues"
);
behavioural!(
    step_18a_analyze_feedback_integrity_and_removal_control,
    "step-18a-analyze-feedback"
);
behavioural!(
    step_18b_implement_feedback_integrity_and_removal_control,
    "step-18b-implement-feedback"
);
behavioural!(
    step_19a_philosophy_check_integrity_and_removal_control,
    "step-19a-philosophy-check"
);
behavioural!(
    step_19b_patterns_check_integrity_and_removal_control,
    "step-19b-patterns-check"
);
behavioural!(
    step_20_final_cleanup_integrity_and_removal_control,
    "step-20-final-cleanup"
);
behavioural!(
    step_20c_quality_audit_integrity_and_removal_control,
    "step-20c-quality-audit"
);
behavioural!(
    step_22_ensure_mergeable_integrity_and_removal_control,
    "step-22-ensure-mergeable"
);
behavioural!(
    agentic_finalizer_integrity_and_removal_control,
    "agentic-finalizer"
);
behavioural!(
    step_02_crusty_review_integrity_and_removal_control,
    "step-02-crusty-review"
);
behavioural!(
    step_04_address_concerns_integrity_and_removal_control,
    "step-04-address-concerns"
);
