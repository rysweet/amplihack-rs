//! Fail-closed cases the validator exists for. Since #2343 they divide in two.
//!
//! A pull request that is not this run's is a matter of what the facts reader
//! reports: it records what `gh` printed and never widens the lookup, so a
//! closed, foreign or absent pull request simply is not there to decide on.
//! Blockers about the local checkout still end the run before anything is read
//! and before any assessment is consulted, so no decision can mask them.

use serde_json::json;

use super::fixture::*;

fn with_url(fx: &Fixture) -> Vec<(&'static str, String)> {
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs
}

// Core regression for #1641 (must still hold): nothing published, no evidence.
// Since #2343 the check no longer waits for the run to claim it published
// something before looking, so the fixture publishes nothing at all: no open
// PR for the branch, and no PR to view.
#[test]
fn run_with_no_pr_and_no_evidence_is_still_failed_implementation() {
    let fx = Fixture::new();
    fx.gh_list(&json!([]));
    fx.gh_fail("view", 1);

    let outcome = fx.validate(&as_refs(&assessed_env(
        &fx.repo_path(),
        "FAILED_IMPLEMENTATION",
        "",
    )));

    assert_failed_implementation(&outcome, "nothing published");
    assert_eq!(outcome.field("publish_state_reached"), "false");
    assert_eq!(outcome.field("pr_number"), "");
    assert_eq!(outcome.field("pr_url"), "");
    let evidence = outcome.field("evidence_used");
    assert!(
        evidence.contains("outcome.pr=none"),
        "the facts must say there was no pull request: {evidence}"
    );
    fx.assert_gh_reads_only();
}

// The same run with nothing deciding it: finalization reports that it reached
// no terminal state, which leaves its step failing for the recovery agent
// rather than inventing an outcome.
#[test]
fn a_run_with_nothing_published_and_no_assessment_reaches_no_decision() {
    let fx = Fixture::new();
    fx.gh_list(&json!([]));
    fx.gh_fail("view", 1);

    let outcome = fx.validate(&as_refs(&resumed_env(&fx.repo_path())));

    assert_no_decision(&outcome, "nothing published, nothing decided");
    assert_eq!(outcome.field("publish_state_reached"), "false");
    assert_eq!(outcome.field("pr_number"), "");
}

// Guards that must hold before gh is ever called.
#[test]
fn unusable_run_scope_is_never_read_and_never_decided() {
    // Target directory fell back to '.': cwd is the repo, but REPO_PATH unset.
    // The facts are not gathered at all, so no assessment can be applied to
    // them -- the decision is read only after the facts are in hand.
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let (_, assessment_json) = assessment("EXISTING_OPEN_PR", "none", "7");
    let outcome = fx.validate(&[
        ("BRANCH_NAME", BRANCH),
        ("PR_URL", PR_URL),
        ("FINALIZER_STEP_STATUS", "ok"),
        (
            "RECIPE_VAR_finalization_assessment",
            assessment_json.as_str(),
        ),
    ]);
    assert!(!outcome.success(), "{}", outcome.describe());
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_MISSING_TERMINAL_EVIDENCE",
        "ungathered facts cannot be decided on\n{}",
        outcome.describe()
    );
    fx.assert_gh_not_called("'.' target fallback");

    // Metadata branch disagrees with the live branch (#1762): nothing is read
    // for a branch the checkout is not on.
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = with_url(&fx);
    set_branch(&mut envs, "feat/other");
    let outcome = fx.validate(&as_refs(&envs));
    assert_no_decision(&outcome, "branch mismatch");
    assert!(
        outcome
            .field("terminal_reason")
            .contains("not on the run branch"),
        "the reason must name the branch mismatch\n{}",
        outcome.describe()
    );
    fx.assert_gh_not_called("branch mismatch");
}

// What used to be the last case of the test above: a run branch named `main`
// or `master`. The facts reader blanked those three names, which refused the
// read before gh was asked -- a decision taken in the collector about what
// the assessment is there to decide, and one that never covered a repository
// whose default branch is `trunk` (#2819). The branch is now read like any
// other, and what still holds is the part that was doing the work: reading
// facts decides nothing, so with no assessment there is no decision, whatever
// the branch is called.
#[test]
fn a_default_branch_name_is_read_and_still_decides_nothing() {
    for branch in ["main", "master"] {
        let fx = Fixture::on_branch(branch);
        let mut view = open_pr(green_rollup());
        view["headRefName"] = json!(branch);
        fx.gh_view(&view);
        let mut envs = with_url(&fx);
        set_branch(&mut envs, branch);

        let outcome = fx.validate(&as_refs(&envs));

        assert_no_decision(&outcome, branch);
        assert!(
            outcome
                .field("evidence_used")
                .contains(&format!("outcome.branch={branch}")),
            "the branch must reach the facts rather than be refused by name: {}",
            outcome.field("evidence_used")
        );
        fx.assert_gh_reads_only();
    }
}

// Every blocker about the local checkout keeps its state, is reached before
// anything is read, and cannot be overridden by an assessment that claims a
// success.
#[test]
fn blockers_keep_their_state_and_are_not_masked_by_a_claimed_success() {
    let claimed = assessment("EXISTING_OPEN_PR", "none", "7");
    let cases: [(&str, (&'static str, String), &str); 3] = [
        (
            "collected dirty worktree",
            evidence_env("true", "", "false"),
            "FAILED_DIRTY_WORKTREE",
        ),
        (
            "missing tooling",
            evidence_env("false", "git", "false"),
            "FAILED_MISSING_TOOLING",
        ),
        (
            "prior terminal probe",
            (
                "RECIPE_VAR_terminal_state",
                json!({"terminal_state": "BLOCKED_CI", "pr_number": "7"}).to_string(),
            ),
            "BLOCKED_CI",
        ),
    ];
    for (name, context, expected) in cases {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(green_rollup()));
        let mut envs = with_url(&fx);
        envs.push(context);
        envs.push(claimed.clone());
        let outcome = fx.validate(&as_refs(&envs));
        assert!(!outcome.success(), "{name}\n{}", outcome.describe());
        assert_eq!(
            outcome.field("terminal_state"),
            expected,
            "{name}: a claimed success must not mask the blocker\n{}",
            outcome.describe()
        );
        fx.assert_gh_not_called(name);
    }

    // A live dirty worktree (no collected evidence) is also a blocker.
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    std::fs::write(fx.repo.join("untracked.txt"), "dirty\n").expect("dirty file");
    let mut envs = with_url(&fx);
    envs.push(claimed);
    let outcome = fx.validate(&as_refs(&envs));
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_DIRTY_WORKTREE",
        "{}",
        outcome.describe()
    );
    fx.assert_gh_not_called("live dirty worktree");
}

// #2343: hollow-success signals and a failed reporting step are facts about
// this run's own reporting, which a published pull request can speak to. They
// no longer end the run on their own; they are carried into the result and
// handed to the assessment, which decides.
#[test]
fn hollow_success_and_reporting_failure_are_carried_as_facts_not_swallowed() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = with_url(&fx);
    envs.push(evidence_env("false", "", "true"));
    envs.push(assessment("HOLLOW_SUCCESS", "none", ""));
    let outcome = fx.validate(&as_refs(&envs));
    assert!(!outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "HOLLOW_SUCCESS");
    assert_eq!(
        outcome.field("hollow_success_detected"),
        "true",
        "the signal must still be reported\n{}",
        outcome.describe()
    );

    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = with_url(&fx);
    envs.push((
        "RECIPE_VAR_finalizer_step_status",
        json!({"status": "failed", "reporting_failure": "true"}).to_string(),
    ));
    envs.push(assessment("FAILED_REPORTING", "none", "7"));
    let outcome = fx.validate(&as_refs(&envs));
    assert!(!outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "FAILED_REPORTING");
    assert_eq!(
        outcome.field("reporting_failure"),
        "true",
        "the typed reporting status must still be reported\n{}",
        outcome.describe()
    );
    assert!(
        outcome
            .field("evidence_used")
            .contains("reporting_failure=true"),
        "the fact must travel with the decision: {}",
        outcome.field("evidence_used")
    );
}

// The publish-state override is internal; the environment cannot spoof it, and
// it is set only from a pull request the facts report open.
#[test]
fn ambient_publish_state_override_cannot_spoof_publication() {
    let fx = Fixture::new();
    fx.gh_list(&json!([]));
    fx.gh_fail("view", 1);
    let mut envs = assessed_env(&fx.repo_path(), "FAILED_IMPLEMENTATION", "");
    envs.push(("publish_state_override", "true".to_owned()));
    envs.push(("PUBLISH_STATE_OVERRIDE", "true".to_owned()));

    let outcome = fx.validate(&as_refs(&envs));

    assert_failed_implementation(&outcome, "spoofed override");
    assert_eq!(outcome.field("publish_state_reached"), "false");
    // #2343: the facts are read whether or not the run claims to have
    // published. What a claim may not do is stand in for the answer.
    fx.assert_gh_reads_only();
}
