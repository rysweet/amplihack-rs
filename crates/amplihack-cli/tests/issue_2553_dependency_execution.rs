//! Completion and readiness tests synchronise through child completion only.

use crate::issue_2553_fixture::{FIRST, Fixture, SECOND, describe, leaf, plan};
use serde_json::{Value, json};
use std::fs;

fn dependent_plan() -> Value {
    let mut producer = leaf(
        "research",
        "Operations",
        "Answer the research questions in chat.",
    );
    producer["workstream_id"] = json!(FIRST);
    let mut consumer = leaf(
        "report",
        "Operations",
        "Report the research result in chat.",
    );
    consumer["workstream_id"] = json!(SECOND);
    consumer["depends_on"] = json!(["research"]);
    // Deliberately opposite to launch order: array order is not readiness.
    plan(vec![consumer, producer])
}

fn success() -> Value {
    json!({"schema_version": 1, "workstream_id": FIRST, "repository": "example/operations",
        "classification": "Operations", "effective_recipe": "operations-direct", "outcome": "succeeded",
        "outputs": {"answer": {"kind": "answer", "text": "Research result", "destination": "chat",
            "evidence": [{"source": "fixture", "reference": "research-answer",
                "observation": "Answered the research question"}]}}})
}

#[test]
fn dependent_receives_only_validated_durable_prerequisite_results() {
    let f = Fixture::new();
    f.normalise(&dependent_plan());
    let out = f.execute("recipe");
    assert!(out.status.success(), "{}", describe(&out));
    let calls = f.calls("execute");
    assert_eq!(calls.len(), 2, "{:?}", f.events());
    assert_eq!(calls[0]["context"]["workstream_id"], FIRST);
    assert_eq!(calls[1]["context"]["workstream_id"], SECOND);
    let prerequisites = calls[1]["context"]["prerequisite_results"]
        .as_array()
        .expect("prerequisite array");
    assert_eq!(prerequisites.len(), 1);
    assert_eq!(prerequisites[0]["workstream_id"], FIRST);
    assert_eq!(prerequisites[0]["repository"], "example/operations");
    assert!(
        prerequisites[0]["result_revision"]
            .as_u64()
            .is_some_and(|r| r > 0)
    );
    let at_launch = calls[1]["states_at_launch"]
        .as_array()
        .expect("durable state snapshot at consumer launch");
    let ready = at_launch
        .iter()
        .find(|s| s["workstream_id"] == FIRST)
        .expect("saved producer before launch");
    assert_eq!(ready["lifecycle"], "succeeded");
    assert_eq!(ready["revision"], prerequisites[0]["result_revision"]);
    assert_eq!(ready["result"]["outputs"], prerequisites[0]["outputs"]);
    let states = f.states();
    let producer = states
        .iter()
        .find(|s| s["workstream_id"] == FIRST)
        .expect("producer state");
    assert_eq!(producer["lifecycle"], "succeeded");
    assert_eq!(producer["result"]["outputs"], prerequisites[0]["outputs"]);
}

#[test]
fn sequential_execution_preserves_the_same_leaves_routes_and_identities() {
    let f = Fixture::new();
    f.normalise(&dependent_plan());
    let out = f
        .command("execute")
        .arg(&f.config)
        .arg("recipe")
        .env("AMPLIHACK_MAX_PARALLEL_WORKSTREAMS", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    f.assert_direct(2);
    let ids: Vec<_> = f
        .calls("execute")
        .iter()
        .map(|c| c["context"]["workstream_id"].clone())
        .collect();
    assert_eq!(ids, vec![json!(FIRST), json!(SECOND)]);
}

#[test]
fn zero_exit_with_invalid_result_does_not_release_dependants() {
    for mutation in [
        "missing-output",
        "wrong-producer",
        "wrong-repository",
        "wrong-kind",
        "blank-answer",
        "missing-evidence",
        "unknown-schema",
        "unknown-outcome",
        "undeclared-output",
    ] {
        let f = Fixture::new();
        f.normalise(&dependent_plan());
        let mut result = success();
        match mutation {
            "missing-output" => result["outputs"] = json!({}),
            "wrong-producer" => result["workstream_id"] = json!(SECOND),
            "wrong-repository" => result["repository"] = json!("example/foreign"),
            "wrong-kind" => result["outputs"]["answer"]["kind"] = json!("issue"),
            "blank-answer" => result["outputs"]["answer"]["text"] = json!(" \n"),
            "missing-evidence" => result["outputs"]["answer"]["evidence"] = json!([]),
            "unknown-schema" => result["schema_version"] = json!(2),
            "unknown-outcome" => result["outcome"] = json!("probably-done"),
            _ => result["outputs"]["unrequested"] = result["outputs"]["answer"].clone(),
        }
        fs::write(
            f.root.join(format!("result-{FIRST}.json")),
            serde_json::to_vec(&result).unwrap(),
        )
        .unwrap();
        let out = f.execute("recipe");
        assert!(!out.status.success(), "{mutation}: {}", describe(&out));
        let calls = f.calls("execute");
        assert_eq!(
            calls.len(),
            1,
            "fixture must launch the producer, then reject its result: {mutation}"
        );
        assert_eq!(calls[0]["context"]["workstream_id"], FIRST);
        assert!(f.states().iter().any(|s| s["workstream_id"] == SECOND
            && s["lifecycle"] == "blocked"
            && s["blocked_by"] == json!([FIRST])));
    }
}

#[test]
fn failed_prerequisite_records_blocked_dependants_without_launching_them() {
    let f = Fixture::new();
    f.normalise(&dependent_plan());
    fs::write(f.root.join("agent-failure"), "reject").unwrap();
    let out = f.execute("recipe");
    assert!(!out.status.success(), "{}", describe(&out));
    assert_eq!(f.calls("execute").len(), 1);
    let states = f.states();
    assert!(
        states
            .iter()
            .any(|s| s["workstream_id"] == FIRST && s["lifecycle"] == "failed")
    );
    assert!(
        states
            .iter()
            .any(|s| s["workstream_id"] == SECOND && s["lifecycle"] == "blocked")
    );
}

#[test]
fn completion_drains_stdout_and_stderr_to_eof_before_returning() {
    let f = Fixture::new();
    let mut item = leaf(
        "answer",
        "Operations",
        "Answer the research questions in chat.",
    );
    item["workstream_id"] = json!(FIRST);
    f.normalise(&plan(vec![item]));
    let out = f.execute("recipe");
    assert!(out.status.success(), "{}", describe(&out));
    let states = f.states();
    let state = states
        .iter()
        .find(|s| s["workstream_id"] == FIRST)
        .expect("durable state");
    let log = fs::read_to_string(state["log_file"].as_str().expect("owned log path")).unwrap();
    assert!(
        log.contains("Fixture research answer"),
        "stdout missing: {log}"
    );
    assert!(
        log.contains("fixture stderr tail without newline"),
        "stderr not drained: {log}"
    );
}

#[test]
fn result_persistence_failure_never_reports_success_or_releases_a_dependant() {
    let f = Fixture::new();
    f.normalise(&dependent_plan());
    // The producer fixture replaces its own state file with a directory after
    // launch. This makes the completion write fail without permission tricks.
    fs::write(f.root.join("break-result-persistence"), FIRST).unwrap();
    let out = f.execute("recipe");
    assert!(!out.status.success(), "{}", describe(&out));
    let calls = f.calls("execute");
    assert_eq!(calls.len(), 1, "failure must occur after producer launch");
    assert_eq!(calls[0]["context"]["workstream_id"], FIRST);
}
