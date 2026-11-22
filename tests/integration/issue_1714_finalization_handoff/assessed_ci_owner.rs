//! #2966: a `BLOCKED_CI` that names no owner is no decision where a person is
//! the destination, and still a hand-off where one is not.
//!
//! `BLOCKED_CI` is the one terminal state whose published definition is a
//! person's next move: a check that fails, or cannot run, for a cause the run
//! cannot fix, with the cause in `reason` and the step its owner must take in
//! `next_action`. Reported without one of them, it ends the run with no cause
//! or no step for the owner. The validator's placeholder sentences would hide
//! that, so it asks the same question it asks of a pull-request state that
//! names no pull request -- whether the decision named what it asserts. It
//! re-judges no fact and reads no check.
//!
//! Inside auto-drive the state does not end the run: the pull request goes to
//! the merge-ready loop, which repairs it, and nobody is waiting on the two
//! sentences. The hand-off therefore still happens, carrying the gap in place
//! of them, because stopping a drive over a missing sentence is the failure
//! mode #2966 exists to remove.

use serde_json::json;

use crate::issue_1641_finalization_open_pr::builders::{
    assert_no_decision, assessment_raw, check_run_named,
};

use super::fixture::*;
use super::validator::{assert_handed_off, with_flag};

const CAUSE: &str =
    "deploy: the workflow reads the secret DEPLOY_TOKEN, which the repository does not define";
const HUMAN_STEP: &str = "repository administrator: add the DEPLOY_TOKEN secret under Settings, Secrets and variables, Actions, then re-run the failed deploy check";

/// Every way the two sentences can be missing, with what the record must say.
const UNOWNED: [(&str, &str, &str); 4] = [
    ("", HUMAN_STEP, "no cause"),
    (CAUSE, "", "no step for its owner"),
    ("", "", "no cause and no step for its owner"),
    ("   ", " ", "no cause and no step for its owner"),
];

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

// #2966, from the step-17b review. Standalone, BLOCKED_CI ends the run for a
// person, so the cause and that person's step are part of the decision. Without
// them the validator's placeholder sentences would stand in, and the run would
// end at exit 81 pointing back at the assessment with no owner and no step. The
// decision contradicts its own definition, which is no decision: the recovery
// agent's to close.
#[test]
fn a_blocked_ci_without_a_cause_or_an_owners_step_is_no_decision() {
    for (reason, next, missing) in UNOWNED {
        let fx = Fixture::new();
        let envs = unowned_env(&fx, reason, next, "7");

        let outcome = validate(&fx, &as_refs(&envs));

        let why = format!("reason={reason:?} next_action={next:?}");
        assert_no_decision(&outcome, &why);
        assert_no_handoff_key(&outcome, &why);
        assert_eq!(
            outcome.output.status.code(),
            Some(1),
            "{why}: no decision fails the step\n{}",
            outcome.describe()
        );
        assert!(
            outcome
                .stderr
                .contains("failed closed: FAILED_MISSING_TERMINAL_EVIDENCE: "),
            "{why}: the one-line failure names the state\n{}",
            outcome.describe()
        );
        let terminal_reason = outcome.field("terminal_reason");
        assert!(
            terminal_reason.contains("BLOCKED_CI") && terminal_reason.contains(missing),
            "{why}: the record says what the decision left out: {terminal_reason}"
        );
        assert!(
            !outcome
                .field("required_next_action")
                .contains("See the finalization assessment"),
            "{why}: no placeholder stands in for the owner's step\n{}",
            outcome.describe()
        );
    }
}

// Inside auto-drive the same answer still carries the pull request to the
// merge-ready loop, which is where the repair continues. The drive is not
// stopped over a missing sentence; the gap is recorded in place of it.
#[test]
fn inside_auto_drive_an_unowned_blocked_ci_still_hands_the_pull_request_on() {
    for (reason, next, missing) in UNOWNED {
        let fx = Fixture::new();
        let envs = unowned_env(&fx, reason, next, "7");

        let outcome = validate(&fx, &with_flag(&envs));

        let why = format!("reason={reason:?} next_action={next:?}");
        assert_handed_off(&outcome, "7", &why);
        let terminal_reason = outcome.field("terminal_reason");
        assert!(
            terminal_reason.contains("BLOCKED_CI") && terminal_reason.contains(missing),
            "{why}: the hand-off records what the decision left out: {terminal_reason}"
        );
    }
}

// With the flag but no pull request to hand on, there is nothing for the loops
// to review, so the answer is no decision whatever the flag says.
#[test]
fn an_unowned_blocked_ci_that_names_no_pull_request_is_no_decision() {
    let fx = Fixture::new();
    let envs = unowned_env(&fx, "", "", "");

    let outcome = validate(&fx, &with_flag(&envs));

    assert_no_decision(&outcome, "no pull request named");
    assert_no_handoff_key(&outcome, "no pull request named");
    assert!(
        !outcome.stderr.contains(HANDOFF_INFO),
        "{}",
        outcome.describe()
    );
}

// The other side: both sentences present is still the decision it always was,
// handed on with the cause and the step. Guards the rule above against widening
// into a refusal of ordinary BLOCKED_CI answers.
#[test]
fn a_blocked_ci_with_both_sentences_is_still_the_decision() {
    let fx = Fixture::new();
    let envs = unowned_env(&fx, CAUSE, HUMAN_STEP, "7");

    let outcome = validate(&fx, &with_flag(&envs));

    assert_handed_off(&outcome, "7", "cause and step both named");
    assert_eq!(outcome.field("terminal_reason"), CAUSE);
    assert_eq!(outcome.field("required_next_action"), HUMAN_STEP);
}
