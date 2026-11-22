//! #2966: `BLOCKED_CI` is reached only through the assessment.
//!
//! A red check is repaired before finalization decides, so the terminal-state
//! probe no longer reports `BLOCKED_CI` and the validator no longer answers it
//! from the probe's record. What remains is the state `assess-finalization`
//! reports for a check that fails, or cannot run, for a cause the run cannot
//! fix: a missing secret, a run awaiting maintainer approval, a setting only an
//! owner can change. That state names the cause and the exact step a person
//! must take, and it ends the run through the existing hand-off path. These
//! cases run the real validator and assert:
//!
//! - AC4: a stale record of the old probe's `BLOCKED_CI` decides nothing. With
//!   a green head and an assessment that says so, the run ends a success.
//! - AC5: an assessed `BLOCKED_CI` puts the cause and the human step in the
//!   result and on the failure line, once, and ends. Nothing here loops.
//! - C6 row 5: with the hand-off flag but no pull request number, it is still
//!   not handed off.

use serde_json::{Value, json};

use crate::issue_1641_finalization_open_pr::builders::{
    assert_no_decision, assessment_raw, check_run_named,
};

use super::fixture::*;
use super::validator::{assert_failed_closed, assert_handed_off, with_flag};

const CAUSE: &str =
    "deploy: the workflow reads the secret DEPLOY_TOKEN, which the repository does not define";
const HUMAN_STEP: &str = "repository administrator: add the DEPLOY_TOKEN secret under Settings, Secrets and variables, Actions, then re-run the failed deploy check";

/// The run as the recipe runner hands it over: the probe's record in
/// `terminal_state`, and the assessment, when there is one, beside it.
fn canonical(fx: &Fixture, probe_record: &Value, assessed: Option<&str>, flag: bool) -> Outcome {
    let repo = fx.repo_path();
    let record = probe_record.to_string();
    let mut envs = vec![
        ("RECIPE_VAR_repo_path", repo.as_str()),
        ("RECIPE_VAR_branch_name", BRANCH),
        ("RECIPE_VAR_pr_url", PR_URL),
        ("RECIPE_VAR_terminal_state", record.as_str()),
        ("RECIPE_VAR_finalizer_step_status", r#"{"status":"ok"}"#),
    ];
    if let Some(assessed) = assessed {
        envs.push(("RECIPE_VAR_finalization_assessment", assessed));
    }
    if flag {
        envs.push(("RECIPE_VAR_finalization_handoff", "true"));
    }
    validate(fx, &envs)
}

/// What the old probe wrote on a red head, still sitting in a resumed run's
/// recorded context.
fn stale_probe_record() -> Value {
    json!({
        "terminal_state": "BLOCKED_CI",
        "terminal_reason": "real failing checks block terminal success",
        "pr_number": "7",
        "pr_url": PR_URL,
    })
}

/// A run whose assessment judged BLOCKED_CI for `pr`, with its cause and the
/// human step, on a head where the `deploy` check fails.
fn unfixable_env(fx: &Fixture, pr: &str) -> Vec<(&'static str, String)> {
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
        "reason": CAUSE,
        "next_action": HUMAN_STEP,
    })));
    envs
}

// AC4. A resumed run can still carry the old probe's BLOCKED_CI in its record.
// The record is a reading of a head that has since been repaired, so it
// decides nothing: the facts are read afresh and the assessment's decision on
// them is reported, whether or not the run is inside auto-drive.
#[test]
fn a_stale_probe_blocked_ci_is_not_answered_from_the_record() {
    for flag in [false, true] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(green_rollup()));
        let assessed = assessment("EXISTING_OPEN_PR", "repaired_ci", "7").1;

        let outcome = canonical(&fx, &stale_probe_record(), Some(&assessed), flag);

        assert!(
            outcome.success(),
            "flag={flag}: a repaired head ends a success\n{}",
            outcome.describe()
        );
        assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
        assert_eq!(outcome.field("terminal_success"), "true");
        assert_eq!(outcome.field("terminal_failure"), "false");
        assert_eq!(outcome.field("pr_number"), "7");
        assert_no_handoff_key(&outcome, "a success needs no hand-off");
        assert!(
            !fx.gh_calls().is_empty(),
            "flag={flag}: the facts must be read live, not taken from the stale record"
        );
        fx.assert_gh_reads_only();
    }
}

// AC4, the other side: the stale record is not a decision either. With nothing
// assessed, finalization reports that it has no decision, which its recovery
// agent closes with the reason, and it is never handed to the auto-drive loops
// as a CI failure.
#[test]
fn a_stale_probe_blocked_ci_with_no_assessment_is_no_decision_and_is_never_handed_off() {
    for flag in [false, true] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(green_rollup()));

        let outcome = canonical(&fx, &stale_probe_record(), None, flag);

        assert_no_decision(&outcome, &format!("flag={flag}"));
        assert_no_handoff_key(&outcome, &format!("flag={flag}"));
    }
}

// AC4, through the evidence the run collected: the same record, held in
// `finalization_evidence.prior_terminal_state`, is not read as a blocker.
#[test]
fn a_stale_blocked_ci_in_the_collected_evidence_is_not_a_blocker() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let repo = fx.repo_path();
    let evidence = json!({
        "schema_version": 1,
        "git": {"dirty_worktree": "false"},
        "tooling": {"missing": "", "gh_required": "true"},
        "prior_terminal_state": {"terminal_state": "BLOCKED_CI"},
        "agent_outputs": {"hollow_success_signals": "false"},
    })
    .to_string();
    let assessed = assessment("EXISTING_OPEN_PR", "repaired_ci", "7").1;

    let outcome = validate(
        &fx,
        &[
            ("RECIPE_VAR_repo_path", repo.as_str()),
            ("RECIPE_VAR_branch_name", BRANCH),
            ("RECIPE_VAR_pr_url", PR_URL),
            ("RECIPE_VAR_finalizer_step_status", r#"{"status":"ok"}"#),
            ("RECIPE_VAR_finalization_evidence", evidence.as_str()),
            ("RECIPE_VAR_finalization_assessment", assessed.as_str()),
        ],
    );

    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
}

// AC5, standalone. The cause and the human step are the assessment's words.
// They reach the result and the failure line, which is the text step recovery
// excerpts and a person reads, and the run ends there: one validator call, one
// failure line, exit 1.
#[test]
fn an_unfixable_blocked_ci_carries_the_cause_and_the_human_step() {
    let fx = Fixture::new();
    let envs = unfixable_env(&fx, "7");

    let outcome = validate(&fx, &as_refs(&envs));

    assert_failed_closed(&outcome, "BLOCKED_CI", "standalone");
    assert_eq!(outcome.field("terminal_reason"), CAUSE);
    assert_eq!(outcome.field("required_next_action"), HUMAN_STEP);
    assert_eq!(outcome.field("pr_number"), "7");
    assert_eq!(outcome.field("pr_url"), PR_URL);
    let evidence = outcome.field("evidence_used");
    assert!(
        evidence.contains("outcome.checks=failing")
            && evidence.contains("assessment_state=BLOCKED_CI"),
        "the failing check and the decision are in the record: {evidence}"
    );

    // The one failure line names the state, the cause and the human step, so
    // that the step survives the 2,000-character excerpt step recovery takes.
    let failure_lines: Vec<&str> = outcome
        .stderr
        .lines()
        .filter(|l| l.contains("workflow finalization failed closed"))
        .collect();
    assert_eq!(failure_lines.len(), 1, "{}", outcome.describe());
    assert_eq!(
        failure_lines[0],
        format!(
            "ERROR: workflow finalization failed closed: BLOCKED_CI: {CAUSE}; next action: {HUMAN_STEP}"
        ),
        "{}",
        outcome.describe()
    );
    let excerpt: String = outcome.stderr.chars().take(2000).collect();
    assert!(
        excerpt.contains(HUMAN_STEP),
        "the human step must sit inside the first 2,000 characters\n{}",
        outcome.describe()
    );
    fx.assert_gh_reads_only();
}

// AC5, inside auto-drive: the same decision hands the pull request on with the
// same cause and step in the result, and ends the build without a failure line.
#[test]
fn an_unfixable_blocked_ci_in_auto_drive_hands_the_cause_on_with_the_pull_request() {
    let fx = Fixture::new();
    let envs = unfixable_env(&fx, "7");

    let outcome = validate(&fx, &with_flag(&envs));

    assert_handed_off(&outcome, "7", "auto-drive");
    assert_eq!(outcome.field("terminal_reason"), CAUSE);
    assert_eq!(outcome.field("required_next_action"), HUMAN_STEP);
    fx.assert_gh_reads_only();
}

// C6 row 5. BLOCKED_CI is handed on only for a pull request the assessment
// named. Without a number there is nothing for the loops to review, so the run
// fails closed with the cause and the step, whatever the flag says.
#[test]
fn a_blocked_ci_that_names_no_pull_request_is_never_handed_off() {
    let fx = Fixture::new();
    let envs = unfixable_env(&fx, "");

    let outcome = validate(&fx, &with_flag(&envs));

    assert_failed_closed(&outcome, "BLOCKED_CI", "no pull request named");
    assert_eq!(outcome.field("terminal_reason"), CAUSE);
    assert_eq!(outcome.field("required_next_action"), HUMAN_STEP);
    assert!(
        !outcome.stderr.contains(HANDOFF_INFO),
        "{}",
        outcome.describe()
    );
}
