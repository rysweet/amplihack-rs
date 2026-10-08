use super::fixtures::*;
use serde_json::{Value, json};

fn completion(positive: bool) -> Value {
    json!({"implementation_terminal_evidence":{"implementation_completed":positive,"terminal_no_op":false},
        "verification_terminal_evidence":{"verification_completed":positive},"allow_no_op":false})
}
fn collect(f: &Fixture, implemented: &str, verified: &str, noop: &str, allow: &str) {
    let r = f.finalizer("collect");
    r.exit(0);
    let j = r.json();
    for (key, value) in [
        ("implementation_completed", implemented),
        ("verification_completed", verified),
        ("terminal_no_op", noop),
        ("allow_no_op", allow),
    ] {
        assert_eq!(j["completion"][key], value, "collect field {key}: {j}");
    }
}
fn validate(f: Fixture, state: &str, code: i32) {
    let r = f.finalizer("validate");
    r.exit(code);
    assert_eq!(r.json()["terminal_state"], state);
    assert_eq!(
        r.json()["terminal_success"],
        if code == 0 { "true" } else { "false" }
    );
}

#[test]
fn collector_file_completion_records_are_consumed() {
    collect(
        &Fixture::new().file(completion(true)),
        "true",
        "true",
        "false",
        "false",
    );
}
#[test]
fn collector_canonical_root_completion_records_are_consumed() {
    collect(
        &Fixture::new()
            .env(
                "RECIPE_VAR_implementation_terminal_evidence",
                json!({"implementation_completed":"true"}).to_string(),
            )
            .env(
                "RECIPE_VAR_verification_terminal_evidence",
                json!({"verification_completed":"true"}).to_string(),
            ),
        "true",
        "true",
        "false",
        "false",
    );
}
#[test]
fn collector_nested_negatives_beat_stale_legacy_positives() {
    collect(
        &Fixture::new()
            .env(
                "RECIPE_VAR_implementation_terminal_evidence__implementation_completed",
                "false",
            )
            .env(
                "RECIPE_VAR_verification_terminal_evidence__verification_completed",
                "false",
            )
            .env("IMPLEMENTATION_COMPLETED", "true")
            .env("VERIFICATION_COMPLETED", "true"),
        "false",
        "false",
        "false",
        "false",
    );
}
#[test]
fn collector_file_negatives_beat_stale_legacy_positives() {
    collect(
        &Fixture::new()
            .file(completion(false))
            .env("IMPLEMENTATION_COMPLETED", "true")
            .env("VERIFICATION_COMPLETED", "true")
            .env("TERMINAL_NO_OP", "true")
            .env("ALLOW_NO_OP", "true"),
        "false",
        "false",
        "false",
        "false",
    );
}
#[test]
fn collector_file_positives_beat_stale_legacy_negatives() {
    collect(
        &Fixture::new()
            .file(completion(true))
            .env("IMPLEMENTATION_COMPLETED", "false")
            .env("VERIFICATION_COMPLETED", "false"),
        "true",
        "true",
        "false",
        "false",
    );
}
#[test]
fn collector_root_missing_field_beats_nested_and_legacy_positive() {
    collect(
        &Fixture::new()
            .env("RECIPE_VAR_implementation_terminal_evidence", "{}")
            .env(
                "RECIPE_VAR_implementation_terminal_evidence__implementation_completed",
                "true",
            )
            .env("IMPLEMENTATION_COMPLETED", "true"),
        "false",
        "false",
        "false",
        "false",
    );
}
#[test]
fn collector_legacy_and_nested_only_completion_remain_supported() {
    collect(
        &Fixture::new()
            .env("IMPLEMENTATION_COMPLETED", "true")
            .env("VERIFICATION_COMPLETED", "true"),
        "true",
        "true",
        "false",
        "false",
    );
    collect(
        &Fixture::new()
            .env(
                "RECIPE_VAR_implementation_terminal_evidence__implementation_completed",
                "true",
            )
            .env(
                "RECIPE_VAR_verification_terminal_evidence__verification_completed",
                "true",
            ),
        "true",
        "true",
        "false",
        "false",
    );
}
#[test]
fn collector_file_noop_and_optout_survive_transport() {
    collect(
        &Fixture::new().file(
            json!({"implementation_terminal_evidence":{"terminal_no_op":true},"allow_no_op":true}),
        ),
        "false",
        "false",
        "true",
        "true",
    );
}
#[test]
fn collector_invalid_authoritative_file_never_borrows_completion() {
    for raw in ["{bad", "null", "", "{}"] {
        collect(
            &Fixture::new()
                .file_raw(raw)
                .env("IMPLEMENTATION_COMPLETED", "true")
                .env("VERIFICATION_COMPLETED", "true"),
            "false",
            "false",
            "false",
            "false",
        );
    }
}
#[test]
fn validator_file_completion_records_match_collector() {
    validate(
        Fixture::new().file(completion(true)),
        "IMPLEMENTED_VERIFIED",
        0,
    );
}
#[test]
fn validator_file_negative_beats_stale_completion_and_noop() {
    validate(
        Fixture::new()
            .file(completion(false))
            .env("IMPLEMENTATION_COMPLETED", "true")
            .env("VERIFICATION_COMPLETED", "true")
            .env("TERMINAL_NO_OP", "true")
            .env("ALLOW_NO_OP", "true"),
        "FAILED_IMPLEMENTATION",
        1,
    );
}
#[test]
fn validator_file_noop_requires_both_implementation_noop_and_optout() {
    validate(
        Fixture::new().file(
            json!({"allow_no_op":true,"implementation_terminal_evidence":{"terminal_no_op":true}}),
        ),
        "ALLOW_NO_OP",
        0,
    );
    validate(
        Fixture::new().file(
            json!({"allow_no_op":true,"verification_terminal_evidence":{"terminal_no_op":true}}),
        ),
        "FAILED_IMPLEMENTATION",
        1,
    );
}
#[test]
fn validator_canonical_root_negative_beats_stale_completion() {
    validate(
        Fixture::new()
            .env(
                "RECIPE_VAR_implementation_terminal_evidence",
                json!({"implementation_completed":false}).to_string(),
            )
            .env("IMPLEMENTATION_COMPLETED", "true")
            .env("VERIFICATION_COMPLETED", "true"),
        "FAILED_IMPLEMENTATION",
        1,
    );
}
#[test]
fn validator_file_reporting_failure_retains_failed_reporting_split() {
    let mut context = completion(true);
    context["finalizer_step_status"] = json!({"status":"failed","reporting_failure":true});
    validate(Fixture::new().file(context), "FAILED_REPORTING", 1);
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
            json!({"agent_outputs":{"hollow_success_signals":"true"}}),
            "HOLLOW_SUCCESS",
        ),
        (
            json!({"prior_terminal_state":{"terminal_state":"BLOCKED_CI"}}),
            "BLOCKED_CI",
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
    validate(
        Fixture::new()
            .env("FINALIZATION_EVIDENCE", "{bad")
            .env("IMPLEMENTATION_COMPLETED", "true")
            .env("VERIFICATION_COMPLETED", "true"),
        "FAILED_INVALID_EVIDENCE",
        1,
    );
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
#[test]
fn finalizer_invalid_mode_retains_error_two() {
    Fixture::new().finalizer("invalid").exit(2);
}
