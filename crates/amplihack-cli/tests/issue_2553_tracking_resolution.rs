//! Explicit tracking uses the production normaliser, dispatcher and CLI.

use crate::issue_2553_fixture::{FIRST, Fixture, REPOSITORY, describe, leaf, plan};
use serde_json::{Value, json};
use std::fs;

fn required() -> Value {
    let mut item = leaf("tracked", "Operations", "Report project status in chat.");
    item["workstream_id"] = json!(FIRST);
    item["tracking"] = json!({"required": true, "title": "Track the status report",
        "body": "Record the requested status report; literal $(touch injected) and `false`."});
    item
}

#[test]
fn explicitly_required_tracking_survives_normalisation() {
    let f = Fixture::new();
    let item = required();
    let config = f.normalise(&plan(vec![item.clone()]));
    assert_eq!(config[0]["tracking"], item["tracking"]);
}

#[test]
fn incomplete_required_tracking_is_rejected_before_creation() {
    let f = Fixture::new();
    for tracking in [
        json!({"required": true}),
        json!({"required": true, "title": "", "body": "b"}),
        json!({"required": true, "title": "t", "body": " \n"}),
    ] {
        let mut item = required();
        item["tracking"] = tracking;
        let out = f.normalise_output(&plan(vec![item]), "");
        assert!(!out.status.success(), "{}", describe(&out));
        assert!(f.events().is_empty());
    }
}

#[test]
fn required_creation_is_once_with_durable_attempt_before_gh_and_issue_before_child() {
    let f = Fixture::new();
    f.normalise(&plan(vec![required()]));
    let out = f.execute("recipe");
    assert!(out.status.success(), "{}", describe(&out));
    let calls = f.calls("gh");
    assert_eq!(calls.len(), 1, "{:?}", f.events());
    let args = calls[0]["args"].as_array().unwrap();
    assert_eq!(&args[..2], &[json!("issue"), json!("create")]);
    assert!(
        args.windows(2)
            .any(|a| a == [json!("--repo"), json!(REPOSITORY)])
    );
    assert_eq!(calls[0]["body"], required()["tracking"]["body"]);
    assert!(
        !f.repo.join("injected").exists(),
        "body must be passed literally"
    );
    let states = calls[0]["states_at_invocation"].as_array().unwrap();
    assert!(
        states.iter().any(|s| s["workstream_id"] == FIRST
            && s["tracking_attempts"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x["status"] == "prepared"))),
        "attempt must be durable before invoking gh: {states:#?}"
    );
    let executions = f.calls("execute");
    assert_eq!(executions.len(), 1);
    assert_eq!(executions[0]["context"]["issue_number"], 2553);
    assert!(
        executions[0]["states_at_launch"]
            .as_array()
            .expect("saved state at launch")
            .iter()
            .any(|s| s["workstream_id"] == FIRST && s["issue"] == 2553),
        "tracking must be durable before launching the child"
    );
    assert!(
        f.states()
            .iter()
            .any(|s| s["issue"] == 2553 && s["workstream_id"] == FIRST)
    );
    let out = f.execute("recipe");
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.calls("gh").len(), 1, "persisted tracking must be reused");
    assert_eq!(
        f.calls("execute").len(),
        1,
        "persisted success must not replay"
    );
}

#[test]
fn supplied_valid_issue_satisfies_required_tracking_without_creation_details() {
    let f = Fixture::new();
    let mut item = required();
    item["issue"] = json!(2553);
    item["tracking"] = json!({"required": true});
    f.normalise(&plan(vec![item]));
    let out = f.execute("recipe");
    assert!(out.status.success(), "{}", describe(&out));
    assert!(f.calls("gh").is_empty());
    assert_eq!(f.calls("execute")[0]["context"]["issue_number"], 2553);
}

#[test]
fn failed_or_invalid_creation_does_not_launch_or_invent_tracking() {
    for mode in [
        "rejected",
        "malformed",
        "wrong-repository",
        "internal-whitespace",
        "oversized",
        "invalid-utf8",
        "sensitive-diagnostic",
    ] {
        let f = Fixture::new();
        fs::write(f.root.join("gh-mode"), mode).unwrap();
        f.normalise(&plan(vec![required()]));
        let out = f.execute("recipe");
        assert!(!out.status.success(), "mode={mode}: {}", describe(&out));
        assert_eq!(
            f.calls("gh").len(),
            1,
            "failure must come from attempted creation"
        );
        assert!(f.calls("execute").is_empty());
        assert!(
            f.states()
                .iter()
                .all(|s| s.get("issue").is_none_or(Value::is_null))
        );
        assert!(!String::from_utf8_lossy(&out.stderr).trim().is_empty());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("fixture-secret-token"));
        assert!(
            !serde_json::to_string(&f.states())
                .unwrap()
                .contains("fixture-secret-token")
        );
        let resumed = f.execute("recipe");
        assert!(!resumed.status.success(), "{}", describe(&resumed));
        assert_eq!(
            f.calls("gh").len(),
            1,
            "invalid receipts must not trigger replay"
        );
    }
}

#[test]
fn tracking_mutations_pin_the_host_and_disable_inherited_overrides() {
    let f = Fixture::new();
    f.normalise(&plan(vec![required()]));
    let out = f
        .command("execute")
        .arg(&f.config)
        .arg("recipe")
        .env("GH_HOST", "foreign.invalid")
        .env("GH_REPO", "example/foreign")
        .env("GH_DEBUG", "api")
        .env("GH_PROMPT_DISABLED", "0")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let calls = f.calls("gh");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["host"], "github.com");
    assert_eq!(calls[0]["prompt_disabled"], "1");
    assert!(calls[0]["repo_override"].is_null());
    assert!(calls[0]["debug"].is_null());
}

#[test]
fn uncertain_creation_is_persisted_and_not_retried_on_resume() {
    let f = Fixture::new();
    fs::write(f.root.join("gh-mode"), "uncertain").unwrap();
    f.normalise(&plan(vec![required()]));
    let first = f.execute("recipe");
    assert!(!first.status.success(), "{}", describe(&first));
    assert_eq!(f.calls("gh").len(), 1);
    assert!(
        f.states()
            .iter()
            .any(|s| s["lifecycle"] == "reconciliation_required")
    );
    fs::write(f.root.join("gh-mode"), "success").unwrap();
    let resumed = f.execute("recipe");
    assert!(
        !resumed.status.success(),
        "reconciliation required: {}",
        describe(&resumed)
    );
    assert_eq!(
        f.calls("gh").len(),
        1,
        "unknown outcomes must not recreate issues"
    );
    assert!(f.calls("execute").is_empty());
}

#[test]
fn administrative_issue_outputs_require_an_explicit_valid_tracking_binding() {
    let f = Fixture::new();
    let mut producer = leaf("create", "Operations", "Open the requested tracking issue.");
    producer["workstream_id"] = json!(FIRST);
    producer["required_outputs"] = json!({"created_issue": "issue"});
    let mut consumer = leaf("use", "Operations", "Report status in the tracking issue.");
    consumer["depends_on"] = json!(["create"]);
    consumer["tracking"] = json!({"required": true, "from_result": {"workstream": "create", "output": "created_issue"}});
    let config = f.normalise(&plan(vec![producer.clone(), consumer.clone()]));
    assert_eq!(
        config[1]["tracking"]["from_result"],
        json!({"workstream_id": FIRST, "output": "created_issue"})
    );
    for modification in [
        "missing-dependency",
        "missing-output",
        "wrong-kind",
        "conflicting-issue",
        "conflicting-creation",
    ] {
        let mut a = producer.clone();
        let mut b = consumer.clone();
        match modification {
            "missing-dependency" => b["depends_on"] = json!([]),
            "missing-output" => b["tracking"]["from_result"]["output"] = json!("unknown"),
            "wrong-kind" => a["required_outputs"]["created_issue"] = json!("answer"),
            "conflicting-issue" => b["issue"] = json!(2553),
            _ => b["tracking"]["title"] = json!("Do not create a substitute"),
        }
        let out = f.normalise_output(&plan(vec![a, b]), "");
        assert!(!out.status.success(), "{modification}: {}", describe(&out));
    }
}

#[test]
fn administrative_issue_adapter_persists_attempt_and_reuses_the_durable_receipt() {
    let f = Fixture::new();
    let mut item = leaf("create", "Operations", "Open the requested issue.");
    item["workstream_id"] = json!(FIRST);
    item["required_outputs"] = json!({"created_issue": "issue"});
    f.normalise(&plan(vec![item]));
    fs::write(
        f.root.join("operation-request.json"),
        serde_json::to_vec(&json!({"action": "create_issue", "arguments": {
        "title": "Requested issue", "body": "Literal $(touch injected) and `false`"}}))
        .unwrap(),
    )
    .unwrap();
    let out = f.execute("recipe");
    assert!(out.status.success(), "{}", describe(&out));
    let gh = f.calls("gh");
    assert_eq!(gh.len(), 1);
    assert_eq!(gh[0]["body"], "Literal $(touch injected) and `false`");
    assert!(!f.repo.join("injected").exists());
    assert!(
        gh[0]["states_at_invocation"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["operation_attempts"][0]["status"] == "prepared")
    );
    let states = f.states();
    assert_eq!(states[0]["operation_attempts"][0]["status"], "succeeded");
    assert_eq!(
        states[0]["result"]["outputs"]["created_issue"],
        states[0]["operation_attempts"][0]["output"]
    );
    let resumed = f.execute("recipe");
    assert!(resumed.status.success(), "{}", describe(&resumed));
    assert_eq!(f.calls("gh").len(), 1);
    assert_eq!(f.calls("execute").len(), 1);
}

#[test]
fn uncertain_administrative_mutation_is_not_repeated_on_resume() {
    let f = Fixture::new();
    let mut item = leaf("create", "Operations", "Open the requested issue.");
    item["workstream_id"] = json!(FIRST);
    item["required_outputs"] = json!({"created_issue": "issue"});
    f.normalise(&plan(vec![item]));
    fs::write(f.root.join("operation-request.json"), serde_json::to_vec(&json!({"action": "create_issue", "arguments": {"title": "Requested issue", "body": "Requested body"}})).unwrap()).unwrap();
    fs::write(f.root.join("gh-mode"), "uncertain").unwrap();
    let failed = f.execute("recipe");
    assert!(!failed.status.success(), "{}", describe(&failed));
    assert_eq!(f.calls("gh").len(), 1);
    assert_eq!(f.states()[0]["lifecycle"], "reconciliation_required");
    let resumed = f.execute("recipe");
    assert!(!resumed.status.success(), "{}", describe(&resumed));
    assert_eq!(f.calls("gh").len(), 1);
    assert_eq!(f.calls("execute").len(), 1);
}

#[test]
fn administrative_assignment_uses_the_declared_repository_and_reuses_success() {
    let f = Fixture::new();
    let mut item = leaf(
        "assign",
        "Operations",
        "Assign the requested issue to its owners.",
    );
    item["workstream_id"] = json!(FIRST);
    item["required_outputs"] = json!({"created_issue": "operation"});
    f.normalise(&plan(vec![item]));
    fs::write(f.root.join("operation-request.json"), serde_json::to_vec(&json!({
        "action": "assign", "arguments": {"issue": "2553", "assignees": ["first-owner", "second-owner"]}
    })).unwrap()).unwrap();
    let out = f.execute("recipe");
    assert!(out.status.success(), "{}", describe(&out));
    let gh = f.calls("gh");
    assert_eq!(gh.len(), 1);
    assert_eq!(
        gh[0]["args"],
        json!([
            "issue",
            "edit",
            "2553",
            "--repo",
            REPOSITORY,
            "--add-assignee",
            "first-owner",
            "--add-assignee",
            "second-owner"
        ])
    );
    let states = f.states();
    let output = &states[0]["operation_attempts"][0]["output"];
    assert_eq!(output["action"], "assign");
    assert_eq!(
        output["issue"],
        json!({"repository": REPOSITORY, "number": 2553})
    );
    assert!(
        states[0].get("issue").is_none_or(Value::is_null),
        "an administrative target is not tracking"
    );
    let resumed = f.execute("recipe");
    assert!(resumed.status.success(), "{}", describe(&resumed));
    assert_eq!(f.calls("gh").len(), 1);
}
