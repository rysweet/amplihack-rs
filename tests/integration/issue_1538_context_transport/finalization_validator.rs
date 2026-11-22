//! What the validator reports through the file and canonical transports.
//!
//! The terminal state is the one `assess-finalization` decided (#2343, #2809).
//! The typed completion booleans still travel, from their own records, and the
//! result still reports them, but they no longer derive a state. Each case
//! carries a decision through the transport it exercises and asserts that the
//! decision arrives intact, that the flags beside it come from the right
//! record, and that a stale or forged copy cannot beat the canonical one.
use super::assessed::{assessed, clean_evidence, completion, decided};
use super::fixtures::*;
use serde_json::{Value, json};

/// Validate, require the exit code, and require the terminal state and its
/// success flag to agree with it. Returns the result for further assertions.
fn validate(f: Fixture, state: &str, code: i32) -> Value {
    let r = f.finalizer("validate");
    r.exit(code);
    let j = r.json();
    assert_eq!(j["terminal_state"], state, "stderr={}", r.stderr);
    assert_eq!(
        j["terminal_success"],
        if code == 0 { "true" } else { "false" }
    );
    j
}

#[test]
fn validator_file_completion_records_match_collector() {
    for (positive, state, code) in [
        (true, "IMPLEMENTED_VERIFIED", 0),
        (false, "FAILED_IMPLEMENTATION", 1),
    ] {
        let context = assessed(completion(positive), state, "");
        let collected = Fixture::new().file(context.clone()).finalizer("collect");
        collected.exit(0);
        let j = validate(Fixture::new().file(context), state, code);
        for key in [
            "implementation_completed",
            "verification_completed",
            "terminal_no_op",
        ] {
            assert_eq!(j[key], collected.json()["completion"][key], "{key}");
        }
        assert_eq!(j["implementation_completed"], positive.to_string());
        assert_eq!(j["verification_completed"], positive.to_string());
    }
}

#[test]
fn validator_file_negative_beats_stale_completion_and_noop() {
    let j = validate(
        Fixture::new()
            .file(assessed(completion(false), "FAILED_IMPLEMENTATION", ""))
            .env("IMPLEMENTATION_COMPLETED", "true")
            .env("VERIFICATION_COMPLETED", "true")
            .env("TERMINAL_NO_OP", "true")
            .env("ALLOW_NO_OP", "true")
            // A stale decision beside the file's own, claiming a success.
            .env(
                "RECIPE_VAR_finalization_assessment",
                decided("ALLOW_NO_OP", "").to_string(),
            ),
        "FAILED_IMPLEMENTATION",
        1,
    );
    for key in [
        "implementation_completed",
        "verification_completed",
        "terminal_no_op",
    ] {
        assert_eq!(
            j[key], "false",
            "the file's record must beat the stale {key}"
        );
    }
}

// The no-op state is the assessment's; bash no longer requires both flags for
// it. What survives is the transport. The two flags still travel through the
// file, each from its own record, and are read exactly as the collector reads
// them. A `terminal_no_op` in the verification record is not the
// implementation's, so it is not reported as one, and the state follows the
// assessment either way.
#[test]
fn validator_file_noop_flags_travel_and_the_assessment_names_the_state() {
    for (records, state, reported_no_op) in [
        (
            json!({"allow_no_op":true,"implementation_terminal_evidence":{"terminal_no_op":true}}),
            "ALLOW_NO_OP",
            "true",
        ),
        (
            json!({"allow_no_op":true,"verification_terminal_evidence":{"terminal_no_op":true}}),
            "NO_DIFF_SUCCESS",
            "false",
        ),
    ] {
        let context = assessed(records, state, "");
        let collected = Fixture::new().file(context.clone()).finalizer("collect");
        collected.exit(0);
        let j = validate(Fixture::new().file(context), state, 0);
        assert_eq!(j["terminal_no_op"], reported_no_op);
        assert_eq!(
            collected.json()["completion"]["terminal_no_op"],
            reported_no_op
        );
        assert_eq!(collected.json()["completion"]["allow_no_op"], "true");
    }
}

#[test]
fn validator_canonical_root_negative_beats_stale_completion() {
    let j = validate(
        Fixture::new()
            .env(
                "RECIPE_VAR_implementation_terminal_evidence",
                json!({"implementation_completed":false}).to_string(),
            )
            .env(
                "RECIPE_VAR_finalization_evidence",
                clean_evidence("").to_string(),
            )
            .env(
                "RECIPE_VAR_finalization_assessment",
                decided("FAILED_IMPLEMENTATION", "").to_string(),
            )
            // Stale: the flat aliases, and a nested scalar under the root.
            .env("IMPLEMENTATION_COMPLETED", "true")
            .env("VERIFICATION_COMPLETED", "true")
            .env(
                "RECIPE_VAR_finalization_assessment__terminal_state",
                "IMPLEMENTED_VERIFIED",
            ),
        "FAILED_IMPLEMENTATION",
        1,
    );
    assert_eq!(j["implementation_completed"], "false");
    assert_eq!(j["verification_completed"], "false");
}

#[test]
fn validator_file_reporting_failure_retains_failed_reporting_split() {
    let mut context = assessed(completion(true), "FAILED_REPORTING", "");
    context["finalizer_step_status"] = json!({"status":"failed","reporting_failure":true});
    let j = validate(Fixture::new().file(context), "FAILED_REPORTING", 1);
    // The split: the work is done and verified, only the report failed.
    assert_eq!(j["reporting_failure"], "true");
    assert_eq!(j["implementation_completed"], "true");
    assert_eq!(j["verification_completed"], "true");
    assert!(
        j["evidence_used"]
            .as_str()
            .unwrap()
            .contains("reporting_failure=true")
    );
}

#[test]
fn validator_legacy_hard_blockers_are_not_bypassed_by_completion() {
    for (evidence, state) in [
        (
            json!({"git":{"dirty_worktree":"true"}}),
            "FAILED_DIRTY_WORKTREE",
        ),
        (
            json!({"tooling":{"missing":"jq"}}),
            "FAILED_MISSING_TOOLING",
        ),
        (
            json!({"prior_terminal_state":{"terminal_state":"FAILED_MEANINGFUL_DIFF"}}),
            "FAILED_MEANINGFUL_DIFF",
        ),
    ] {
        validate(
            Fixture::new()
                .env("FINALIZATION_EVIDENCE", evidence.to_string())
                .env("IMPLEMENTATION_COMPLETED", "true")
                .env("VERIFICATION_COMPLETED", "true"),
            state,
            1,
        );
    }
    // #2343: evidence that cannot be read is no longer a terminal state of its
    // own. It is still never bypassed by positive completion: the run fails,
    // and the reason names what could not be read.
    let r = Fixture::new()
        .env("FINALIZATION_EVIDENCE", "{bad")
        .env("IMPLEMENTATION_COMPLETED", "true")
        .env("VERIFICATION_COMPLETED", "true")
        .finalizer("validate");
    r.exit(1);
    assert_eq!(r.json()["terminal_success"], "false");
    assert_ne!(r.json()["terminal_state"], "IMPLEMENTED_VERIFIED");
}

// The modern form of the same attack. The legacy aliases cannot carry an
// assessment, so a decision that claims a success travels through the file or
// the canonical root. A blocker about the local checkout ends the run before
// the outcome is read, so neither transport lets the claim past it.
#[test]
fn validator_hard_blockers_are_not_bypassed_by_a_claimed_success_assessment() {
    for (blocker, state) in [
        (
            json!({"git":{"dirty_worktree":"true"}}),
            "FAILED_DIRTY_WORKTREE",
        ),
        (
            json!({"tooling":{"missing":"git"}}),
            "FAILED_MISSING_TOOLING",
        ),
        (
            json!({"prior_terminal_state":{"terminal_state":"FAILED_MEANINGFUL_DIFF"}}),
            "FAILED_MEANINGFUL_DIFF",
        ),
    ] {
        for claim in ["IMPLEMENTED_VERIFIED", "ALLOW_NO_OP"] {
            let mut evidence = clean_evidence("");
            evidence
                .as_object_mut()
                .unwrap()
                .extend(blocker.as_object().unwrap().clone());
            let mut context = assessed(completion(true), claim, "");
            context["finalization_evidence"] = evidence.clone();
            let canonical = Fixture::new()
                .env("RECIPE_VAR_finalization_evidence", evidence.to_string())
                .env(
                    "RECIPE_VAR_finalization_assessment",
                    decided(claim, "").to_string(),
                );
            for f in [Fixture::new().file(context), canonical] {
                let j = validate(f, state, 1);
                assert_eq!(j["terminal_failure"], "true");
            }
        }
    }
}

// Hollow success is no longer an exit of its own (#2809): the signal is a
// typed fact that travels in the evidence and is reported, and the assessment
// names HOLLOW_SUCCESS when it is what happened. Positive completion records
// beside the signal change neither what is reported nor what was decided.
#[test]
fn validator_hollow_success_signal_travels_and_the_assessment_names_the_state() {
    for (hollow, state, code) in [
        (true, "HOLLOW_SUCCESS", 1),
        (false, "IMPLEMENTED_VERIFIED", 0),
    ] {
        let mut context = assessed(completion(true), state, "");
        context["finalization_evidence"]["agent_outputs"] =
            json!({"hollow_success_signals": hollow});
        let j = validate(Fixture::new().file(context), state, code);
        assert_eq!(j["hollow_success_detected"], hollow.to_string());
        assert_eq!(j["implementation_completed"], "true");
    }
}

#[test]
fn validator_file_dirty_blocker_cannot_be_overridden_by_stale_clean_evidence() {
    let mut context = completion(true);
    context["finalization_evidence"] = json!({"git":{"dirty_worktree":"true"}});
    validate(
        Fixture::new()
            .file(context)
            .env("FINALIZATION_EVIDENCE", "{}")
            .env("IMPLEMENTATION_COMPLETED", "true")
            .env("VERIFICATION_COMPLETED", "true"),
        "FAILED_DIRTY_WORKTREE",
        1,
    );
}
