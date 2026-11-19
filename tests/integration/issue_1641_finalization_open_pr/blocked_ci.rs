//! A failing check on the run's open PR is a negative CI signal. It fails
//! closed as `BLOCKED_CI` (the validator's existing CI-failure vocabulary)
//! whatever this run's own `verification_completed` says: local pre-commit
//! evidence never overrides a red check. An unclassifiable check is not
//! "CI has not reported yet" either, so it never rides on pre-commit evidence.

use serde_json::json;

use super::fixture::*;

const FACTS: &str = "publish_state_reached=true,pr_state=OPEN,pr_head_matches_branch=true,pr_head_sha_matches=true,pr_number=7";

#[test]
fn failing_check_is_blocked_ci_even_with_typed_verification_evidence() {
    for rollup in [
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
    ] {
        for verification in ["", "true"] {
            let fx = Fixture::new();
            fx.gh_view(&open_pr(rollup.clone()));
            let mut envs = resumed_env(&fx.repo_path());
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
            // #2343: the run's own verification boolean is not consulted at
            // all, so it is no longer among the facts a red check is read on.
            assert_eq!(
                outcome.field("evidence_used"),
                format!("{FACTS},all_checks=failing"),
                "{why}"
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

#[test]
fn unclassifiable_check_with_typed_verification_evidence_fails_closed() {
    for rollup in [
        json!([{"__typename": "MysteryCheck", "state": "SUCCESS"}]),
        json!([check_run("COMPLETED", json!(null))]),
    ] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(rollup.clone()));
        let mut envs = resumed_env(&fx.repo_path());
        envs.push(("PR_URL", PR_URL.to_owned()));
        envs.push(("VERIFICATION_COMPLETED", "true".to_owned()));

        let outcome = fx.validate(&as_refs(&envs));

        // #2343: an unreadable check class is an unread fact, and an unread
        // fact against a GitHub origin is a gap that names itself, never a
        // success borrowed from this run's own pre-commit evidence.
        assert_failed_implementation(&outcome, &format!("rollup {rollup}"));
        let reason = outcome.field("terminal_reason");
        assert!(
            reason.contains("check") && reason.contains('7'),
            "rollup {rollup}: the reason must name the unread check state of the PR\n{}",
            outcome.describe()
        );
    }
}
