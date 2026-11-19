//! Outcome-first classification: the pushed head, the PR at that head and its
//! check class decide the result. Context artefacts never do.

use super::fixture_ext::*;
use crate::issue_1641_finalization_open_pr::builders::{as_refs, green_rollup};
use crate::issue_1641_finalization_open_pr::fixture::*;

fn assert_hands_pr_on(outcome: &Outcome) {
    assert!(outcome.success(), "must succeed\n{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    assert_eq!(outcome.field("terminal_success"), "true");
    assert_eq!(outcome.field("terminal_failure"), "false");
    assert_eq!(outcome.field("pr_number"), "7");
    assert_eq!(outcome.field("pr_url"), PR_URL);
    assert!(
        !outcome.stderr.contains(INVALID_CONTEXT_WARNING),
        "absent context must not be reported as invalid\n{}",
        outcome.describe()
    );
}

/// Failure that names the gap by the facts of the fixture.
fn assert_gap_names(outcome: &Outcome, facts: &[&str]) {
    assert!(!outcome.success(), "must fail\n{}", outcome.describe());
    assert_eq!(outcome.field("terminal_success"), "false");
    assert_eq!(outcome.field("terminal_failure"), "true");
    assert!(
        !outcome.stderr.contains(INVALID_CONTEXT_WARNING),
        "the gap must be the real one, not the context file\n{}",
        outcome.describe()
    );
    let reason = outcome.field("terminal_reason");
    for fact in facts {
        assert!(
            reason.contains(fact),
            "terminal_reason must carry `{fact}`: {reason}\n{}",
            outcome.describe()
        );
    }
}

#[test]
fn finished_run_with_no_context_file_passes() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());

    let outcome = fx.validate(&as_refs(&finished_env(&fx.repo_path())));

    assert_hands_pr_on(&outcome);
    fx.assert_gh_reads_only();
}

#[test]
fn nonexistent_context_file_sends_the_check_to_the_evidence() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());
    let mut envs = finished_env(&fx.repo_path());
    envs.push((
        "AMPLIHACK_CONTEXT_FILE",
        fx.tmp
            .path()
            .join("absent/context.json")
            .display()
            .to_string(),
    ));

    assert_hands_pr_on(&fx.validate(&as_refs(&envs)));
}

#[test]
fn unreadable_context_file_sends_the_check_to_the_evidence() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());
    let file = fx.tmp.path().join("context.json");
    std::fs::write(&file, "{ not json").expect("write");
    let mut envs = finished_env(&fx.repo_path());
    envs.push(("AMPLIHACK_CONTEXT_FILE", file.display().to_string()));

    assert_hands_pr_on(&fx.validate(&as_refs(&envs)));
}

// #2784: PR open at the pushed head, checks still pending, no verification
// boolean. Finished work, handed on, not an implementation failure.
#[test]
fn pending_checks_on_open_pr_hand_the_pr_on() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), pending_rollup());

    let outcome = fx.validate(&as_refs(&finished_env(&fx.repo_path())));

    assert_hands_pr_on(&outcome);
    assert!(
        outcome.field("required_next_action").contains('7'),
        "next action must name the PR\n{}",
        outcome.describe()
    );
    assert_ne!(outcome.field("terminal_state"), "FAILED_IMPLEMENTATION");
}

#[test]
fn pending_checks_do_not_depend_on_verification_completed() {
    for flag in ["true", "false"] {
        let fx = Fixture::new();
        fx.with_remote();
        fx.push();
        fx.gh_pr_at(&fx.head(), pending_rollup());
        let mut envs = finished_env(&fx.repo_path());
        envs.push(("VERIFICATION_COMPLETED", flag.to_owned()));

        assert_hands_pr_on(&fx.validate(&as_refs(&envs)));
    }
}

#[test]
fn checks_not_yet_reported_hand_the_pr_on() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), serde_json::json!([]));

    assert_hands_pr_on(&fx.validate(&as_refs(&finished_env(&fx.repo_path()))));
}

#[test]
fn failing_checks_stay_blocked_ci() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), failing_rollup());

    let outcome = fx.validate(&as_refs(&finished_env(&fx.repo_path())));

    assert!(!outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "BLOCKED_CI");
}

#[test]
fn pr_head_differing_from_local_head_fails_naming_both_heads() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    let local = fx.head();
    let other = fx.unrelated_commit();
    fx.gh_pr_at(&other, green_rollup());

    let outcome = fx.validate(&as_refs(&finished_env(&fx.repo_path())));

    assert_gap_names(&outcome, &[short(&local), short(&other), "7"]);
}

#[test]
fn local_head_ahead_of_pushed_and_pr_head_fails_naming_both_heads() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    let pushed = fx.head();
    let local = fx.commit_more("later.txt");
    fx.gh_pr_at(&pushed, green_rollup());

    let outcome = fx.validate(&as_refs(&finished_env(&fx.repo_path())));

    assert_gap_names(&outcome, &[short(&local), short(&pushed)]);
}

#[test]
fn branch_never_pushed_and_no_pr_fails_naming_the_branch() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.gh_no_pr();
    let local = fx.head();

    let outcome = fx.validate(&as_refs(&resumed(&fx)));

    assert_gap_names(&outcome, &[BRANCH, short(&local)]);
}

#[test]
fn pushed_branch_with_no_pr_fails_naming_the_branch() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_no_pr();

    let outcome = fx.validate(&as_refs(&resumed(&fx)));

    assert_gap_names(&outcome, &[BRANCH]);
}

// Typed booleans and finalizer prose are not evidence of a push.
#[test]
fn typed_success_booleans_cannot_manufacture_success() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.gh_no_pr();
    let mut envs = resumed(&fx);
    envs.push(("IMPLEMENTATION_COMPLETED", "true".to_owned()));
    envs.push(("VERIFICATION_COMPLETED", "true".to_owned()));
    envs.push((
        "AGENTIC_FINALIZER_NARRATIVE",
        "pushed, PR open, CI green".to_owned(),
    ));

    let outcome = fx.validate(&as_refs(&envs));

    assert!(!outcome.success(), "{}", outcome.describe());
    assert_ne!(outcome.field("terminal_success"), "true");
}

#[test]
fn gh_erroring_is_not_an_unexplained_terminal_failure() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_fail("view", 1);
    fx.gh_fail("list", 1);

    let outcome = fx.validate(&as_refs(&resumed(&fx)));

    assert!(
        !outcome.field("terminal_reason").trim().is_empty(),
        "the unread facts must be named\n{}",
        outcome.describe()
    );
    assert_ne!(outcome.field("terminal_state"), "IMPLEMENTED_VERIFIED");
}

// The outcome collect-finalization-evidence recorded is what the validator
// reads on a healthy run. Every leaf it classifies on must be readable: a leaf
// outside the field allowlist reports invalid context, which would strip the
// pull request from a confirmed outcome and sink a finished run.
#[test]
fn a_recorded_outcome_is_classified_without_reading_the_remote_again() {
    let fx = Fixture::new();
    let evidence = serde_json::json!({
        "schema_version": 1,
        "git": {"dirty_worktree": "false"},
        "tooling": {"missing": "", "gh_required": "true"},
        "prior_terminal_state": {"terminal_state": ""},
        "agent_outputs": {"hollow_success_signals": "unknown"},
        "outcome": {
            "schema_version": 1,
            "branch": BRANCH,
            "local_head": fx.head(),
            "remote": REPO,
            "github_origin": "true",
            "branch_pushed": "true",
            "remote_head": "",
            "pr_number": "7",
            "pr_url": PR_URL,
            "pr_state": "OPEN",
            "pr_head": fx.head(),
            "pr_head_relation": "local",
            "checks": "pending",
            "verdict": "SUCCESS",
            "gap": "",
        },
    })
    .to_string();
    let mut envs = finished_env(&fx.repo_path());
    envs.push(("RECIPE_VAR_finalization_evidence", evidence));

    let outcome = fx.validate(&as_refs(&envs));

    assert_hands_pr_on(&outcome);
    fx.assert_gh_not_called("a recorded outcome is the read; it is not read again");
}

fn resumed(fx: &Fixture) -> Vec<(&'static str, String)> {
    crate::issue_1641_finalization_open_pr::builders::resumed_env(&fx.repo_path())
}
