use super::assessed::completion;
use super::fixtures::*;
use serde_json::json;

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
fn finalizer_invalid_mode_retains_error_two() {
    Fixture::new().finalizer("invalid").exit(2);
}

// #2966: the collector used to derive `ci.state` from the terminal-state
// probe's record: `FAILURE` when the probe had said `BLOCKED_CI`, `UNKNOWN`
// otherwise. That was a second, older reading of CI beside the live one the
// `outcome` facts carry, and nothing read it. It is gone, so a stale record of
// the old probe's blocker cannot turn into a CI fact. The record itself is still
// carried, as what the probe said, under `prior_terminal_state`.
#[test]
fn collector_derives_no_ci_state_from_the_probes_record() {
    for prior in ["BLOCKED_CI", "FOLLOWUP_CREATED", "MERGED"] {
        let f = Fixture::new().env(
            "RECIPE_VAR_terminal_state",
            json!({"terminal_state": prior}).to_string(),
        );
        let r = f.finalizer("collect");
        r.exit(0);
        let j = r.json();
        assert!(
            j.get("ci").is_none(),
            "{prior}: the collector must not derive a CI state from the probe's record: {j}"
        );
        assert_eq!(
            j["prior_terminal_state"]["terminal_state"], prior,
            "{prior}: the probe's record is still carried as what the probe said: {j}"
        );
    }
}
