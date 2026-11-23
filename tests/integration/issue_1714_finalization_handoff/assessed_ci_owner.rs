//! #2966, crusty round 3: a `BLOCKED_CI` that leaves out its cause or its
//! owner's step is reported as `BLOCKED_CI`, never refused.
//!
//! `BLOCKED_CI` is the one terminal state whose published definition is a
//! person's next move: a check that fails, or cannot run, for a cause the run
//! cannot fix, with the cause in `reason` and the step its owner must take in
//! `next_action`. Completing both sentences is guidance in the assessment
//! prompt, for the agent to do. Rounds 1 and 2 also made the validator refuse
//! a decision without them (`FAILED_MISSING_TERMINAL_EVIDENCE`), so a run
//! could end over a missing sentence rather than over the cause. That refusal
//! is gone. The validator reports the decision the assessment made, with a
//! sentence that names what was left out in place of a generic placeholder,
//! and the validator step's recovery agent completes the owner and the step
//! in its hand-off. Inside auto-drive the pull request is handed on as for any
//! `BLOCKED_CI`.

use serde_json::json;

use crate::issue_1641_finalization_open_pr::builders::{assessment_raw, check_run_named};

use super::fixture::*;
use super::validator::{assert_handed_off, with_flag};

const CAUSE: &str =
    "deploy: the workflow reads the secret DEPLOY_TOKEN, which the repository does not define";
const HUMAN_STEP: &str = "repository administrator: add the DEPLOY_TOKEN secret under Settings, Secrets and variables, Actions, then re-run the failed deploy check";

/// Every way the two sentences can be missing.
const UNOWNED: [(&str, &str); 4] = [("", HUMAN_STEP), (CAUSE, ""), ("", ""), ("   ", " ")];

/// A `BLOCKED_CI` as the agent could return it, with either sentence left out.
/// `reason` carries the cause and `next_action` the step its owner must take.
fn unowned_env(fx: &Fixture, reason: &str, next: &str, pr: &str) -> Vec<(&'static str, String)> {
    fx.gh_view(&open_pr(json!([
        check_run_named("ci", "COMPLETED", json!("SUCCESS")),
        check_run_named("deploy", "COMPLETED", json!("FAILURE")),
    ])));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs.push(assessment_raw(json!({
        "terminal_state": "BLOCKED_CI",
        "action_taken": "none",
        "pr_number": pr,
        "reason": reason,
        "next_action": next,
    })));
    envs
}

/// The sentence the record carries for each one the assessment left out.
const NO_CAUSE: &str =
    "assess-finalization reported BLOCKED_CI but named no failing check and no cause";
const NO_STEP: &str = "assess-finalization named no owner and no step: read the failing check on the pull request, then name who must act and the exact step they must take";

fn expected(reason: &str, next: &str) -> (String, String) {
    let blank = |s: &str| s.trim().is_empty();
    (
        if blank(reason) { NO_CAUSE } else { reason }.to_owned(),
        if blank(next) { NO_STEP } else { next }.to_owned(),
    )
}

// Standalone: the decision is reported as BLOCKED_CI, which fails the step
// for its recovery agent like any BLOCKED_CI, and the record says which
// sentence the assessment left out. It is not FAILED_MISSING_TERMINAL_EVIDENCE
// and carries no placeholder pointing back at the assessment.
#[test]
fn a_blocked_ci_without_a_cause_or_an_owners_step_is_reported_not_refused() {
    for (reason, next) in UNOWNED {
        let fx = Fixture::new();
        let envs = unowned_env(&fx, reason, next, "7");

        let outcome = validate(&fx, &as_refs(&envs));

        let why = format!("reason={reason:?} next_action={next:?}");
        assert_eq!(
            outcome.field("terminal_state"),
            "BLOCKED_CI",
            "{why}: the assessment's decision is what is reported\n{}",
            outcome.describe()
        );
        assert_no_handoff_key(&outcome, &why);
        assert_eq!(
            outcome.output.status.code(),
            Some(1),
            "{why}: BLOCKED_CI fails the step for its recovery agent\n{}",
            outcome.describe()
        );
        assert!(
            outcome.stderr.contains("failed closed: BLOCKED_CI: "),
            "{why}: the one-line failure names the state\n{}",
            outcome.describe()
        );
        assert!(
            !outcome.stderr.contains("FAILED_MISSING_TERMINAL_EVIDENCE"),
            "{why}: a missing sentence is not a refusal\n{}",
            outcome.describe()
        );
        let (want_reason, want_next) = expected(reason, next);
        assert_eq!(outcome.field("terminal_reason"), want_reason, "{why}");
        assert_eq!(outcome.field("required_next_action"), want_next, "{why}");
        assert!(
            !outcome
                .field("required_next_action")
                .contains("See the finalization assessment"),
            "{why}: no placeholder stands in for the owner's step\n{}",
            outcome.describe()
        );
    }
}

// Inside auto-drive the same answer carries the pull request to the
// merge-ready loop as BLOCKED_CI, with the same sentences.
#[test]
fn inside_auto_drive_an_unowned_blocked_ci_hands_the_pull_request_on() {
    for (reason, next) in UNOWNED {
        let fx = Fixture::new();
        let envs = unowned_env(&fx, reason, next, "7");

        let outcome = validate(&fx, &with_flag(&envs));

        let why = format!("reason={reason:?} next_action={next:?}");
        assert_handed_off(&outcome, "7", &why);
        let (want_reason, want_next) = expected(reason, next);
        assert_eq!(outcome.field("terminal_reason"), want_reason, "{why}");
        assert_eq!(outcome.field("required_next_action"), want_next, "{why}");
    }
}

// With the flag but no pull request named, there is nothing for the loops to
// review: the decision is still reported as BLOCKED_CI, and nothing is handed
// on.
#[test]
fn an_unowned_blocked_ci_that_names_no_pull_request_is_reported_without_a_handoff() {
    let fx = Fixture::new();
    let envs = unowned_env(&fx, "", "", "");

    let outcome = validate(&fx, &with_flag(&envs));

    assert_eq!(
        outcome.field("terminal_state"),
        "BLOCKED_CI",
        "{}",
        outcome.describe()
    );
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{}",
        outcome.describe()
    );
    assert_no_handoff_key(&outcome, "no pull request named");
    assert!(
        !outcome.stderr.contains(HANDOFF_INFO),
        "{}",
        outcome.describe()
    );
}

// The other side: both sentences present are reported as given, handed on
// with the cause and the step.
#[test]
fn a_blocked_ci_with_both_sentences_is_reported_as_given() {
    let fx = Fixture::new();
    let envs = unowned_env(&fx, CAUSE, HUMAN_STEP, "7");

    let outcome = validate(&fx, &with_flag(&envs));

    assert_handed_off(&outcome, "7", "cause and step both named");
    assert_eq!(outcome.field("terminal_reason"), CAUSE);
    assert_eq!(outcome.field("required_next_action"), HUMAN_STEP);
}
