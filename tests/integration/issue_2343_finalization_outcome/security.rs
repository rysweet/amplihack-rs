//! The validator's trust boundary (#2343, #2809).
//!
//! Everything the finalization step is handed -- the agentic assessment's own
//! JSON, the run's recorded branch name, the worktree's origin URL -- comes
//! from outside the validator and none of it is trusted. These are the cases
//! the security review named: S1 (an assessment that claims a success it did
//! not produce), S2 (a GitHub origin whose pull request cannot be read, with
//! both completion booleans set), S4 (a branch name git itself rejects) and
//! S5 (an origin URL carrying userinfo).

use super::fixture_ext::*;
use crate::issue_1641_finalization_open_pr::builders::{as_refs, green_rollup, set_branch};
use crate::issue_1641_finalization_open_pr::fixture::*;

fn resumed(fx: &Fixture) -> Vec<(&'static str, String)> {
    crate::issue_1641_finalization_open_pr::builders::resumed_env(&fx.repo_path())
}

/// Neither stream may carry `text`, whatever the outcome was.
fn assert_absent(outcome: &Outcome, text: &str, why: &str) {
    for stream in [&outcome.stdout, &outcome.stderr] {
        assert!(
            !stream.contains(text),
            "{why}: `{text}` must never be emitted\n{}",
            outcome.describe()
        );
    }
}

// S1. The assessment is an explanation, never a proof. One claiming SUCCESS
// with a pushed branch and an open PR behind it, when neither exists, must
// leave the result a failure whose reason is the real gap.
#[test]
fn an_assessment_claiming_success_cannot_manufacture_one() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.gh_no_pr();
    let local = fx.head();
    let mut envs = resumed(&fx);
    envs.push(assessment_env("SUCCESS", "opened_pull_request"));

    let outcome = fx.validate(&as_refs(&envs));

    assert!(
        !outcome.success(),
        "an unproduced success must stay a failure\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_success"), "false");
    assert_eq!(outcome.field("terminal_state"), "FAILED_IMPLEMENTATION");
    let reason = outcome.field("terminal_reason");
    for fact in [BRANCH, local.as_str()] {
        assert!(
            reason.contains(fact),
            "the reason must name the real gap (`{fact}`): {reason}\n{}",
            outcome.describe()
        );
    }
    // The record was read: its one sentence is attached to the failure, which
    // is all an assessment may contribute.
    assert!(
        reason.contains(ASSESSMENT_GAP),
        "the assessment supplies the explanation: {reason}\n{}",
        outcome.describe()
    );
    // The pull request the assessment named is not adopted as this run's.
    assert_eq!(outcome.field("pr_number"), "");
    assert_eq!(outcome.field("pr_url"), "");
    fx.assert_gh_reads_only();
}

// S2. A GitHub origin whose pull request cannot be read has an unread fact,
// not an absent one. The typed completion booleans speak only for a run with
// no GitHub pull request to read at all, so they must not stand in here.
#[test]
fn unreadable_github_outcome_never_falls_back_to_the_typed_booleans() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_unreachable();
    let mut envs = resumed(&fx);
    envs.push(("IMPLEMENTATION_COMPLETED", "true".to_owned()));
    envs.push(("VERIFICATION_COMPLETED", "true".to_owned()));

    let outcome = fx.validate(&as_refs(&envs));

    assert!(
        !outcome.success(),
        "an unread GitHub outcome is a gap, not a success\n{}",
        outcome.describe()
    );
    assert_ne!(outcome.field("terminal_state"), "IMPLEMENTED_VERIFIED");
    assert_eq!(outcome.field("terminal_success"), "false");
    let reason = outcome.field("terminal_reason");
    assert!(
        reason.contains("could not be read") && reason.contains(BRANCH),
        "the unread fact must be named, not the missing bookkeeping: {reason}\n{}",
        outcome.describe()
    );
    crate::issue_1641_finalization_open_pr::builders::assert_no_leak(&outcome);
}

// S4. A branch name git itself rejects never reaches a git or gh argument.
// The read fails soft -- one JSON result, no crash -- and nothing from the
// name appears in it.
#[test]
fn a_branch_name_git_rejects_reaches_neither_git_nor_the_result() {
    for hostile in ["feat/evilmarker..x", "-evilmarker", "feat/evilmarker\nx"] {
        let fx = Fixture::new();
        fx.with_remote();
        let mut envs = resumed(&fx);
        set_branch(&mut envs, hostile);

        let outcome = fx.validate(&as_refs(&envs));

        // One JSON object, parsed; a soft failure, not a crash.
        assert_eq!(outcome.field("terminal_success"), "false");
        assert_eq!(
            outcome.output.status.code(),
            Some(1),
            "the read must fail soft, not abort\n{}",
            outcome.describe()
        );
        assert!(
            outcome
                .field("terminal_reason")
                .contains("valid branch name"),
            "the reader must say why it could not read: {}\n{}",
            outcome.field("terminal_reason"),
            outcome.describe()
        );
        assert_absent(&outcome, "evilmarker", "a rejected branch name");
        fx.assert_gh_not_called("a branch name git rejects is never asked about");
    }
}

// S5. The origin URL is never reported: the remote is named as owner/repo
// through parse_github_repo_identity, which discards the userinfo.
#[test]
fn userinfo_in_the_origin_url_never_reaches_the_result() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.origin_with_userinfo();
    fx.gh_pr_at(&fx.head(), green_rollup());

    let outcome = fx.validate(&as_refs(&finished_env(&fx.repo_path())));

    assert!(
        outcome.success(),
        "an open PR at the local head is still the success\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    for secret in [ORIGIN_USERINFO, "ghp-s3cr3t-2343", "ci-bot"] {
        assert_absent(&outcome, secret, "origin userinfo");
    }
}

// The same holds when the read fails: a failure's own explanation is built
// from the facts, and the origin is not one of them.
#[test]
fn userinfo_stays_out_of_a_failing_result_too() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.origin_with_userinfo();
    fx.gh_unreachable();

    let outcome = fx.validate(&as_refs(&resumed(&fx)));

    assert!(!outcome.success(), "{}", outcome.describe());
    for secret in [ORIGIN_USERINFO, "ghp-s3cr3t-2343", "ci-bot"] {
        assert_absent(&outcome, secret, "origin userinfo on the failure path");
    }
}
