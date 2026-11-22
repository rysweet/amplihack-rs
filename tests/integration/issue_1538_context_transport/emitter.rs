//! File-backed values must survive the actual validator's JSON output boundary.
//!
//! The terminal state is the file's assessment, reported as written (#2343,
//! #2809). The values under test are the ones the validator emits beside it.
use super::assessed::{assessed, completion, facts_line};
use super::fixtures::*;
use serde_json::{Value, json};

fn completed() -> Value {
    assessed(completion(true), "IMPLEMENTED_VERIFIED", "")
}

fn validate(context: Value) -> Run {
    // Only file paths enter argv/environment. The unrelated cwd and the
    // outcome recorded in the evidence prevent repository discovery from
    // masking this boundary.
    let f = Fixture::new();
    let ceiling = f.temp.path().display().to_string();
    let r = f
        .file(context)
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .finalizer("validate");
    println!(
        "actual validator exit={}\nstdout={}\nstderr={}",
        r.code, r.stdout, r.stderr
    );
    r
}

fn expected(pr_url: &str, pr_number: &str) -> Value {
    json!({
        "terminal_success": "true",
        "terminal_state": "IMPLEMENTED_VERIFIED",
        "terminal_reason": "assess-finalization decided IMPLEMENTED_VERIFIED on the run's outcome",
        "required_next_action": "Act on IMPLEMENTED_VERIFIED.",
        "hollow_success_detected": "false",
        "evidence_used": format!(
            "{},assessment_state=IMPLEMENTED_VERIFIED,assessment_action=none,reporting_failure=false",
            facts_line()
        ),
        "finalizer_schema_version": "1",
        "finalizer_confidence": "high",
        "finalizer_output_valid": "true",
        "reporting_failure": "false",
        "implementation_completed": "true",
        "verification_completed": "true",
        "publish_state_reached": "false",
        "terminal_no_op": "false",
        "terminal_failure": "false",
        "pr_url": pr_url,
        "pr_number": pr_number,
        "observed_phases": "workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-refactor-review,workflow-precommit-test,workflow-publish,workflow-pr-review,workflow-finalize",
        "missing_evidence": ""
    })
}

fn exact_output(r: &Run, expected: Value, exit: i32) {
    r.exit(exit);
    let actual = r.json(); // Reject truncated JSON or additional objects/text.
    assert_eq!(actual.as_object().unwrap().len(), 19);
    assert!(actual.as_object().unwrap().values().all(Value::is_string));
    assert_eq!(actual, expected);
    assert!(!r.stderr.contains("Argument list too long"));
}

#[test]
fn validator_large_file_pr_url_preserves_complete_string_output() {
    let url = format!("https://example.test/{}end", "urlλ\"\\\n".repeat(30_000));
    assert!(url.len() >= 200_000);
    let mut context = completed();
    context["terminal_state"] = json!({"pr_url": url, "pr_number": 1538});
    exact_output(&validate(context), expected(&url, "1538"), 0);
}

#[test]
fn validator_large_file_string_pr_number_preserves_complete_string_output() {
    let number = "1538".repeat(50_000);
    assert!(number.len() >= 200_000);
    let mut context = completed();
    context["terminal_state"] = json!({"pr_number": number});
    exact_output(&validate(context), expected("", &number), 0);
}

#[test]
fn validator_large_file_missing_tooling_preserves_strict_failure_and_diagnostics() {
    let missing = format!("git,{}end", "diagnosticλ\"\\\n".repeat(20_000));
    assert!(missing.len() >= 200_000);
    let mut context = completed();
    context["finalization_evidence"]["tooling"] = json!({"missing": missing});
    let mut result = expected("", "");
    result["terminal_success"] = json!("false");
    result["terminal_state"] = json!("FAILED_MISSING_TOOLING");
    result["terminal_reason"] = json!(format!(
        "collected evidence reported missing deterministic tooling: {missing}"
    ));
    result["required_next_action"] =
        json!("Run finalization from an environment with required git and jq tooling available.");
    result["evidence_used"] = json!(format!("finalization_evidence.tooling.missing={missing}"));
    result["finalizer_confidence"] = json!("low");
    result["finalizer_output_valid"] = json!("false");
    result["terminal_failure"] = json!("true");
    exact_output(&validate(context), result, 1);
}

#[test]
fn validator_numeric_pr_number_is_emitted_as_a_string() {
    let mut context = completed();
    context["terminal_state"] = json!({"pr_number": 1538});
    exact_output(&validate(context), expected("", "1538"), 0);
}

// The assessment's own text is a transported value too. Large and carrying
// control characters, it must come out bounded and clean, in an object that is
// still one complete result of 19 String fields.
#[test]
fn validator_large_file_assessment_text_is_bounded_and_free_of_control_characters() {
    let text = format!("decided {}", "x\n\t\u{1}".repeat(70_000));
    assert!(text.len() >= 200_000);
    let mut context = completed();
    context["finalization_assessment"]["reason"] = json!(text);
    context["finalization_assessment"]["next_action"] = json!(text);
    let r = validate(context);
    r.exit(0);
    let actual = r.json();
    assert_eq!(actual.as_object().unwrap().len(), 19);
    assert!(actual.as_object().unwrap().values().all(Value::is_string));
    assert_eq!(actual["terminal_state"], "IMPLEMENTED_VERIFIED");
    assert_eq!(actual["terminal_success"], "true");
    for field in ["terminal_reason", "required_next_action"] {
        let text = actual[field].as_str().unwrap();
        assert!(text.starts_with("decided xxxx"), "{field}: {text:.40}");
        assert!(text.len() <= 1200, "{field} is {} bytes", text.len());
        assert!(
            !text.chars().any(char::is_control),
            "{field} kept a control"
        );
    }
}
