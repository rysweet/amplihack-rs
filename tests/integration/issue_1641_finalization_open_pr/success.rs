//! Success paths: a resumed run whose branch already has an open, same-repo PR
//! is `EXISTING_OPEN_PR`, not `FAILED_IMPLEMENTATION`, even though this run's
//! implementation/verification steps left no outputs (PRs #1635 and #1639).
//!
//! Since #2343 the work is split in two. Reading the pull request is
//! `finalization_outcome_facts`, which reports what `gh` printed and judges
//! none of it; the cases about HOW the pull request is read are therefore
//! asserted against the facts. Turning those facts into `EXISTING_OPEN_PR` is
//! `assess-finalization`'s decision, which the validator reports; the cases
//! about the terminal state supply that decision and assert the end state.

use serde_json::json;

use super::fixture::*;

/// The `--json` fields the facts reader asks `gh pr view` for.
pub(super) const VIEW_FIELDS: [&str; 8] = [
    "number",
    "url",
    "state",
    "isDraft",
    "headRefName",
    "headRefOid",
    "isCrossRepository",
    "statusCheckRollup",
];

pub(super) fn assert_existing_open_pr(outcome: &Outcome, checks: &str) {
    assert!(
        outcome.success(),
        "a decided EXISTING_OPEN_PR must succeed\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    assert_eq!(outcome.field("terminal_success"), "true");
    assert_eq!(outcome.field("terminal_failure"), "false");
    assert_eq!(outcome.field("finalizer_output_valid"), "true");
    assert_eq!(outcome.field("hollow_success_detected"), "false");
    assert_eq!(outcome.field("reporting_failure"), "false");
    assert_eq!(outcome.field("publish_state_reached"), "true");
    assert_eq!(outcome.field("pr_number"), "7");
    assert_eq!(outcome.field("pr_url"), PR_URL);
    // The facts the state was reported against travel with it, so a reader can
    // see the pull request and its check class behind the decision.
    let evidence = outcome.field("evidence_used");
    for fact in [
        "outcome.pr=7",
        "outcome.pr_state=OPEN",
        &format!("outcome.checks={checks}"),
        "assessment_state=EXISTING_OPEN_PR",
    ] {
        assert!(
            evidence.contains(fact),
            "evidence_used must carry `{fact}`: {evidence}\n{}",
            outcome.describe()
        );
    }
    assert!(
        !outcome.stderr.contains("failed closed"),
        "success must not announce a fail-closed result\n{}",
        outcome.describe()
    );
}

// Core regression for #1641: PR published, checks green, and the run has no
// implementation_completed / verification_completed outputs at all.
#[test]
fn resumed_run_with_open_pr_and_green_checks_is_not_failed_implementation() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = assessed_env(&fx.repo_path(), "EXISTING_OPEN_PR", "7");
    envs.push(("PR_URL", PR_URL.to_owned()));

    let outcome = fx.validate(&as_refs(&envs));

    assert_existing_open_pr(&outcome, "green");
    assert_no_leak(&outcome);
}

#[test]
fn resumed_run_with_only_a_pr_number_succeeds() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = assessed_env(&fx.repo_path(), "EXISTING_OPEN_PR", "7");
    envs.push(("PR_NUMBER", "7".to_owned()));

    assert_existing_open_pr(&fx.validate(&as_refs(&envs)), "green");
    fx.assert_gh_reads_only();
}

// CI has not reported yet (pending or no checks). #2784, #2343: a published PR
// at the run's head whose checks have not reported is finished work waiting on
// CI, and this run's own verification boolean is not consulted at all -- not by
// the facts reader, which never sees it, and not by the validator.
#[test]
fn pending_or_absent_checks_are_read_as_such_and_hand_the_pr_on() {
    for (rollup, checks) in [
        (json!([check_run("IN_PROGRESS", json!(null))]), "pending"),
        (
            json!([
                check_run("COMPLETED", json!("SUCCESS")),
                status_context("PENDING")
            ]),
            "pending",
        ),
        (json!([]), "none"),
    ] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(rollup.clone()));
        assert_eq!(
            fx.fact(BRANCH, Some(PR_URL), "checks"),
            checks,
            "rollup {rollup} must read as {checks}"
        );

        for verification in ["", "true", "false"] {
            let fx = Fixture::new();
            fx.gh_view(&open_pr(rollup.clone()));
            let mut envs = assessed_env(&fx.repo_path(), "EXISTING_OPEN_PR", "7");
            envs.push(("PR_URL", PR_URL.to_owned()));
            if !verification.is_empty() {
                envs.push(("VERIFICATION_COMPLETED", verification.to_owned()));
            }

            let outcome = fx.validate(&as_refs(&envs));
            assert!(outcome.success(), "rollup {rollup}\n{}", outcome.describe());
            assert_existing_open_pr(&outcome, checks);
        }
    }
}

// Production runs carry context as RECIPE_VAR_* (canonical tier), the shape the
// #1635/#1639 runs had: publish status in the terminal_state object, no URL.
#[test]
fn canonical_recipe_var_transport_reaches_the_facts_and_the_decision() {
    let fx = Fixture::new();
    fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
    fx.gh_view(&open_pr(green_rollup()));
    let terminal_state = json!({"publish_status": "existing-open-pr"}).to_string();
    let repo_path = fx.repo_path();
    let (_, assessment_json) = assessment("EXISTING_OPEN_PR", "none", "7");

    let outcome = fx.validate(&[
        ("RECIPE_VAR_repo_path", repo_path.as_str()),
        ("RECIPE_VAR_branch_name", BRANCH),
        ("RECIPE_VAR_terminal_state", terminal_state.as_str()),
        ("RECIPE_VAR_finalizer_step_status", r#"{"status":"ok"}"#),
        (
            "RECIPE_VAR_finalization_assessment",
            assessment_json.as_str(),
        ),
    ]);

    assert_existing_open_pr(&outcome, "green");
}

// The canonical result schema is unchanged: 19 keys, every value a String.
#[test]
fn existing_open_pr_result_keeps_the_nineteen_string_key_schema() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = assessed_env(&fx.repo_path(), "EXISTING_OPEN_PR", "7");
    envs.push(("PR_URL", PR_URL.to_owned()));

    let outcome = fx.validate(&as_refs(&envs));
    assert!(outcome.success(), "{}", outcome.describe());
    let result = outcome.json();
    let object = result.as_object().expect("result is a JSON object");
    assert_eq!(object.len(), 19, "result must keep 19 keys: {result}");
    assert!(
        object.values().all(serde_json::Value::is_string),
        "every result value must be a String: {result}"
    );
}

// workflow-complete must list the new success state.
#[test]
fn terminal_vocabulary_includes_existing_open_pr() {
    let fx = Fixture::new();
    let outcome = fx.run_dispatcher(
        "complete",
        &[
            ("TASK_DESCRIPTION", "resumed run"),
            ("ISSUE_NUMBER", "1641"),
            (
                "RECIPE_VAR_workflow_result__terminal_state",
                "EXISTING_OPEN_PR",
            ),
            ("RECIPE_VAR_workflow_result__terminal_success", "true"),
            ("RECIPE_VAR_workflow_result__terminal_failure", "false"),
            ("RECIPE_VAR_workflow_result__reporting_failure", "false"),
            ("RECIPE_VAR_workflow_result__finalizer_output_valid", "true"),
        ],
    );
    assert!(outcome.success(), "{}", outcome.describe());
    let vocabulary = outcome.json()["terminal_vocabulary"].clone();
    assert!(
        vocabulary
            .as_array()
            .is_some_and(|v| v.iter().any(|s| s == "EXISTING_OPEN_PR")),
        "terminal_vocabulary must include EXISTING_OPEN_PR: {vocabulary}"
    );
}
