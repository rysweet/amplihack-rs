//! The end state of a finalization run.
//!
//! `assess-finalization` decides the terminal state from the collected facts;
//! `validate-agentic-finalization` reports that decision. These cases drive the
//! whole step and assert what it ends as. Nothing here reads the text of a
//! shell file, and nothing here asserts an intermediate classification,
//! because there no longer is one.
//!
//! What they can and cannot show follows from that split. Deciding the state
//! is an LLM call, so a case that needs a decision supplies one and the state
//! it supplies is a control, not a result: what is under test is everything
//! else -- which facts reach the record, which blockers precede the decision,
//! which decisions are published at all, and what a lost context file costs.
//! No case here is named for an outcome it was handed.
//!
//! Two cases below still read as outcome tests even so -- the diverged pull
//! request and the nothing-pushed run inject `FAILED_IMPLEMENTATION` and
//! assert it comes back. What they really show is the reporting, which their
//! comments say; #2844 tracks either renaming them or testing the decision
//! where it is made, against `workflow-finalization-assessment.yaml`.

use super::fixture_ext::*;
use crate::issue_1641_finalization_open_pr::builders::{
    as_refs, assessment, assessment_raw, green_rollup,
};
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
fn pending_checks_do_not_stop_the_decided_success_from_naming_its_pull_request() {
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
fn a_context_file_carrying_the_assessment_publishes_the_state_it_decided() {
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
fn a_run_with_no_context_file_at_all_still_publishes_the_decided_success() {
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
fn a_blocked_ci_decision_is_reported_against_the_failing_check_and_the_request() {
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

// What this shows is the reporting, not the classification: the decision is
// supplied, and what is asserted is that the result is reported against the
// facts the run really produced -- both heads reach the record, and the facts
// the assessment was given carry the divergence. Which state those facts
// deserve is `assess-finalization`'s, and no test here can stand in for it.
#[test]
fn a_failure_decided_on_a_diverged_pull_request_is_reported_against_both_heads() {
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
fn a_failure_decided_with_nothing_pushed_names_the_branch_and_no_pull_request() {
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

// #2819 R1: `EXISTING_OPEN_PR` is defined as a pull request open at the run's
// head. A decision that reports it while naming no pull request has shown
// nothing, so finalization refuses to publish it: that is no decision, and the
// run goes to the recovery agent with the reason quoting what came back. This
// is not the validator judging the facts -- it never asks whether the pull
// request should exist, only whether the decision named the one it asserts.
#[test]
fn an_open_pull_request_success_that_names_no_pull_request_is_no_decision() {
    for returned in ["", "abc", "7 OR 1=1", "0"] {
        let fx = Fixture::new();
        fx.with_remote();
        fx.gh_no_pr();

        let outcome = fx.validate(&as_refs(&decided_resumed(
            &fx,
            "EXISTING_OPEN_PR",
            returned,
        )));

        assert!(
            !outcome.success(),
            "pr_number `{returned}`: a success naming no pull request must not be published\n{}",
            outcome.describe()
        );
        assert_eq!(
            outcome.field("terminal_state"),
            "FAILED_MISSING_TERMINAL_EVIDENCE",
            "pr_number `{returned}`\n{}",
            outcome.describe()
        );
        assert_eq!(outcome.field("terminal_success"), "false");
        assert_eq!(outcome.field("finalizer_confidence"), "low");
        assert_eq!(outcome.field("pr_number"), "");
        let reason = outcome.field("terminal_reason");
        assert!(
            reason.contains("EXISTING_OPEN_PR") && reason.contains("pr_number"),
            "the reason must quote what was returned: {reason}"
        );
        if !returned.is_empty() {
            assert!(
                reason.contains(returned),
                "the reason must quote `{returned}`: {reason}"
            );
        }
    }
}

// The same decision, with the pull request the facts really carry named back:
// that is the case the refusal above must not touch.
#[test]
fn the_same_success_is_published_when_the_decision_names_the_pull_request() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());

    assert_hands_pr_on(&fx.validate(&as_refs(&decided(&fx, "EXISTING_OPEN_PR", "7"))));
}

// #2819 R3: and when that number came back as a JSON number rather than a
// string. `gh` reports `"number": 7`, so an agent told to read the pull
// request back and report its number returns the integer it was shown. A type
// gate in bash would read that as a wrong type, reach the caller as an empty
// value, and refuse a correct decision with a reason -- "it returned an empty
// pr_number" -- that is not true of what came back. Every leaf of the answer
// is read the same way, so the other three are here too.
#[test]
fn a_decision_whose_leaves_are_json_numbers_or_booleans_is_read_as_written() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());
    let mut envs = finished_env(&fx.repo_path());
    envs.push(assessment_raw(serde_json::json!({
        "terminal_state": "EXISTING_OPEN_PR",
        "action_taken": "none",
        "pr_number": 7,
        "reason": "gh pr view 7 reports the request open at the run's head",
        "next_action": "Wait for CI on PR #7.",
    })));

    let outcome = fx.validate(&as_refs(&envs));

    assert_hands_pr_on(&outcome);
    assert!(
        !outcome.stderr.contains(INVALID_CONTEXT_WARNING),
        "a number is a value, not invalid context\n{}",
        outcome.describe()
    );
}

// The same leniency, on the other side of the transport: a recorded outcome
// whose `pr_number` is a JSON number is the pull request the facts carry, not
// an unreadable field.
#[test]
fn a_recorded_outcome_whose_pr_number_is_a_json_number_is_read_as_written() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());
    let mut envs = finished_env(&fx.repo_path());
    envs.push((
        "RECIPE_VAR_finalization_evidence",
        serde_json::json!({
            "schema_version": 1,
            "git": {"dirty_worktree": "false"},
            "tooling": {"missing": "", "gh_required": "false"},
            "outcome": {"branch": BRANCH, "local_head": fx.head(), "remote": REPO,
                        "remote_head": fx.head(), "pr_number": 7, "pr_state": "OPEN",
                        "checks": "green", "unread": ""},
        })
        .to_string(),
    ));
    envs.push(assessment("EXISTING_OPEN_PR", "none", "7"));

    let outcome = fx.validate(&as_refs(&envs));

    assert_hands_pr_on(&outcome);
    assert!(
        outcome.field("evidence_used").contains("outcome.pr=7"),
        "the recorded number must reach the record: {}",
        outcome.field("evidence_used")
    );
}

// A leaf that is a container still has no scalar to render, so it stays a
// wrong type and the decision is refused -- the leniency is about how a value
// was written, never about what the allowlist admits.
#[test]
fn a_decision_whose_pr_number_is_a_container_is_still_no_decision() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.gh_no_pr();
    let mut envs = crate::issue_1641_finalization_open_pr::builders::resumed_env(&fx.repo_path());
    envs.push(assessment_raw(serde_json::json!({
        "terminal_state": "EXISTING_OPEN_PR",
        "action_taken": "none",
        "pr_number": [7],
        "reason": "r",
        "next_action": "n",
    })));

    let outcome = fx.validate(&as_refs(&envs));

    assert!(!outcome.success(), "{}", outcome.describe());
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_MISSING_TERMINAL_EVIDENCE",
        "{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("pr_number"), "");
}

// The successes that are not about a pull request are untouched: they assert
// that there is none to name.
#[test]
fn a_success_that_asserts_no_pull_request_is_published_without_one() {
    for state in ["IMPLEMENTED_VERIFIED", "ALLOW_NO_OP", "NO_DIFF_SUCCESS"] {
        let fx = Fixture::new();
        fx.with_remote();
        fx.gh_no_pr();

        let outcome = fx.validate(&as_refs(&decided_resumed(&fx, state, "")));

        assert!(outcome.success(), "{state}: {}", outcome.describe());
        assert_eq!(outcome.field("terminal_state"), state);
        assert_eq!(outcome.field("terminal_success"), "true");
        assert_eq!(outcome.field("pr_number"), "");
    }
}

// The ordering guarantee, as behaviour: a decision is never applied to facts
// that were not gathered. Without a recorded worktree the outcome cannot be
// read at all, and the success the assessment decided is not published.
#[test]
fn a_decision_is_not_published_when_the_facts_could_not_be_gathered() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());
    // No REPO_PATH and no worktree_setup: nothing says where to read.
    let envs = vec![
        ("BRANCH_NAME", BRANCH.to_owned()),
        ("FINALIZER_STEP_STATUS", "ok".to_owned()),
        assessment("EXISTING_OPEN_PR", "none", "7"),
    ];

    let outcome = fx.validate(&as_refs(&envs));

    assert!(!outcome.success(), "{}", outcome.describe());
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_MISSING_TERMINAL_EVIDENCE"
    );
    assert_eq!(outcome.field("terminal_success"), "false");
    assert!(
        outcome
            .field("terminal_reason")
            .contains("could not be gathered"),
        "the reason must name the gap: {}",
        outcome.field("terminal_reason")
    );
    assert!(
        outcome
            .field("evidence_used")
            .contains("outcome=ungathered"),
        "the record must say the facts were never gathered: {}",
        outcome.field("evidence_used")
    );
}

// #2819 R2: a context file that was selected and carried nothing says so,
// once, naming the path and why. Without that line a consumer downstream
// reports only that its producer "produced no output" and nothing anywhere
// names the file. The status is unchanged -- absence, not a failure -- so the
// run still ends on its own facts.
#[test]
fn a_context_file_that_carried_nothing_says_so_and_still_does_not_end_the_run() {
    for (file, write) in [("absent/context.json", false), ("context.json", true)] {
        let fx = Fixture::new();
        fx.with_remote();
        fx.push();
        fx.gh_pr_at(&fx.head(), green_rollup());
        let path = fx.tmp.path().join(file);
        if write {
            std::fs::write(&path, "{ not json").expect("write");
        }
        let mut envs = decided(&fx, "EXISTING_OPEN_PR", "7");
        envs.push(("AMPLIHACK_CONTEXT_FILE", path.display().to_string()));

        let outcome = fx.validate(&as_refs(&envs));

        assert!(
            outcome.stderr.contains(&path.display().to_string()),
            "{file}: the warning must name the path that carried nothing\n{}",
            outcome.describe()
        );
        assert!(
            outcome.stderr.contains("AMPLIHACK_CONTEXT_FILE"),
            "{file}: the warning must name the variable\n{}",
            outcome.describe()
        );
        assert!(
            !outcome.stderr.contains(INVALID_CONTEXT_WARNING),
            "{file}: it is absence, not invalid data\n{}",
            outcome.describe()
        );
        assert!(
            outcome
                .field("evidence_used")
                .contains("outcome.checks=green"),
            "{file}: the run's own facts are still read: {}",
            outcome.field("evidence_used")
        );
    }
}

// Nothing selected is not a diagnostic.
#[test]
fn a_run_that_selected_no_context_file_is_told_nothing_about_one() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());

    let outcome = fx.validate(&as_refs(&decided(&fx, "EXISTING_OPEN_PR", "7")));

    assert!(
        !outcome.stderr.contains("AMPLIHACK_CONTEXT_FILE"),
        "an unselected context file is not a diagnostic\n{}",
        outcome.describe()
    );
}

// Typed completion booleans and finalizer prose are not evidence of anything.
// They cannot reach a success on their own, and they cannot overturn the
// decision that was made. The decision is held fixed and they are varied, so
// the result moving would be the classifier #2343 removed coming back.
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
            "schema_version": 3,
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
