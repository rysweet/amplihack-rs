//! The hand-off of a terminal state decided on a published outcome (#1714,
//! #2343).
//!
//! `assess-finalization` decides the state; the validator reports it and hands
//! it to the merge-ready loop under the same rules as any other hand-off. The
//! prior-state probe's own routes, and the flag rules themselves, are in
//! `validator.rs`.

use serde_json::json;

use super::fixture::*;
use super::validator::{assert_failed_closed, assert_handed_off, prior_state_env, with_flag};

// T1b: the open-PR check with a failing check run.
#[test]
fn open_pr_with_a_failing_check_is_handed_off() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(json!([check_run("COMPLETED", json!("FAILURE"))])));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs.push(assessment("BLOCKED_CI", "none", "7"));
    let outcome = validate(&fx, &with_flag(&envs));
    assert_handed_off(&outcome, "7", "open-PR check");
    assert_eq!(outcome.field("pr_url"), PR_URL);
    fx.assert_gh_reads_only();
}

// #2343: FAILED_IMPLEMENTATION on a published pull request IS handed off, so
// the crusty and merge-ready loops get the request that exists to be reviewed.
// Auto-drive's own step-03 re-reads that request's live state before the loops
// accept it (see step03.rs), which is where "is it still open" is answered.
#[test]
fn failed_implementation_on_a_named_pull_request_is_handed_off() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs.push(assessment("FAILED_IMPLEMENTATION", "none", "7"));

    let outcome = validate(&fx, &with_flag(&envs));

    assert!(
        outcome.success(),
        "the hand-off exits 0\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), "FAILED_IMPLEMENTATION");
    assert_eq!(outcome.field("terminal_success"), "false");
    assert_eq!(outcome.field("handoff"), "merge_ready_loop");
    assert_eq!(outcome.field("pr_number"), "7");
    assert_eq!(outcome.field("pr_url"), PR_URL);

    // Without the flag the same run fails closed with no hand-off key.
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let outcome = validate(&fx, &as_refs(&envs));
    assert_failed_closed(&outcome, "FAILED_IMPLEMENTATION", "standalone run");
}

// T4, validator half: BLOCKED_CI with no valid PR number is not handed off.
#[test]
fn blocked_ci_without_a_valid_pr_number_fails_closed() {
    for pr in ["", "abc", "0", "#134", "134 ", "99999999999"] {
        let fx = Fixture::new();
        let envs = prior_state_env(&fx, "BLOCKED_CI", pr);
        let outcome = validate(&fx, &with_flag(&envs));
        assert_failed_closed(&outcome, "BLOCKED_CI", &format!("pr={pr:?}"));
        assert!(
            !outcome.stderr.contains(HANDOFF_INFO),
            "pr={pr:?}\n{}",
            outcome.describe()
        );
    }
}

// The 19-key schema of a result that is not handed off is unchanged.
#[test]
fn a_success_result_has_no_handoff_key_even_with_the_flag() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs.push(assessment("EXISTING_OPEN_PR", "none", "7"));
    let outcome = validate(&fx, &with_flag(&envs));
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    assert_no_handoff_key(&outcome, "EXISTING_OPEN_PR");
    assert_eq!(outcome.json().as_object().map(|o| o.len()), Some(19));
}
