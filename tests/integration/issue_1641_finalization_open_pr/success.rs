//! Success paths: a resumed run whose branch already has an open, same-repo PR
//! with a passing verification signal is `EXISTING_OPEN_PR`, not
//! `FAILED_IMPLEMENTATION`, even though this run's implementation/verification
//! steps left no outputs (PRs #1635 and #1639).

use serde_json::json;

use super::fixture::*;

const EVIDENCE_GREEN: &str = "publish_state_reached=true,pr_state=OPEN,pr_head_matches_branch=true,pr_head_sha_matches=true,pr_number=7,required_checks=green";
const EVIDENCE_PRECOMMIT: &str = "publish_state_reached=true,pr_state=OPEN,pr_head_matches_branch=true,pr_head_sha_matches=true,pr_number=7,precommit_passed=true";

fn assert_existing_open_pr(outcome: &Outcome, evidence: &str) {
    assert!(
        outcome.success(),
        "resumed run with an open PR and a passing verification signal must succeed\n{}",
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
    assert_eq!(outcome.field("evidence_used"), evidence);
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
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));

    let outcome = fx.validate(&as_refs(&envs));

    assert_existing_open_pr(&outcome, EVIDENCE_GREEN);
    let calls = fx.gh_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("pr view 7 ")
            && c.contains(&format!("--repo {REPO}"))
            && c.contains("--json")
            && [
                "number",
                "url",
                "state",
                "headRefName",
                "headRefOid",
                "isCrossRepository",
                "statusCheckRollup"
            ]
            .iter()
            .all(|f| c.contains(f))),
        "validator must inspect the PR read-only with --repo and the documented fields: {calls:?}"
    );
    fx.assert_gh_reads_only();
    assert_no_leak(&outcome);
}

#[test]
fn resumed_run_with_only_a_pr_number_succeeds() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_NUMBER", "7".to_owned()));

    assert_existing_open_pr(&fx.validate(&as_refs(&envs)), EVIDENCE_GREEN);
    fx.assert_gh_reads_only();
}

// The GitHub publish path records lowercase `existing-open-pr` and no URL, so
// the validator must find the PR with a read-only `pr list --head <branch>`.
#[test]
fn lowercase_existing_open_pr_status_without_url_uses_pr_list_fallback() {
    for (alias, status) in [
        ("TERMINAL_STATE_PUBLISH_STATUS", "existing-open-pr"),
        ("PR_PUBLISH_RESULT_STATE", "existing-open-pr"),
        ("TERMINAL_STATE_PUBLISH_STATUS", "EXISTING_OPEN_PR"),
        ("TERMINAL_STATE_PUBLISH_STATUS", "FOLLOWUP_CREATED"),
        ("TERMINAL_STATE_TERMINAL_STATE", "existing-open-pr"),
    ] {
        let fx = Fixture::new();
        fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
        fx.gh_view(&open_pr(green_rollup()));
        let mut envs = resumed_env(&fx.repo_path());
        envs.push((alias, status.to_owned()));

        let outcome = fx.validate(&as_refs(&envs));

        assert!(
            outcome.success(),
            "{alias}={status}\n{}",
            outcome.describe()
        );
        assert_existing_open_pr(&outcome, EVIDENCE_GREEN);
        let calls = fx.gh_calls();
        assert!(
            calls.iter().any(|c| c.starts_with("pr list ")
                && c.contains(&format!("--head {BRANCH}"))
                && c.contains("--state open")),
            "{alias}={status}: publish status alone must search open PRs for the run branch: {calls:?}"
        );
        assert!(
            calls.iter().any(|c| c.starts_with("pr view 7 ")),
            "{alias}={status}: the single match must be re-checked with pr view: {calls:?}"
        );
        fx.assert_gh_reads_only();
    }
}

// Checks not green, but this run's typed precommit verification passed.
#[test]
fn non_green_checks_with_typed_verification_evidence_succeed_as_precommit_passed() {
    for rollup in [
        json!([check_run("IN_PROGRESS", json!(null))]),
        json!([check_run("COMPLETED", json!("FAILURE"))]),
        json!([]),
    ] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(rollup.clone()));
        let mut envs = resumed_env(&fx.repo_path());
        envs.push(("PR_URL", PR_URL.to_owned()));
        envs.push(("VERIFICATION_COMPLETED", "true".to_owned()));

        let outcome = fx.validate(&as_refs(&envs));
        assert!(outcome.success(), "rollup {rollup}\n{}", outcome.describe());
        assert_existing_open_pr(&outcome, EVIDENCE_PRECOMMIT);
    }
}

#[test]
fn neutral_skipped_and_success_status_contexts_count_as_green() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(json!([
        check_run("COMPLETED", json!("SUCCESS")),
        check_run("COMPLETED", json!("NEUTRAL")),
        check_run("COMPLETED", json!("SKIPPED")),
        status_context("SUCCESS"),
    ])));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));

    assert_existing_open_pr(&fx.validate(&as_refs(&envs)), EVIDENCE_GREEN);
}

// Production runs carry context as RECIPE_VAR_* (canonical tier), the shape the
// #1635/#1639 runs had: publish status in the terminal_state object, no URL.
#[test]
fn canonical_recipe_var_transport_reaches_the_open_pr_path() {
    let fx = Fixture::new();
    fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
    fx.gh_view(&open_pr(green_rollup()));
    let terminal_state = json!({"publish_status": "existing-open-pr"}).to_string();
    let repo_path = fx.repo_path();

    let outcome = fx.validate(&[
        ("RECIPE_VAR_repo_path", repo_path.as_str()),
        ("RECIPE_VAR_branch_name", BRANCH),
        ("RECIPE_VAR_terminal_state", terminal_state.as_str()),
        ("RECIPE_VAR_finalizer_step_status", r#"{"status":"ok"}"#),
    ]);

    assert_existing_open_pr(&outcome, EVIDENCE_GREEN);
}

// The canonical result schema is unchanged: 19 keys, every value a String.
#[test]
fn existing_open_pr_result_keeps_the_nineteen_string_key_schema() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = resumed_env(&fx.repo_path());
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

// Successful runs must not leave retry scratch files in the worktree, which
// would dirty it for the next run.
#[test]
fn open_pr_lookup_leaves_the_worktree_clean() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));

    assert!(fx.validate(&as_refs(&envs)).success());
    assert_eq!(
        fx.git(&["status", "--porcelain"]),
        "",
        "worktree must stay clean"
    );
    assert!(
        !path_has_retry_scratch(&fx.repo),
        "no retry scratch in the worktree"
    );
    assert!(
        fs_is_empty(&fx.tmp.path().join("scratch")),
        "the private mktemp dir must be removed"
    );
}

fn fs_is_empty(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_none())
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
