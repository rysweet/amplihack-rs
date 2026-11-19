//! #1714 case 2, hand-off half: on the mistt #138 history built in
//! `ancestor.rs`, auto-drive hands a `FAILED_IMPLEMENTATION` on the confirmed
//! open PR to its loops; a standalone run, and a PR the open-PR check did not
//! confirm, still fail closed.

use serde_json::json;

use super::ancestor::{
    FLAG, commit_run_work, drop_pushed_branch, merge_main_on_pushed, pending, pr_env, view_at,
};
use super::fixture::*;
use crate::issue_1641_finalization_open_pr::fixture::assert_failed_implementation;

// The mistt #138 history while CI has not reported and this run has no
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

    let envs = pr_env(&fx);
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
            evidence.contains("pr_state=OPEN")
                && evidence.contains("pr_head_fast_forwards_local_head=true")
                && evidence.contains("all_checks=pending"),
            "{why}: the result records the confirmed PR facts: {evidence}"
        );
        assert!(
            outcome.field("required_next_action").contains('7'),
            "{why}: the next action names the PR to watch\n{}",
            outcome.describe()
        );
    }
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
    let envs = pr_env(&fx);
    let mut with_flag = as_refs(&envs);
    with_flag.push(FLAG);
    let outcome = validate(&fx, &with_flag);
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "BLOCKED_CI");
    assert_eq!(outcome.field("handoff"), "merge_ready_loop");
    assert_eq!(outcome.field("pr_number"), "7");
}

// FAILED_IMPLEMENTATION is handed off only for a PR the open-PR check
// confirmed. A PR number with no confirmed PR (here the head diverged) keeps
// failing closed even inside auto-drive.
#[test]
fn failed_implementation_without_a_confirmed_pr_is_never_handed_off() {
    let fx = Fixture::new();
    commit_run_work(&fx);
    view_at(&fx, &"0".repeat(40), pending());
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_NUMBER", "7".to_owned()));
    let mut with_flag = as_refs(&envs);
    with_flag.push(FLAG);
    let outcome = validate(&fx, &with_flag);
    assert_failed_implementation(&outcome, "unconfirmed PR");
    assert_no_handoff_key(&outcome, "unconfirmed PR");
    assert!(
        !outcome.stderr.contains("handed to the merge-ready loop"),
        "{}",
        outcome.describe()
    );
}
