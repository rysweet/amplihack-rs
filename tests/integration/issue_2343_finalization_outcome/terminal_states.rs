//! The end state of a finalization run.
//!
//! `assess-finalization` decides the terminal state from the collected facts;
//! `validate-agentic-finalization` reports that decision. These cases drive the
//! whole step and assert what it ends as. Nothing here reads the text of a
//! shell file, and nothing here asserts an intermediate classification,
//! because there no longer is one.

use super::fixture_ext::*;
use crate::issue_1641_finalization_open_pr::builders::{as_refs, assessment, green_rollup};
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

/// A failure whose reason the fixture's own facts appear in.
fn assert_fails_naming(outcome: &Outcome, state: &str, facts: &[&str]) {
    assert!(!outcome.success(), "must fail\n{}", outcome.describe());
    assert_eq!(
        outcome.field("terminal_state"),
        state,
        "{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_success"), "false");
    assert_eq!(outcome.field("terminal_failure"), "true");
    assert!(
        !outcome.stderr.contains(INVALID_CONTEXT_WARNING),
        "the gap must be the real one, not the context file\n{}",
        outcome.describe()
    );
    let evidence = outcome.field("evidence_used");
    for fact in facts {
        assert!(
            evidence.contains(fact),
            "evidence_used must carry `{fact}`: {evidence}\n{}",
            outcome.describe()
        );
    }
}

// #2784: a pull request open at the pushed head whose checks have not finished
// is finished work waiting on CI. It is the deputy's first outcome case: the
// run ends a success that hands the pull request on, named.
#[test]
fn a_pending_ci_pull_request_ends_as_a_hand_on_success() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), pending_rollup());

    let outcome = fx.validate(&as_refs(&decided(&fx, "EXISTING_OPEN_PR", "7")));

    assert_hands_pr_on(&outcome);
    assert_eq!(outcome.field("publish_state_reached"), "true");
    assert_ne!(outcome.field("terminal_state"), "FAILED_IMPLEMENTATION");
    fx.assert_gh_reads_only();
}

// #1726: a context file that is absent, or present and unreadable, is absence
// of bookkeeping, not invalidity. The run's facts are still gathered and the
// result still names the real gap. What a lost file does cost is the
// assessment it was carrying, so the run ends with no decision -- and the
// reason says exactly that, rather than blaming the context file.
#[test]
fn a_lost_context_file_still_gathers_the_facts_and_names_the_real_gap() {
    for file in ["absent/context.json", "context.json"] {
        let fx = Fixture::new();
        fx.with_remote();
        fx.push();
        fx.gh_pr_at(&fx.head(), green_rollup());
        let path = fx.tmp.path().join(file);
        if file == "context.json" {
            std::fs::write(&path, "{ not json").expect("write");
        }
        let mut envs = decided(&fx, "EXISTING_OPEN_PR", "7");
        envs.push(("AMPLIHACK_CONTEXT_FILE", path.display().to_string()));

        let outcome = fx.validate(&as_refs(&envs));

        assert!(
            !outcome.stderr.contains(INVALID_CONTEXT_WARNING),
            "{file}: absence must not be reported as invalid context\n{}",
            outcome.describe()
        );
        let evidence = outcome.field("evidence_used");
        assert!(
            evidence.contains("outcome.pr=7") && evidence.contains("outcome.checks=green"),
            "{file}: the run's facts are still gathered: {evidence}"
        );
        assert_eq!(
            outcome.field("terminal_state"),
            "FAILED_MISSING_TERMINAL_EVIDENCE",
            "{file}: a lost file loses the decision it carried\n{}",
            outcome.describe()
        );
        assert!(
            outcome
                .field("terminal_reason")
                .contains("no terminal state"),
            "{file}: the reason must name the missing decision: {}",
            outcome.field("terminal_reason")
        );
    }
}

// The production shape of the same transport: the context FILE carries the
// assessment, and the run ends as the state it decided.
#[test]
fn a_context_file_carrying_the_assessment_ends_the_run_as_that_state() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());
    let file = fx.tmp.path().join("context.json");
    std::fs::write(
        &file,
        serde_json::json!({
            "repo_path": fx.repo_path(),
            "branch_name": BRANCH,
            "pr_url": PR_URL,
            "finalizer_step_status": r#"{"status":"ok","reporting_failure":"false"}"#,
            "finalization_assessment": serde_json::json!({
                "terminal_state": "EXISTING_OPEN_PR",
                "action_taken": "none",
                "pr_number": "7",
                "reason": "pull request #7 is open at the run head",
                "next_action": "Watch CI on PR #7.",
            })
            .to_string(),
        })
        .to_string(),
    )
    .expect("write context file");

    let outcome = fx.validate(&[("AMPLIHACK_CONTEXT_FILE", file.to_str().expect("path"))]);

    assert_hands_pr_on(&outcome);
}

#[test]
fn a_run_with_no_context_file_at_all_ends_as_a_success() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());

    assert_hands_pr_on(&fx.validate(&as_refs(&decided(&fx, "EXISTING_OPEN_PR", "7"))));
}

// The run's own verification boolean is not consulted any more (#2784): the
// same facts and the same decision give the same end state either way.
#[test]
fn the_end_state_does_not_depend_on_verification_completed() {
    for flag in ["true", "false"] {
        let fx = Fixture::new();
        fx.with_remote();
        fx.push();
        fx.gh_pr_at(&fx.head(), pending_rollup());
        let mut envs = decided(&fx, "EXISTING_OPEN_PR", "7");
        envs.push(("VERIFICATION_COMPLETED", flag.to_owned()));

        assert_hands_pr_on(&fx.validate(&as_refs(&envs)));
    }
}

#[test]
fn a_failing_check_ends_blocked_ci_and_names_the_pull_request() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), failing_rollup());

    let outcome = fx.validate(&as_refs(&decided(&fx, "BLOCKED_CI", "7")));

    assert_fails_naming(
        &outcome,
        "BLOCKED_CI",
        &["outcome.checks=failing", "outcome.pr=7"],
    );
    assert_eq!(outcome.field("pr_number"), "7");
    assert_eq!(outcome.field("pr_url"), PR_URL);
}

// A pull request at a commit unrelated to the local head. The facts carry both
// heads, so the failure the assessment decides is reported against them.
#[test]
fn a_pull_request_at_an_unrelated_head_ends_failed_implementation() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    let local = fx.head();
    let other = fx.unrelated_commit();
    fx.gh_pr_at(&other, green_rollup());

    let outcome = fx.validate(&as_refs(&decided(&fx, "FAILED_IMPLEMENTATION", "7")));

    assert_fails_naming(&outcome, "FAILED_IMPLEMENTATION", &[&local]);
    let (value, _) = fx.facts(BRANCH, Some(PR_URL));
    assert_eq!(value["pr_head"], other, "the facts carry the PR's own head");
    assert_eq!(value["pr_head_descends_local_head"], "false");
}

#[test]
fn a_branch_that_was_never_pushed_ends_failed_implementation_naming_the_branch() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.gh_no_pr();
    let local = fx.head();

    let outcome = fx.validate(&as_refs(&decided_resumed(&fx, "FAILED_IMPLEMENTATION", "")));

    assert_fails_naming(
        &outcome,
        "FAILED_IMPLEMENTATION",
        &[
            BRANCH,
            &local,
            "outcome.remote_head=none",
            "outcome.pr=none",
        ],
    );
    assert_eq!(
        outcome.field("pr_number"),
        "",
        "there is no pull request to name"
    );
}

// Typed completion booleans and finalizer prose are not evidence of anything.
// They cannot reach a success on their own, and they cannot overturn the
// decision that was made.
#[test]
fn typed_success_booleans_and_prose_cannot_overturn_the_decision() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.gh_no_pr();
    let mut envs = decided_resumed(&fx, "FAILED_IMPLEMENTATION", "");
    envs.push(("IMPLEMENTATION_COMPLETED", "true".to_owned()));
    envs.push(("VERIFICATION_COMPLETED", "true".to_owned()));
    envs.push((
        "AGENTIC_FINALIZER_NARRATIVE",
        "pushed, PR open, CI green".to_owned(),
    ));

    let outcome = fx.validate(&as_refs(&envs));

    assert!(!outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "FAILED_IMPLEMENTATION");
    assert_ne!(outcome.field("terminal_success"), "true");
}

// A blocker about the local checkout is answered before the facts are even
// gathered, so no decision can mask it.
#[test]
fn a_dirty_worktree_beats_an_assessment_that_claims_a_success() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());
    std::fs::write(fx.repo.join("dirty.txt"), "uncommitted\n").expect("write");

    let outcome = fx.validate(&as_refs(&decided(&fx, "EXISTING_OPEN_PR", "7")));

    assert!(!outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "FAILED_DIRTY_WORKTREE");
    assert_eq!(outcome.field("terminal_success"), "false");
}

// The recorded outcome is the read on a healthy run. Every leaf the validator
// carries must be readable from it: a leaf outside the field allowlist reports
// invalid context, which would strip the pull request from the result.
#[test]
fn a_recorded_outcome_is_reported_without_reading_the_remote_again() {
    let fx = Fixture::new();
    let evidence = serde_json::json!({
        "schema_version": 1,
        "git": {"dirty_worktree": "false"},
        "tooling": {"missing": "", "gh_required": "true"},
        "prior_terminal_state": {"terminal_state": ""},
        "agent_outputs": {"hollow_success_signals": "unknown"},
        "outcome": {
            "schema_version": 2,
            "branch": BRANCH,
            "local_head": fx.head(),
            "remote": REPO,
            "github_origin": "true",
            "remote_head": fx.head(),
            "remote_head_descends_local_head": "true",
            "open_pr_count": "1",
            "pr_number": "7",
            "pr_url": PR_URL,
            "pr_state": "OPEN",
            "pr_draft": "false",
            "pr_head": fx.head(),
            "pr_cross_repository": "false",
            "pr_head_descends_local_head": "true",
            "checks": "pending",
            "unread": "",
        },
    })
    .to_string();
    let mut envs = decided(&fx, "EXISTING_OPEN_PR", "7");
    envs.push(("RECIPE_VAR_finalization_evidence", evidence));

    let outcome = fx.validate(&as_refs(&envs));

    assert_hands_pr_on(&outcome);
    assert!(
        outcome
            .field("evidence_used")
            .contains("outcome.checks=pending"),
        "the recorded facts must reach the record: {}",
        outcome.field("evidence_used")
    );
    fx.assert_gh_not_called("a recorded outcome is the read; it is not read again");
}

// The assessment supplies the state and the words; it never supplies the URL.
// The canonical one is built for the remote the facts report.
#[test]
fn the_pull_request_url_is_built_from_the_facts_not_from_the_assessment() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());
    let mut envs = finished_env(&fx.repo_path());
    envs.push(assessment("EXISTING_OPEN_PR", "none", "7"));

    let outcome = fx.validate(&as_refs(&envs));

    assert_hands_pr_on(&outcome);
    assert_eq!(outcome.field("pr_url"), PR_URL);
}

// The canonical result schema is unchanged: 19 keys, every value a String.
#[test]
fn the_result_keeps_the_nineteen_string_key_schema() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());

    let outcome = fx.validate(&as_refs(&decided(&fx, "EXISTING_OPEN_PR", "7")));

    assert!(outcome.success(), "{}", outcome.describe());
    let result = outcome.json();
    let object = result.as_object().expect("result is a JSON object");
    assert_eq!(object.len(), 19, "result must keep 19 keys: {result}");
    assert!(
        object.values().all(serde_json::Value::is_string),
        "every result value must be a String: {result}"
    );
}
