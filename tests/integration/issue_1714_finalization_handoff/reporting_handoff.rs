//! #1714, reporting-failure exits: when the reporting step failed, inside
//! auto-drive a PR confirmed by the open-PR check is handed to the loops on
//! both exits (`FAILED_REPORTING` and the reporting-path
//! `FAILED_IMPLEMENTATION`). A standalone run makes no gh call and fails
//! closed as before, an unconfirmed PR is never handed off, and a confirmed
//! green PR never turns a reporting failure into success.

use serde_json::json;

use super::ancestor::{
    FLAG, commit_run_work, drop_pushed_branch, merge_main_on_pushed, pending, view_at,
};
use super::fixture::*;

/// A resumed run on the mistt #138 history whose reporting step failed.
fn reporting_failed_env(fx: &Fixture, completed: bool) -> Vec<(&'static str, String)> {
    let mut envs = resumed_env(&fx.repo_path());
    envs.retain(|(k, _)| *k != "FINALIZER_STEP_STATUS");
    envs.push(("FINALIZER_STEP_STATUS", "failed".to_owned()));
    envs.push(("PR_URL", PR_URL.to_owned()));
    if completed {
        envs.push(("IMPLEMENTATION_COMPLETED", "true".to_owned()));
        envs.push(("VERIFICATION_COMPLETED", "true".to_owned()));
    }
    envs
}

fn with_flag<'a>(envs: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    let mut refs = as_refs(envs);
    refs.push(FLAG);
    refs
}

/// The PR head is a fast-forward of the local HEAD, as in the #1714 report.
fn confirmed_pr(fx: &Fixture, rollup: serde_json::Value) {
    let (_, pr_head) = merge_main_on_pushed(fx);
    drop_pushed_branch(fx);
    view_at(fx, &pr_head, rollup);
}

fn assert_handed_off(outcome: &Outcome, state: &str, why: &str) {
    assert!(
        outcome.success(),
        "{why}: the hand-off exits 0\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), state, "{why}");
    assert_eq!(outcome.field("terminal_success"), "false", "{why}");
    assert_eq!(outcome.field("terminal_failure"), "true", "{why}");
    assert_eq!(outcome.field("pr_number"), "7", "{why}");
    assert_eq!(outcome.field("pr_url"), PR_URL, "{why}");
    assert_eq!(outcome.field("handoff"), "merge_ready_loop", "{why}");
    let evidence = outcome.field("evidence_used");
    assert!(
        evidence.contains("reporting_failure=true")
            && evidence.contains("pr_state=OPEN")
            && evidence.contains("pr_head_fast_forwards_local_head=true"),
        "{why}: the hand-off records the reporting failure and the confirmed PR: {evidence}"
    );
    assert!(
        outcome.stderr.contains(&format!(
            "INFO: #1714: {state} handed to the merge-ready loop (PR #7)"
        )),
        "{why}\n{}",
        outcome.describe()
    );
}

fn assert_failed_reporting_closed(outcome: &Outcome, why: &str) {
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{why}\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), "FAILED_REPORTING", "{why}");
    assert_eq!(outcome.field("terminal_success"), "false", "{why}");
    assert_eq!(outcome.field("finalizer_output_valid"), "true", "{why}");
    assert_no_handoff_key(outcome, why);
}

// Implementation and verification succeeded, the reporter failed, the PR is
// open: auto-drive hands it to the loops; a standalone run fails closed
// exactly as before and never calls gh.
#[test]
fn failed_reporting_on_a_confirmed_open_pr_is_handed_off_only_in_auto_drive() {
    let fx = Fixture::new();
    confirmed_pr(&fx, pending());
    let envs = reporting_failed_env(&fx, true);

    let outcome = validate(&fx, &with_flag(&envs));
    assert_handed_off(&outcome, "FAILED_REPORTING", "auto-drive");
    assert!(
        outcome
            .field("evidence_used")
            .contains("all_checks=pending"),
        "{}",
        outcome.describe()
    );
    fx.assert_gh_reads_only();

    let fx = Fixture::new();
    confirmed_pr(&fx, pending());
    let outcome = validate(&fx, &as_refs(&envs));
    assert_failed_reporting_closed(&outcome, "standalone");
    fx.assert_gh_not_called("standalone reporting failure");
}

// Reporting failed and the evidence is incomplete: auto-drive hands the
// confirmed PR off as FAILED_IMPLEMENTATION; standalone fails closed.
#[test]
fn reporting_path_failed_implementation_on_a_confirmed_open_pr_is_handed_off() {
    let fx = Fixture::new();
    confirmed_pr(&fx, pending());
    let envs = reporting_failed_env(&fx, false);

    let outcome = validate(&fx, &with_flag(&envs));
    assert_handed_off(&outcome, "FAILED_IMPLEMENTATION", "auto-drive");
    fx.assert_gh_reads_only();

    let fx = Fixture::new();
    confirmed_pr(&fx, pending());
    let outcome = validate(&fx, &as_refs(&envs));
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), "FAILED_IMPLEMENTATION");
    assert_no_handoff_key(&outcome, "standalone");
    fx.assert_gh_not_called("standalone reporting-path FAILED_IMPLEMENTATION");
}

// A confirmed PR with every check green is still a reporting failure: the
// check can hand it off, never classify it EXISTING_OPEN_PR.
#[test]
fn a_green_confirmed_pr_never_turns_a_reporting_failure_into_success() {
    let fx = Fixture::new();
    confirmed_pr(&fx, green_rollup());
    let envs = reporting_failed_env(&fx, true);
    let outcome = validate(&fx, &with_flag(&envs));
    assert_handed_off(&outcome, "FAILED_REPORTING", "green PR");
    assert!(outcome.field("evidence_used").contains("all_checks=green"));
}

// A PR whose head diverged from the local HEAD is not confirmed, so neither
// reporting exit hands it off, even inside auto-drive.
#[test]
fn reporting_failures_without_a_confirmed_pr_are_never_handed_off() {
    for completed in [true, false] {
        let fx = Fixture::new();
        commit_run_work(&fx);
        view_at(&fx, &"0".repeat(40), json!([]));
        let envs = reporting_failed_env(&fx, completed);
        let outcome = validate(&fx, &with_flag(&envs));
        assert_eq!(
            outcome.output.status.code(),
            Some(1),
            "completed={completed}\n{}",
            outcome.describe()
        );
        let expected = if completed {
            "FAILED_REPORTING"
        } else {
            "FAILED_IMPLEMENTATION"
        };
        assert_eq!(outcome.field("terminal_state"), expected);
        assert_no_handoff_key(&outcome, "unconfirmed PR");
        assert_eq!(
            outcome.field("pr_number"),
            "",
            "a failure keeps the run's own PR evidence, not the rejected PR's"
        );
        assert!(
            !outcome.stderr.contains("handed to the merge-ready loop"),
            "{}",
            outcome.describe()
        );
    }
}
