//! A failing check on the run's open PR is a negative CI signal. The facts
//! reader classes the rollup as `failing`. Since #2966 a failing check is
//! repaired before finalization decides, so `BLOCKED_CI` is the state the
//! assessment reports only for a cause the run cannot fix, and the validator
//! reports it as given -- whatever this run's own `verification_completed`
//! says, which neither the reader nor the validator consults since #2343. An
//! unclassifiable check is not "CI has not reported yet" either: it reads as
//! `unknown`, and local pre-commit evidence can never turn it into a success.

use serde_json::json;

use super::fixture::*;

fn failing_rollups() -> Vec<serde_json::Value> {
    vec![
        json!([check_run("COMPLETED", json!("FAILURE"))]),
        json!([
            check_run("COMPLETED", json!("SUCCESS")),
            check_run("COMPLETED", json!("CANCELLED"))
        ]),
        json!([status_context("FAILURE")]),
        json!([status_context("ERROR")]),
        json!([
            check_run("IN_PROGRESS", json!(null)),
            check_run("COMPLETED", json!("TIMED_OUT"))
        ]),
    ]
}

// Every shape of red check reads as `failing`, and nothing about this run can
// change that reading: the reader is handed the worktree, the branch and the
// pull request reference, and nothing else.
#[test]
fn every_failing_rollup_reads_as_a_failing_check() {
    for rollup in failing_rollups() {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(rollup.clone()));

        let (facts, _) = fx.facts(BRANCH, Some(PR_URL));

        assert_eq!(facts["checks"], "failing", "rollup {rollup}");
        assert_eq!(facts["pr_state"], "OPEN", "rollup {rollup}");
        assert_eq!(facts["pr_number"], "7", "rollup {rollup}");
        fx.assert_gh_reads_only();
    }
}

#[test]
fn blocked_ci_is_reported_whatever_this_run_claims_about_verification() {
    for rollup in failing_rollups() {
        for verification in ["", "true"] {
            let fx = Fixture::new();
            fx.gh_view(&open_pr(rollup.clone()));
            let mut envs = assessed_env(&fx.repo_path(), "BLOCKED_CI", "7");
            envs.push(("PR_URL", PR_URL.to_owned()));
            if !verification.is_empty() {
                envs.push(("VERIFICATION_COMPLETED", verification.to_owned()));
            }
            let why = format!("rollup {rollup}, verification_completed={verification:?}");

            let outcome = fx.validate(&as_refs(&envs));

            assert!(
                !outcome.success(),
                "{why}: must fail closed\n{}",
                outcome.describe()
            );
            assert_eq!(
                outcome.field("terminal_state"),
                "BLOCKED_CI",
                "{why}\n{}",
                outcome.describe()
            );
            assert_eq!(outcome.field("terminal_success"), "false", "{why}");
            assert_eq!(outcome.field("terminal_failure"), "true", "{why}");
            assert_eq!(outcome.field("pr_number"), "7", "{why}");
            assert_eq!(outcome.field("pr_url"), PR_URL, "{why}");
            assert_ne!(
                outcome.field("required_next_action"),
                "No action required.",
                "{why}"
            );
            // #2343: the run's own verification boolean is not among the facts
            // a red check is read on; the failing rollup is.
            let evidence = outcome.field("evidence_used");
            assert!(
                evidence.contains("outcome.checks=failing")
                    && evidence.contains("outcome.pr=7")
                    && evidence.contains("assessment_state=BLOCKED_CI"),
                "{why}: evidence_used must carry the failing check and the decision: {evidence}"
            );
            assert!(
                !evidence.contains("verification_completed=true"),
                "{why}: a red check is not read on this run's own pre-commit evidence: {evidence}"
            );
            assert!(
                outcome
                    .stderr
                    .contains("workflow finalization failed closed: BLOCKED_CI"),
                "{why}\n{}",
                outcome.describe()
            );
            fx.assert_gh_reads_only();
        }
    }
}

// An unreadable check class is an unread fact, never "green enough".
#[test]
fn an_unclassifiable_check_reads_as_unknown_not_as_green() {
    for rollup in [
        json!([{"__typename": "MysteryCheck", "state": "SUCCESS"}]),
        json!([check_run("COMPLETED", json!(null))]),
    ] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(rollup.clone()));

        assert_eq!(
            fx.fact(BRANCH, Some(PR_URL), "checks"),
            "unknown",
            "rollup {rollup} must read as unknown"
        );
    }
}

// And typed pre-commit evidence cannot stand in for the decision that unread
// fact needs: with nothing judging it, finalization reports that it has no
// terminal state rather than borrowing a success from this run's own claims.
#[test]
fn an_unclassifiable_check_with_typed_verification_evidence_is_no_success() {
    for rollup in [
        json!([{"__typename": "MysteryCheck", "state": "SUCCESS"}]),
        json!([check_run("COMPLETED", json!(null))]),
    ] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(rollup.clone()));
        let mut envs = resumed_env(&fx.repo_path());
        envs.push(("PR_URL", PR_URL.to_owned()));
        envs.push(("VERIFICATION_COMPLETED", "true".to_owned()));
        envs.push(("IMPLEMENTATION_COMPLETED", "true".to_owned()));

        let outcome = fx.validate(&as_refs(&envs));

        assert_no_decision(&outcome, &format!("rollup {rollup}"));
        let evidence = outcome.field("evidence_used");
        assert!(
            evidence.contains("outcome.checks=unknown"),
            "rollup {rollup}: the unread check class must be carried: {evidence}"
        );
    }
}
