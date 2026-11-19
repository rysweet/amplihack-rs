//! Which terminal states finalization hands to the merge-ready loop, asserted
//! by running the validator rather than by reading its source (#1714, #2343).
//!
//! The brick `workflow_finalization_handoff.sh` is the one definition of the
//! handed-off set. This file proves the validator uses exactly that set and no
//! other, from the outside: every state the finaliser can report is driven
//! through a real run with the hand-off flag on and a pull request available,
//! and the result either carries `handoff: merge_ready_loop` or fails closed
//! with no `handoff` key at all.

use super::fixture::*;

/// Every state `workflow_finalization_complete.sh` publishes that finalization
/// itself can reach, with whether the brick hands it off.
const STATES: &[(&str, bool)] = &[
    ("BLOCKED_CI", true),
    ("FAILED_IMPLEMENTATION", true),
    ("FAILED_REPORTING", true),
    ("EXISTING_OPEN_PR", false),
    ("IMPLEMENTED_VERIFIED", false),
    ("ALLOW_NO_OP", false),
    ("NO_DIFF_SUCCESS", false),
    ("HOLLOW_SUCCESS", false),
    ("FAILED_MEANINGFUL_DIFF", false),
    ("FAILED_CLOSED_UNMERGED", false),
    ("FAILED_PR_METADATA_UNAVAILABLE", false),
];

/// A run on a pull request the facts can read, whose assessment decided
/// `state`, inside auto-drive.
fn run(state: &str) -> (Fixture, Outcome) {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs.push(assessment(state, "none", "7"));
    let mut refs = as_refs(&envs);
    refs.push(("RECIPE_VAR_finalization_handoff", "true"));
    let outcome = validate(&fx, &refs);
    (fx, outcome)
}

// T6: exactly BLOCKED_CI, FAILED_IMPLEMENTATION and FAILED_REPORTING are
// handed to the merge-ready loop, and nothing else is, flag or no flag.
#[test]
fn exactly_the_bricks_three_states_are_handed_to_the_merge_ready_loop() {
    for (state, handed) in STATES {
        let (_fx, outcome) = run(state);
        assert_eq!(
            outcome.field("terminal_state"),
            *state,
            "the validator reports the state the assessment decided\n{}",
            outcome.describe()
        );
        if *handed {
            assert!(
                outcome.success(),
                "{state}: a hand-off exits 0\n{}",
                outcome.describe()
            );
            assert_eq!(
                outcome.field("handoff"),
                "merge_ready_loop",
                "{state} is one of the brick's hand-off states\n{}",
                outcome.describe()
            );
            assert_eq!(outcome.field("terminal_success"), "false", "{state}");
            assert_eq!(outcome.field("pr_number"), "7", "{state}");
            assert!(
                outcome.stderr.contains(&format!(
                    "INFO: #1714: {state} handed to the merge-ready loop (PR #7)"
                )),
                "{state}: stderr names the hand-off\n{}",
                outcome.describe()
            );
        } else {
            assert_no_handoff_key(&outcome, state);
        }
    }
}

// Every non-success state that is not handed off still ends the run with
// exit 1, so a failure can never be mistaken for a hand-off.
#[test]
fn states_outside_the_brick_set_still_fail_closed() {
    for (state, handed) in STATES {
        if *handed {
            continue;
        }
        let (_fx, outcome) = run(state);
        let success_state = matches!(
            *state,
            "EXISTING_OPEN_PR" | "IMPLEMENTED_VERIFIED" | "ALLOW_NO_OP" | "NO_DIFF_SUCCESS"
        );
        if success_state {
            assert!(
                outcome.success(),
                "{state} is a success\n{}",
                outcome.describe()
            );
            assert_eq!(outcome.field("terminal_success"), "true", "{state}");
        } else {
            assert_eq!(
                outcome.output.status.code(),
                Some(1),
                "{state} must fail closed\n{}",
                outcome.describe()
            );
            assert_eq!(outcome.field("terminal_success"), "false", "{state}");
            assert!(
                outcome
                    .stderr
                    .contains("workflow finalization failed closed"),
                "{state}: stderr announces the fail-closed exit\n{}",
                outcome.describe()
            );
        }
    }
}

// Without the flag, not one of the three is handed off: a standalone
// default-workflow run still fails closed on every failure.
#[test]
fn no_state_is_handed_off_without_the_flag() {
    for (state, handed) in STATES {
        if !*handed {
            continue;
        }
        let fx = Fixture::new();
        fx.gh_view(&open_pr(green_rollup()));
        let mut envs = resumed_env(&fx.repo_path());
        envs.push(("PR_URL", PR_URL.to_owned()));
        envs.push(assessment(state, "none", "7"));
        let outcome = validate(&fx, &as_refs(&envs));
        assert_eq!(
            outcome.output.status.code(),
            Some(1),
            "{state} without the flag\n{}",
            outcome.describe()
        );
        assert_eq!(outcome.field("terminal_state"), *state);
        assert_no_handoff_key(&outcome, state);
        assert!(
            !outcome.stderr.contains("handed to the merge-ready loop"),
            "{state}\n{}",
            outcome.describe()
        );
    }
}

// The hand-off needs a pull request to hand on: with neither the assessment
// nor the facts naming one, even a hand-off state fails closed.
#[test]
fn a_handoff_state_with_no_pull_request_fails_closed() {
    for (state, handed) in STATES {
        if !*handed {
            continue;
        }
        let fx = Fixture::new();
        fx.gh_list(&serde_json::json!([]));
        fx.gh_fail("view", 1);
        let mut envs = resumed_env(&fx.repo_path());
        envs.push(assessment(state, "none", ""));
        let mut refs = as_refs(&envs);
        refs.push(("RECIPE_VAR_finalization_handoff", "true"));
        let outcome = validate(&fx, &refs);
        assert_eq!(
            outcome.output.status.code(),
            Some(1),
            "{state} with no pull request\n{}",
            outcome.describe()
        );
        assert_eq!(outcome.field("pr_number"), "", "{state}");
        assert_no_handoff_key(&outcome, state);
    }
}
