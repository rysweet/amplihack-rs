//! #1714, reporting-failure exits: when the reporting step failed, inside
//! auto-drive the pull request is handed to the loops on both states
//! (`FAILED_REPORTING` and the reporting-path `FAILED_IMPLEMENTATION`). A
//! standalone run fails closed as before, a run with no pull request at all is
//! never handed off, and a green pull request never turns a reporting failure
//! into a success.
//!
//! Since #2343 the validator makes no reporting split of its own: it carries
//! the typed `reporting_failure` into the result and into `evidence_used`, so
//! the fact reaches `assess-finalization`, and reports the state that step
//! decided. These tests supply that decision and assert the end state.

use serde_json::json;

use super::ancestor::{
    FLAG, commit_run_work, drop_pushed_branch, merge_main_on_pushed, pending, view_at,
};
use super::fixture::*;

/// A resumed run on the mistt #138 history whose reporting step failed, with
/// the state `assess-finalization` decided on it.
fn reporting_failed_env(
    fx: &Fixture,
    completed: bool,
    state: &str,
    pr: &str,
) -> Vec<(&'static str, String)> {
    // An assessment arrives as RECIPE_VAR_finalization_assessment, which puts
    // the finalization cohort in the canonical tier, so every value this run
    // needs from that cohort is given in its canonical form too.
    let mut envs = resumed_env(&fx.repo_path());
    envs.retain(|(k, _)| *k != "FINALIZER_STEP_STATUS");
    envs.push((
        "RECIPE_VAR_finalizer_step_status",
        json!({"status": "failed", "reporting_failure": "true"}).to_string(),
    ));
    envs.push(("PR_URL", PR_URL.to_owned()));
    if completed {
        envs.push((
            "RECIPE_VAR_implementation_terminal_evidence",
            json!({"implementation_completed": "true"}).to_string(),
        ));
        envs.push((
            "RECIPE_VAR_verification_terminal_evidence",
            json!({"verification_completed": "true"}).to_string(),
        ));
    }
    envs.push(assessment(state, "none", pr));
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
    assert_eq!(
        outcome.field("reporting_failure"),
        "true",
        "{why}: the typed reporting failure is carried into the result"
    );
    let evidence = outcome.field("evidence_used");
    assert!(
        evidence.contains("reporting_failure=true")
            && evidence.contains("outcome.pr=7")
            && evidence.contains("outcome.pr_state=OPEN"),
        "{why}: the hand-off records the reporting failure and the pull request: {evidence}"
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
    assert_eq!(outcome.field("reporting_failure"), "true", "{why}");
    // BEHAVIOUR CHANGE (#2343): FAILED_REPORTING now leaves through the common
    // non-success path, so `finalizer_output_valid` is "false" like every
    // other failure. Before #2343 the validator's own reporting split set it
    // "true" to say the finaliser's own output was well formed.
    assert_eq!(outcome.field("finalizer_output_valid"), "false", "{why}");
    assert_no_handoff_key(outcome, why);
}

// Implementation and verification succeeded, the reporter failed, the PR is
// open: auto-drive hands it to the loops; a standalone run fails closed
// exactly as before and never calls gh.
#[test]
fn failed_reporting_on_a_confirmed_open_pr_is_handed_off_only_in_auto_drive() {
    let fx = Fixture::new();
    confirmed_pr(&fx, pending());
    let envs = reporting_failed_env(&fx, true, "FAILED_REPORTING", "7");

    let outcome = validate(&fx, &with_flag(&envs));
    assert_handed_off(&outcome, "FAILED_REPORTING", "auto-drive");
    assert!(
        outcome
            .field("evidence_used")
            .contains("outcome.checks=pending"),
        "{}",
        outcome.describe()
    );
    fx.assert_gh_reads_only();

    let fx = Fixture::new();
    confirmed_pr(&fx, pending());
    let outcome = validate(&fx, &as_refs(&envs));
    assert_failed_reporting_closed(&outcome, "standalone");
}

// Reporting failed and the evidence is incomplete: auto-drive hands the
// confirmed PR off as FAILED_IMPLEMENTATION; standalone fails closed.
#[test]
fn reporting_path_failed_implementation_on_a_confirmed_open_pr_is_handed_off() {
    let fx = Fixture::new();
    confirmed_pr(&fx, pending());
    let envs = reporting_failed_env(&fx, false, "FAILED_IMPLEMENTATION", "7");

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
    assert_eq!(outcome.field("reporting_failure"), "true");
    assert_no_handoff_key(&outcome, "standalone");
}

// A confirmed PR with every check green is still a reporting failure: the
// check can hand it off, never classify it EXISTING_OPEN_PR.
#[test]
fn a_green_confirmed_pr_never_turns_a_reporting_failure_into_success() {
    let fx = Fixture::new();
    confirmed_pr(&fx, green_rollup());
    let envs = reporting_failed_env(&fx, true, "FAILED_REPORTING", "7");
    let outcome = validate(&fx, &with_flag(&envs));
    assert_handed_off(&outcome, "FAILED_REPORTING", "green PR");
    assert!(
        outcome
            .field("evidence_used")
            .contains("outcome.checks=green")
    );
}

// A PR whose head diverged from the local HEAD is not confirmed, so neither
// reporting exit hands it off, even inside auto-drive.
#[test]
fn reporting_failures_without_a_confirmed_pr_are_never_handed_off() {
    for completed in [true, false] {
        let expected = if completed {
            "FAILED_REPORTING"
        } else {
            "FAILED_IMPLEMENTATION"
        };
        let fx = Fixture::new();
        commit_run_work(&fx);
        view_at(&fx, &"0".repeat(40), json!([]));
        fx.gh_list(&json!([]));
        let mut envs = reporting_failed_env(&fx, completed, expected, "");
        // No pull request of the run's own either: nothing to hand on.
        envs.retain(|(k, _)| *k != "PR_URL");
        let outcome = validate(&fx, &with_flag(&envs));
        assert_eq!(
            outcome.output.status.code(),
            Some(1),
            "completed={completed}\n{}",
            outcome.describe()
        );
        assert_eq!(outcome.field("terminal_state"), expected);
        assert_eq!(outcome.field("reporting_failure"), "true");
        assert_no_handoff_key(&outcome, "no pull request");
        assert_eq!(
            outcome.field("pr_number"),
            "",
            "a failure with no pull request names none"
        );
        assert!(
            !outcome.stderr.contains("handed to the merge-ready loop"),
            "{}",
            outcome.describe()
        );
    }
}

// The validator makes no reporting split of its own any more, so the typed
// fact has to survive to the record and to assess-finalization's evidence. It
// is carried whatever the decided state is, including a success.
#[test]
fn the_typed_reporting_failure_is_carried_into_every_result() {
    for (status, expected) in [("failed", "true"), ("ok", "false")] {
        let fx = Fixture::new();
        confirmed_pr(&fx, green_rollup());
        let mut envs = resumed_env(&fx.repo_path());
        envs.retain(|(k, _)| *k != "FINALIZER_STEP_STATUS");
        envs.push((
            "RECIPE_VAR_finalizer_step_status",
            json!({"status": status}).to_string(),
        ));
        envs.push(("PR_URL", PR_URL.to_owned()));
        envs.push(assessment("EXISTING_OPEN_PR", "none", "7"));

        let outcome = validate(&fx, &as_refs(&envs));

        assert!(outcome.success(), "status={status}\n{}", outcome.describe());
        assert_eq!(
            outcome.field("reporting_failure"),
            expected,
            "status={status}: the typed fact is reported, not acted on"
        );
        assert!(
            outcome
                .field("evidence_used")
                .contains(&format!("reporting_failure={expected}")),
            "status={status}: and recorded in evidence_used: {}",
            outcome.field("evidence_used")
        );
    }
}
