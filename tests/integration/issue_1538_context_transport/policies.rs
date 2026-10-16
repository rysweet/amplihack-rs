use super::fixtures::*;
use serde_json::json;

#[test]
fn legacy_helper_exact_positive_tokens_and_string_schema() {
    for token in [
        "WORK_VERIFIED",
        "VERIFIED",
        "SUCCESS",
        "APPROVED",
        "PASS",
        "PASSED",
    ] {
        Fixture::new()
            .env("VERDICT_JSON", verdict(token))
            .helper()
            .helper_state("IMPLEMENTATION_COMPLETED");
    }
}

#[test]
fn helper_does_not_normalize_case_or_trim_verdicts() {
    for token in [
        "pass",
        " PASS ",
        "NOT_APPROVED",
        "UNVERIFIED",
        "HOLLOW_SUCCESS",
        "",
    ] {
        Fixture::new()
            .env("VERDICT_JSON", verdict(token))
            .helper()
            .helper_state("IMPLEMENTATION_UNPROVEN");
    }
}

#[test]
fn enforcer_preserves_synonym_normalization_and_hollow_rejection() {
    for token in [" verified ", "pass", "Success", "APPROVED", "PASSED"] {
        Fixture::new()
            .env("VERDICT_JSON", verdict(token))
            .gate()
            .gate("APPROVED");
    }
    for token in [
        "HOLLOW",
        "FAILED",
        "FAIL",
        "NO_WORK",
        "NO_ARTIFACTS",
        "EMPTY",
    ] {
        Fixture::new()
            .env("VERDICT_JSON", verdict(token))
            .gate()
            .gate("HOLLOW_SUCCESS");
    }
    for token in ["UNVERIFIED", "NOT_APPROVED", "novel", ""] {
        Fixture::new()
            .env("VERDICT_JSON", verdict(token))
            .gate()
            .gate("INSUFFICIENT_EVIDENCE");
    }
}

#[test]
fn helper_boolish_noop_keeps_priority_and_no_whitespace_trimming() {
    for token in ["true", "1", "yes", "y", "TRUE", "YeS"] {
        Fixture::new()
            .env("ALLOW_NO_OP", token)
            .env("VERDICT_JSON", verdict("HOLLOW_SUCCESS"))
            .helper()
            .helper_state("ALLOW_NO_OP");
    }
    Fixture::new()
        .env("ALLOW_NO_OP", " true ")
        .env("VERDICT_JSON", verdict("HOLLOW_SUCCESS"))
        .helper()
        .helper_state("IMPLEMENTATION_UNPROVEN");
}

#[test]
fn enforcer_requires_literal_true_noop() {
    for token in ["TRUE", "1", "yes", "y", " true ", "false"] {
        Fixture::new()
            .env("ALLOW_NO_OP", token)
            .env("VERDICT_JSON", verdict("HOLLOW_SUCCESS"))
            .gate()
            .gate("HOLLOW_SUCCESS");
    }
    Fixture::new()
        .env("ALLOW_NO_OP", "true")
        .env("VERDICT_JSON", verdict("HOLLOW_SUCCESS"))
        .gate()
        .gate("ALLOW_NO_OP=true");
}

#[test]
fn exact_sentinel_substring_and_enforcer_order_are_preserved() {
    let text = format!("prefix\n{SENTINEL}\nsuffix");
    let f = Fixture::new()
        .env("IMPLEMENTATION", text)
        .env("ALLOW_NO_OP", "true");
    f.gate().gate("sentinel matched");
    f.helper().helper_state("ALLOW_NO_OP");
    for text in [
        "No files modified - orchestration task",
        "no files modified — orchestration task",
    ] {
        Fixture::new()
            .env("IMPLEMENTATION", text)
            .env("VERDICT_JSON", verdict("HOLLOW_SUCCESS"))
            .helper()
            .helper_state("IMPLEMENTATION_UNPROVEN");
    }
}

#[test]
fn canonical_noop_bool_and_selected_sentinel_are_consumed() {
    let f = Fixture::new()
        .file(json!({"allow_no_op":true,"verdict_json":{"verdict":"HOLLOW_SUCCESS"}}));
    f.helper().helper_state("ALLOW_NO_OP");
    f.gate().gate("ALLOW_NO_OP=true");
    let f = Fixture::new()
        .env(
            "RECIPE_VAR_implementation",
            format!("prefix {SENTINEL} suffix"),
        )
        .env("RECIPE_VAR_verdict_json", verdict("HOLLOW_SUCCESS"));
    f.helper().helper_state("ALLOW_NO_OP");
    f.gate().gate("sentinel matched");
}

#[test]
fn canonical_object_allows_one_string_decode_but_never_recursive_salvage() {
    let once = json!(verdict("WORK_VERIFIED")).to_string();
    Fixture::new()
        .env("RECIPE_VAR_verdict_json", once.clone())
        .helper()
        .helper_state("IMPLEMENTATION_COMPLETED");
    let twice = json!(once).to_string();
    Fixture::new()
        .env("RECIPE_VAR_verdict_json", twice)
        .env("VERDICT_JSON", verdict("WORK_VERIFIED"))
        .helper()
        .helper_state("IMPLEMENTATION_UNPROVEN");
}

#[test]
fn canonical_multiline_object_retains_evidence_array_in_enforcer() {
    let object: serde_json::Value = serde_json::from_str(&verdict("WORK_VERIFIED")).unwrap();
    let f = Fixture::new().env(
        "RECIPE_VAR_verdict_json",
        serde_json::to_string_pretty(&object).unwrap(),
    );
    f.helper().helper_state("IMPLEMENTATION_COMPLETED");
    let run = f.gate();
    run.gate("APPROVED");
    let offset = run
        .stderr
        .find('{')
        .expect("approved diagnostics retain object");
    let observed: serde_json::Value = serde_json::from_str(&run.stderr[offset..]).unwrap();
    assert_eq!(
        observed, object,
        "complete parsed object equality, including evidence ARRAY"
    );
}

#[test]
fn enforcer_metadata_and_native_output_type_remain_unchanged() {
    let s = step("workflow-tdd", "step-08c-enforce-verdict");
    assert_eq!(s["type"], "bash");
    assert_eq!(s["output"], "implementation_noop_guard");
    assert_eq!(
        s["condition"],
        "resume_checkpoint != 'checkpoint-after-implementation' and resume_checkpoint != 'checkpoint-after-review-feedback'"
    );
    for (recipe, id) in [
        ("workflow-tdd", "implementation-terminal-evidence"),
        ("workflow-precommit-test", "verification-terminal-evidence"),
        ("workflow-finalize", "validate-agentic-finalization"),
    ] {
        assert_eq!(
            step(recipe, id)["parse_json"],
            true,
            "{id} must retain OBJECT output"
        );
    }
}

#[test]
fn encoded_nul_cannot_turn_canonical_or_legacy_data_into_positive_tokens() {
    for key in ["VERDICT_JSON", "RECIPE_VAR_verdict_json"] {
        Fixture::new()
            .env(key, verdict("WORK_VERI\0FIED"))
            .helper()
            .helper_state("IMPLEMENTATION_UNPROVEN");
    }
    let f = Fixture::new()
        .file(json!({"allow_no_op":"tru\0e", "verdict_json":{"verdict":"HOLLOW_SUCCESS"}}));
    f.helper().helper_state("IMPLEMENTATION_UNPROVEN");
    f.gate().gate("HOLLOW_SUCCESS");
}
