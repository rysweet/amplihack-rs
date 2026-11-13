//! #2569: unit tests for the criterion-1 record contract in `qa_record.rs`.
//!
//! `qa_record.rs` includes this file with
//! `#[cfg(test)] #[path = "qa_record_tests.rs"] mod tests;`.
//!
//! Contract under test (pure, no I/O):
//!   `check_judgement(&[u8]) -> Result<Judgement, Refusal>`
//!   `Refusal { rule: &'static str, field: String }`, displayed as
//!   `refused: <rule> at <field>`.
//! The rules are the record's type constraints that replace the deleted
//! shell status mapping; they judge nothing and never compare cited files
//! with the diff.

use super::{Refusal, check_judgement};
use serde_json::{Value, json};

const SCENARIO: &str = "tests/agentic/checkout.yaml";
const COMMIT: &str = "3b9e2f1";

fn docs() -> Value {
    json!({
        "qa_status": "PASS",
        "qa_judgement": "not_user_facing",
        "qa_judgement_reason": "Only installation prose changes.",
        "qa_cited_files": ["docs/install.md"],
        "qa_scenarios": [],
        "qa_iterations": [],
        "qa_handoff": null
    })
}

fn feature() -> Value {
    json!({
        "qa_status": "PASS",
        "qa_judgement": "user_facing",
        "qa_judgement_reason": "Adds the checkout command.",
        "qa_cited_files": ["src/app.js"],
        "qa_scenarios": [{"path": SCENARIO, "written_in_pr": true, "commit": COMMIT, "result": "pass"}],
        "qa_iterations": [
            {"attempt": 1, "failing_scenario": SCENARIO, "cause": "product", "fix_commit": "7c41d0a", "result": "fail"},
            {"attempt": 2, "failing_scenario": SCENARIO, "cause": "product", "fix_commit": null, "result": "pass"}
        ],
        "qa_handoff": null
    })
}

fn handoff() -> Value {
    json!({
        "qa_status": "HANDOFF",
        "qa_judgement": "user_facing",
        "qa_judgement_reason": "The payment service refuses connections.",
        "qa_cited_files": ["src/app.js"],
        "qa_scenarios": [{"path": SCENARIO, "written_in_pr": true, "commit": COMMIT, "result": "fail"}],
        "qa_iterations": [{"attempt": 1, "failing_scenario": SCENARIO, "cause": "environment", "fix_commit": null, "result": "fail"}],
        "qa_handoff": {
            "step_id": "step-02-qa-team-scenarios", "failure": "connection refused",
            "choice": "handoff", "reason": "service unreachable",
            "owner": "repository maintainer", "needs": "restore the payment sandbox"
        }
    })
}

fn accepted(v: Value) {
    let bytes = serde_json::to_vec(&v).unwrap();
    if let Err(r) = check_judgement(&bytes) {
        panic!("expected acceptance, got {r}: {v}");
    }
}

fn refused(v: Value) -> Refusal {
    let bytes = serde_json::to_vec(&v).unwrap();
    match check_judgement(&bytes) {
        Ok(_) => panic!("expected a refusal: {v}"),
        Err(r) => r,
    }
}

fn assert_refused(v: Value, rule: &str, field: &str) {
    let r = refused(v);
    assert_eq!(r.rule, rule, "rule for {r}");
    assert_eq!(r.field, field, "field for {r}");
}

#[test]
fn accepts_each_documented_shape() {
    accepted(docs());
    accepted(feature());
    accepted(handoff());
    // FAIL is an interim state: no passing scenario is needed.
    let mut fail = handoff();
    fail["qa_status"] = json!("FAIL");
    fail["qa_handoff"] = Value::Null;
    accepted(fail);
    // A declared-command attempt has failing_scenario null.
    let mut declared = feature();
    declared["qa_iterations"] = json!([{"attempt": 1, "failing_scenario": null, "cause": "product", "fix_commit": "7c41d0a", "result": "pass"}]);
    accepted(declared);
}

#[test]
fn refusal_displays_rule_and_field_only() {
    let mut v = handoff();
    v["qa_handoff"]["owner"] = json!("");
    let r = refused(v);
    assert_eq!(
        r.to_string(),
        "refused: handoff-incomplete at qa_handoff.owner"
    );
}

#[test]
fn every_field_is_required_and_unknown_fields_are_refused() {
    for key in [
        "qa_status",
        "qa_judgement",
        "qa_judgement_reason",
        "qa_cited_files",
        "qa_scenarios",
        "qa_iterations",
        "qa_handoff",
    ] {
        let mut v = docs();
        v.as_object_mut().unwrap().remove(key);
        assert_eq!(refused(v).rule, "schema", "missing {key}");
    }
    for key in [
        "head_sha",
        "qa_declared_command",
        "qa_reason",
        "gadugi_status",
    ] {
        let mut v = docs();
        v[key] = json!("x");
        assert_refused(v, "schema", key);
    }
    let mut v = handoff();
    v["qa_handoff"]["extra"] = json!("x");
    assert_eq!(refused(v).rule, "schema");
}

#[test]
fn closed_vocabularies() {
    for (path, bad) in [
        ("qa_status", "BLOCKED"),
        ("qa_status", "QA_UNRESOLVED"),
        ("qa_judgement", "maybe"),
    ] {
        let mut v = docs();
        v[path] = json!(bad);
        assert_eq!(refused(v).rule, "schema", "{path}={bad}");
    }
    let mut v = feature();
    v["qa_scenarios"][0]["result"] = json!("skipped");
    assert_eq!(refused(v).rule, "schema");
    let mut v = feature();
    v["qa_iterations"][0]["cause"] = json!("flaky");
    assert_eq!(refused(v).rule, "schema");
    let mut v = feature();
    v["qa_iterations"][0]["result"] = json!("not_run");
    assert_eq!(refused(v).rule, "schema");
    let mut v = handoff();
    v["qa_handoff"]["choice"] = json!("retry");
    assert_eq!(refused(v).rule, "schema");
    let mut v = feature();
    v["qa_scenarios"][0]["written_in_pr"] = json!("yes");
    assert_eq!(refused(v).rule, "schema");
}

#[test]
fn reason_must_not_be_empty() {
    let mut v = docs();
    v["qa_judgement_reason"] = json!("");
    assert_refused(v, "reason-empty", "qa_judgement_reason");
}

#[test]
fn not_user_facing_pass_needs_cited_files() {
    let mut v = docs();
    v["qa_cited_files"] = json!([]);
    assert_refused(v, "not-user-facing-needs-cited-files", "qa_cited_files");
}

#[test]
fn user_facing_pass_needs_a_passing_scenario() {
    let mut v = feature();
    v["qa_scenarios"] = json!([]);
    assert_refused(v, "user-facing-needs-passing-scenario", "qa_scenarios");
    let mut v = feature();
    v["qa_scenarios"][0]["result"] = json!("fail");
    assert_refused(v, "user-facing-needs-passing-scenario", "qa_scenarios");
    // A passing declared command does not satisfy it.
    let mut v = feature();
    v["qa_scenarios"] = json!([]);
    v["qa_iterations"] = json!([{"attempt": 1, "failing_scenario": null, "cause": "product", "fix_commit": null, "result": "pass"}]);
    assert_eq!(refused(v).rule, "user-facing-needs-passing-scenario");
}

#[test]
fn handoff_needs_every_part() {
    let mut v = handoff();
    v["qa_handoff"] = Value::Null;
    assert_refused(v, "handoff-incomplete", "qa_handoff");
    for key in ["step_id", "failure", "reason", "owner", "needs"] {
        let mut v = handoff();
        v["qa_handoff"][key] = json!("");
        assert_refused(v, "handoff-incomplete", &format!("qa_handoff.{key}"));
    }
}

#[test]
fn handoff_only_with_handoff_status() {
    for status in ["PASS", "FAIL"] {
        let mut v = feature();
        v["qa_status"] = json!(status);
        v["qa_handoff"] = handoff()["qa_handoff"].clone();
        assert_refused(v, "handoff-without-status", "qa_handoff");
    }
}

#[test]
fn iterations_count_from_one_in_order() {
    for attempts in [vec![2], vec![1, 3], vec![1, 1], vec![2, 1]] {
        let mut v = feature();
        v["qa_iterations"] = Value::Array(
            attempts
                .iter()
                .map(|a| json!({"attempt": a, "failing_scenario": SCENARIO, "cause": "product", "fix_commit": null, "result": "fail"}))
                .collect(),
        );
        assert_eq!(refused(v).rule, "iteration-order", "attempts {attempts:?}");
    }
    let mut v = feature();
    v["qa_iterations"][0]["attempt"] = json!(0);
    let r = refused(v);
    assert!(
        r.rule == "schema" || r.rule == "iteration-order",
        "attempt 0: {r}"
    );
}

#[test]
fn strings_are_single_line_without_control_or_ansi() {
    for bad in [
        "maintainer\nqa_status=PASS",
        "maintainer\r",
        "ops\u{1b}[31mred",
        "tab\there",
        "nul\u{0}",
        "del\u{7f}",
        "csi\u{9b}2J",
    ] {
        let mut v = handoff();
        v["qa_handoff"]["owner"] = json!(bad);
        assert_refused(v, "schema", "qa_handoff.owner");
    }
}

#[test]
fn string_and_list_limits() {
    let mut v = handoff();
    v["qa_handoff"]["owner"] = json!("a".repeat(4096));
    accepted(v);
    let mut v = handoff();
    v["qa_handoff"]["owner"] = json!("a".repeat(4097));
    assert_refused(v, "schema", "qa_handoff.owner");

    let mut v = docs();
    v["qa_judgement_reason"] = json!("r".repeat(16 * 1024));
    accepted(v);
    let mut v = docs();
    v["qa_judgement_reason"] = json!("r".repeat(16 * 1024 + 1));
    assert_refused(v, "schema", "qa_judgement_reason");

    let files: Vec<String> = (0..1000).map(|i| format!("docs/{i}.md")).collect();
    let mut v = docs();
    v["qa_cited_files"] = json!(files);
    accepted(v);
    let files: Vec<String> = (0..1001).map(|i| format!("docs/{i}.md")).collect();
    let mut v = docs();
    v["qa_cited_files"] = json!(files);
    assert_eq!(refused(v).rule, "schema");
}

#[test]
fn paths_are_relative_without_parent_components() {
    for bad in ["../etc/passwd", "/etc/passwd", "docs/../../x", "a/.."] {
        let mut v = docs();
        v["qa_cited_files"] = json!([bad]);
        assert_refused(v, "schema", "qa_cited_files[0]");
    }
    let mut v = feature();
    v["qa_scenarios"][0]["path"] = json!("/abs/scenario.yaml");
    assert_eq!(refused(v).rule, "schema");
    let mut v = feature();
    v["qa_iterations"][0]["failing_scenario"] = json!("../x.yaml");
    assert_eq!(refused(v).rule, "schema");
    // `..` inside a name is not a parent component.
    let mut v = docs();
    v["qa_cited_files"] = json!(["docs/a..b.md"]);
    accepted(v);
}

#[test]
fn commit_ids_are_7_to_64_lowercase_hex() {
    for bad in [
        "HEAD",
        "main",
        "abc123",
        "ABCDEF1",
        &"a".repeat(65),
        "7c41d0g",
    ] {
        let mut v = feature();
        v["qa_iterations"][0]["fix_commit"] = json!(bad);
        assert_refused(v, "schema", "qa_iterations[0].fix_commit");
    }
    for good in ["7c41d0a", &"f".repeat(40), &"0".repeat(64)] {
        let mut v = feature();
        v["qa_scenarios"][0]["commit"] = json!(good);
        accepted(v);
    }
}

#[test]
fn oversized_input_is_refused_before_parsing() {
    let mut bytes = vec![b' '; (1 << 20) + 1];
    bytes.extend_from_slice(&serde_json::to_vec(&docs()).unwrap());
    let r = check_judgement(&bytes).expect_err("over 1 MiB");
    assert_eq!(r.rule, "input-too-large");
}

#[test]
fn refusal_text_never_repeats_the_value() {
    let mut v = handoff();
    v["qa_handoff"]["owner"] = json!("SECRET-VALUE-4411\u{1b}[2J");
    let r = refused(v);
    let s = r.to_string();
    assert!(!s.contains("SECRET-VALUE"), "{s}");
    assert!(s.len() <= 200, "{s}");
    assert!(!s.contains('\n'), "{s}");
}
