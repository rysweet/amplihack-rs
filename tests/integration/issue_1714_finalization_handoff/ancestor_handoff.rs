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
// verification evidence: inside auto-drive the confirmed PR is handed to the
// loops as FAILED_IMPLEMENTATION; a standalone run still fails closed.
#[test]
fn failed_implementation_on_a_confirmed_open_pr_is_handed_off_only_in_auto_drive() {
    let fx = Fixture::new();
    let (_, pr_head) = merge_main_on_pushed(&fx);
    drop_pushed_branch(&fx);
    view_at(&fx, &pr_head, pending());

    let envs = pr_env(&fx);
    let mut with_flag = as_refs(&envs);
    with_flag.push(FLAG);
    let outcome = validate(&fx, &with_flag);
    assert!(
        outcome.success(),
        "the hand-off exits 0\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), "FAILED_IMPLEMENTATION");
    assert_eq!(outcome.field("terminal_success"), "false");
    assert_eq!(outcome.field("terminal_failure"), "true");
    assert_eq!(outcome.field("finalizer_output_valid"), "false");
    assert_eq!(outcome.field("pr_number"), "7");
    assert_eq!(outcome.field("pr_url"), PR_URL);
    assert_eq!(outcome.field("handoff"), "merge_ready_loop");
    let evidence = outcome.field("evidence_used");
    assert!(
        evidence.contains("pr_state=OPEN")
            && evidence.contains("pr_head_fast_forwards_local_head=true")
            && evidence.contains("all_checks=pending"),
        "the hand-off records the confirmed PR facts: {evidence}"
    );
    assert!(
        outcome
            .stderr
            .contains(&format!("{FI_HANDOFF_INFO} (PR #7)")),
        "{}",
        outcome.describe()
    );
    fx.assert_gh_reads_only();

    let outcome = validate(&fx, &as_refs(&envs));
    assert_failed_implementation(&outcome, "standalone");
    assert_no_handoff_key(&outcome, "standalone");
    assert_eq!(
        outcome.field("pr_number"),
        "",
        "a standalone failure keeps the run's own PR evidence"
    );
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
