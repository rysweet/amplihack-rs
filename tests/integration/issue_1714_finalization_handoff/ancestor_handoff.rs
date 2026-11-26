//! #1714 case 2, hand-off half: on the example-org #138 history built in
//! `ancestor.rs`, auto-drive hands a `FAILED_IMPLEMENTATION` on the confirmed
//! open PR to its loops; a standalone run, and a PR the open-PR check did not
//! confirm, still fail closed.

use serde_json::json;

use super::ancestor::{
    FLAG, commit_run_work, drop_pushed_branch, merge_main_on_pushed, pending, pr_env, view_at,
};
use super::fixture::*;

// The example-org #138 history while CI has not reported and this run has no
// verification evidence. Since #2343 and #2784 this is not an implementation
// failure at all: the pull request is open at a head that carries the run's
// work, so the run succeeds and hands that request on, named, with or without
// auto-drive. The hand-off path is unneeded here, because `autodrive_build_handoff`
// returns cleanly on a success and the merge-ready loop is reached anyway.
#[test]
fn pending_checks_on_a_confirmed_open_pr_hand_the_pr_on_with_or_without_auto_drive() {
    let fx = Fixture::new();
    let (_, pr_head) = merge_main_on_pushed(&fx);
    drop_pushed_branch(&fx);
    view_at(&fx, &pr_head, pending());

    let mut envs = pr_env(&fx);
    envs.push(assessment("EXISTING_OPEN_PR", "none", "7"));
    let mut with_flag = as_refs(&envs);
    with_flag.push(FLAG);
    for (refs, why) in [(with_flag, "auto-drive"), (as_refs(&envs), "standalone")] {
        let outcome = validate(&fx, &refs);
        assert!(outcome.success(), "{why}\n{}", outcome.describe());
        assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR", "{why}");
        assert_eq!(outcome.field("terminal_success"), "true", "{why}");
        assert_eq!(outcome.field("pr_number"), "7", "{why}");
        assert_eq!(outcome.field("pr_url"), PR_URL, "{why}");
        assert_no_handoff_key(&outcome, why);
        let evidence = outcome.field("evidence_used");
        assert!(
            evidence.contains("outcome.pr=7")
                && evidence.contains("outcome.pr_state=OPEN")
                && evidence.contains("outcome.checks=pending"),
            "{why}: the result records the facts it was decided on: {evidence}"
        );
    }
    // The PR head really is a fast-forward of the local head on this history,
    // which is what makes EXISTING_OPEN_PR the right call.
    assert_eq!(
        fx.fact(BRANCH, Some("7"), "pr_head_descends_local_head"),
        "true"
    );
    fx.assert_gh_reads_only();
}

// The same history with a red check is BLOCKED_CI, handed off as before.
#[test]
fn an_ancestor_local_head_with_a_failing_check_is_blocked_ci_and_handed_off() {
    let fx = Fixture::new();
    let (_, pr_head) = merge_main_on_pushed(&fx);
    drop_pushed_branch(&fx);
    view_at(
        &fx,
        &pr_head,
        json!([check_run("COMPLETED", json!("FAILURE"))]),
    );
    let mut envs = pr_env(&fx);
    envs.push(assessment("BLOCKED_CI", "none", "7"));
    let mut with_flag = as_refs(&envs);
    with_flag.push(FLAG);
    let outcome = validate(&fx, &with_flag);
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "BLOCKED_CI");
    assert_eq!(outcome.field("handoff"), "merge_ready_loop");
    assert_eq!(outcome.field("pr_number"), "7");
    assert_eq!(outcome.field("pr_url"), PR_URL);
    assert!(
        outcome
            .field("evidence_used")
            .contains("outcome.checks=failing"),
        "the failing check is recorded: {}",
        outcome.field("evidence_used")
    );
}

// #1714 case 2, at facts level: a pull request sitting at a head unrelated to
// the run's work is never read as carrying it. The reader says so plainly --
// the heads differ and merge-base did not answer "true" -- which is what
// assess-finalization decides FAILED_IMPLEMENTATION on.
#[test]
fn an_unrelated_pr_head_is_never_read_as_carrying_the_runs_work() {
    let fx = Fixture::new();
    let local = commit_run_work(&fx);
    view_at(&fx, &"0".repeat(40), pending());

    let facts = fx.facts(BRANCH, Some("7")).0;

    assert_eq!(facts["local_head"], local);
    assert_eq!(facts["pr_head"], "0".repeat(40));
    assert_eq!(facts["pr_number"], "7");
    assert_ne!(
        facts["pr_head_descends_local_head"], "true",
        "an unrelated head must never read as carrying the run's work: {facts}"
    );
}

// The hand-off needs a pull request to hand on. With none in the run's own
// evidence and none on the branch, FAILED_IMPLEMENTATION fails closed even
// inside auto-drive, and nothing is offered to the loops.
#[test]
fn failed_implementation_without_a_confirmed_pr_is_never_handed_off() {
    let fx = Fixture::new();
    commit_run_work(&fx);
    fx.gh_list(&json!([]));
    fx.gh_fail("view", 1);
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(assessment("FAILED_IMPLEMENTATION", "none", ""));
    let mut with_flag = as_refs(&envs);
    with_flag.push(FLAG);

    let outcome = validate(&fx, &with_flag);

    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), "FAILED_IMPLEMENTATION");
    assert_eq!(outcome.field("terminal_success"), "false");
    assert_eq!(outcome.field("pr_number"), "", "no pull request is named");
    assert_no_handoff_key(&outcome, "no pull request");
    assert!(
        !outcome.stderr.contains("handed to the merge-ready loop"),
        "{}",
        outcome.describe()
    );
}
