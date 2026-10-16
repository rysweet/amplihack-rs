use super::fixtures::*;
use serde_json::json;

pub(super) fn precommit() -> serde_json::Value {
    json!({"status":"PASS","exit_code":0,"precommit_exit_code":0,"workspace_exit_code":0,
        "workspace_command":"cargo test --workspace --locked","validation_scope":"FULL_WORKSPACE",
        "test_threads":"DEFAULT","evidence":["controlled-precommit-fixture"]})
}
pub(super) fn testing() -> serde_json::Value {
    json!({"status":"PASS","exit_code":0,"scenarios":[
        {"status":"PASS","exit_code":0},{"status":"PASS","exit_code":0}],
        "evidence":["controlled-scenario-fixture"]})
}

fn verification(f: &Fixture, completed: &str, noop: &str) -> serde_json::Value {
    let r = f.run(&body(
        "workflow-precommit-test",
        "verification-terminal-evidence",
    ));
    r.exit(0);
    let j = r.json();
    assert_eq!(j["verification_completed"], completed);
    assert_eq!(j["terminal_no_op"], noop);
    assert_eq!(j.as_object().unwrap().len(), 4);
    assert!(j.as_object().unwrap().values().all(|v| v.is_string()));
    j
}

#[test]
fn verification_canonical_validation_outputs_produce_evidence() {
    verification(
        &Fixture::new()
            .env("RECIPE_VAR_precommit_results", precommit().to_string())
            .env("RECIPE_VAR_local_testing_gate", testing().to_string()),
        "true",
        "false",
    );
}

#[test]
fn verification_file_positive_beats_stale_missing_outputs() {
    verification(
        &Fixture::new()
            .file(json!({"precommit_results":precommit(),"local_testing_gate":testing()}))
            .env("PRECOMMIT_RESULTS", "")
            .env("LOCAL_TESTING_GATE", ""),
        "true",
        "false",
    );
}

#[test]
fn verification_canonical_missing_member_suppresses_legacy_positive() {
    verification(
        &Fixture::new()
            .env("RECIPE_VAR_precommit_results", "output")
            .env("PRECOMMIT_RESULTS", "stale")
            .env("LOCAL_TESTING_GATE", "stale"),
        "false",
        "false",
    );
}

#[test]
fn verification_file_false_noop_suppresses_stale_true() {
    verification(
        &Fixture::new()
            .file(json!({"allow_no_op":false}))
            .env("ALLOW_NO_OP", "true"),
        "false",
        "false",
    );
}

#[test]
fn verification_file_true_noop_is_preserved() {
    verification(
        &Fixture::new().file(json!({"allow_no_op":true})),
        "false",
        "true",
    );
}

#[test]
fn verification_legacy_only_structured_receipts_and_boolish_policy_remain_supported() {
    verification(
        &Fixture::new()
            .env("PRECOMMIT_RESULTS", precommit().to_string())
            .env("LOCAL_TESTING_GATE", testing().to_string()),
        "true",
        "false",
    );
    verification(
        &Fixture::new().env("PRECOMMIT_RESULTS", precommit().to_string()),
        "false",
        "false",
    );
    verification(&Fixture::new().env("ALLOW_NO_OP", "YES"), "false", "true");
}

#[test]
fn producer_objects_flow_through_collector_validator_and_completion() {
    // Run actual producers first; do not inject fabricated completion flags.
    let f = Fixture::new().env("VERDICT_JSON", verdict("WORK_VERIFIED"));
    let ir = f.helper();
    ir.helper_state("IMPLEMENTATION_COMPLETED");
    let vr = verification(
        &Fixture::new()
            .env("PRECOMMIT_RESULTS", precommit().to_string())
            .env("LOCAL_TESTING_GATE", testing().to_string()),
        "true",
        "false",
    );
    let context = json!({"implementation_terminal_evidence":ir.json(),"verification_terminal_evidence":vr,
        "finalizer_step_status":{"status":"ok","reporting_failure":"false"}});
    let f = Fixture::new().file(context.clone());
    let collected = f.finalizer("collect");
    collected.exit(0);
    assert_eq!(
        collected.json()["completion"]["implementation_completed"],
        "true"
    );
    assert_eq!(
        collected.json()["completion"]["verification_completed"],
        "true"
    );
    let mut context = context;
    context["finalization_evidence"] = collected.json();
    let validated = Fixture::new().file(context.clone()).finalizer("validate");
    validated.exit(0);
    assert_eq!(validated.json()["terminal_state"], "IMPLEMENTED_VERIFIED");
    context["workflow_result"] = validated.json();
    let complete = Fixture::new().file(context).finalizer("complete");
    complete.exit(0);
    assert_eq!(
        complete.json()["workflow_result"]["terminal_state"],
        "IMPLEMENTED_VERIFIED"
    );
    assert_eq!(complete.json()["terminal_success"], "true");
}

#[test]
fn reporting_status_file_narrative_and_stale_alias_authority() {
    let f = Fixture::new()
        .file(json!({"agentic_finalizer_narrative":"actual report"}))
        .env("AGENTIC_FINALIZER_NARRATIVE", "");
    let r = f.run(&body("workflow-finalize", "finalizer-step-status"));
    r.exit(0);
    assert_eq!(r.json(), json!({"status":"ok","reporting_failure":"false"}));
}

#[test]
fn reporting_status_canonical_empty_suppresses_stale_narrative() {
    let f = Fixture::new()
        .env("RECIPE_VAR_agentic_finalizer_narrative", "")
        .env("AGENTIC_FINALIZER_NARRATIVE", "stale positive report");
    let r = f.run(&body("workflow-finalize", "finalizer-step-status"));
    r.exit(0);
    assert_eq!(
        r.json(),
        json!({"status":"failed","reporting_failure":"true"})
    );
}

#[test]
fn failed_or_subset_reports_never_produce_verification() {
    for failure in [
        json!({"status":"FAIL","exit_code":101}),
        {
            let mut value = precommit();
            value["workspace_exit_code"] = json!(101);
            value
        },
        {
            let mut value = precommit();
            value["validation_scope"] = json!("SUBSET");
            value
        },
        {
            let mut value = precommit();
            value["test_threads"] = json!("SERIAL");
            value
        },
        {
            let mut value = precommit();
            value["exit_code"] = json!("0");
            value
        },
        {
            let mut value = precommit();
            value["evidence"] = json!([]);
            value
        },
    ] {
        verification(
            &Fixture::new().file(json!({"precommit_results":failure,
            "local_testing_gate":testing()})),
            "false",
            "false",
        );
    }
    verification(
        &Fixture::new()
            .env("PRECOMMIT_RESULTS", "nonempty failed workspace101")
            .env("LOCAL_TESTING_GATE", "nonempty PASS scenarios"),
        "false",
        "false",
    );
    let mut failure = testing();
    failure["scenarios"][1]["exit_code"] = json!(1);
    verification(
        &Fixture::new().file(json!({"precommit_results":precommit(),
        "local_testing_gate":failure})),
        "false",
        "false",
    );
}

#[test]
fn verification_rejects_nul_in_a_serialized_report_without_truncating_it_to_pass() {
    let invalid = format!("{}\0", precommit());
    let f =
        Fixture::new().file(json!({"precommit_results":invalid,"local_testing_gate":testing()}));
    let output = verification(&f, "false", "false");
    assert_eq!(output["terminal_state"], "VERIFICATION_UNPROVEN");
}
