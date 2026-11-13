//! Identity, owned artefacts and guarded resume/reconciliation contracts.

use crate::issue_2553_fixture::{FIRST, Fixture, REPOSITORY, SECOND, describe, leaf, plan};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

fn two_leaves() -> Value {
    let mut a = leaf("status", "Operations", "Report status in chat.");
    a["workstream_id"] = json!(FIRST);
    let mut b = leaf(
        "research",
        "Operations",
        "Answer the research questions in chat.",
    );
    b["workstream_id"] = json!(SECOND);
    plan(vec![a, b])
}

fn save_state(f: &Fixture, state: &Value) {
    let dir = f.tmp.join("amplihack-workstreams/state");
    let path = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json"))
        .find(|p| {
            serde_json::from_slice::<Value>(&fs::read(p).unwrap()).unwrap()["workstream_id"]
                == state["workstream_id"]
        })
        .expect("existing owned state file");
    fs::write(path, serde_json::to_vec_pretty(state).unwrap()).unwrap();
}

#[test]
fn multiple_issue_less_leaves_have_separate_paths_processes_and_persisted_identity() {
    let f = Fixture::new();
    f.normalise(&two_leaves());
    let out = f.execute("recipe");
    assert!(out.status.success(), "{}", describe(&out));
    f.assert_direct(2);
    let states = f.states();
    assert_eq!(states.len(), 2, "state must not collide at ws-0.json");
    for field in [
        "workstream_id",
        "log_file",
        "progress_sidecar",
        "artefact_dir",
    ] {
        assert!(
            states[0][field].as_str().is_some_and(|v| !v.is_empty()),
            "{field}: {states:#?}"
        );
        assert_ne!(states[0][field], states[1][field], "{field}");
    }
    for state in &states {
        assert_eq!(state["work_dir"], f.repo.to_str().unwrap());
        assert!(Path::new(state["artefact_dir"].as_str().unwrap()).starts_with(&f.tmp));
        assert_ne!(state["artefact_dir"], state["work_dir"]);
        assert_eq!(
            state["process_scope"]["workstream_id"],
            state["workstream_id"]
        );
        assert_eq!(state["process_scope"]["repository"], REPOSITORY);
        assert!(state.get("issue").is_none(), "{state:#}");
    }
    assert_ne!(
        states[0]["process_scope"]["pid"],
        states[1]["process_scope"]["pid"]
    );
    assert!(
        f.repo.join("README.md").is_file(),
        "Operations cleanup must preserve the repository"
    );
}

#[test]
fn persisted_success_is_skipped_on_resume_without_renaming_paths() {
    let f = Fixture::new();
    f.normalise(&two_leaves());
    let first = f.execute("recipe");
    assert!(first.status.success(), "{}", describe(&first));
    let before = f.states();
    let resumed = f.execute("recipe");
    assert!(resumed.status.success(), "{}", describe(&resumed));
    assert_eq!(
        f.calls("execute").len(),
        2,
        "no replay of durable successes"
    );
    let after = f.states();
    for state in &before {
        let same = after
            .iter()
            .find(|s| s["workstream_id"] == state["workstream_id"])
            .unwrap();
        for field in ["work_dir", "log_file", "progress_sidecar", "artefact_dir"] {
            assert_eq!(same[field], state[field], "{field}");
        }
    }
}

fn interrupted(f: &Fixture) -> Value {
    let mut item = leaf("assign", "Operations", "Assign issue #2553 to owner.");
    item["workstream_id"] = json!(FIRST);
    item["required_outputs"] = json!({"assignment": "operation"});
    f.normalise(&plan(vec![item]));
    let audit = f.tmp.join("assignment-audit.json");
    fs::write(
        &audit,
        serde_json::to_vec(&json!({"repository": REPOSITORY,
        "issue": 2553, "assignees": ["owner"], "source": "isolated-service"}))
        .unwrap(),
    )
    .unwrap();
    let result = json!({"schema_version": 1, "workstream_id": FIRST, "repository": REPOSITORY,
        "classification": "Operations", "effective_recipe": "operations-direct", "outcome": "succeeded",
        "outputs": {"assignment": {"kind": "operation", "action": "assign",
            "issue": {"repository": REPOSITORY, "number": 2553},
            "evidence": [{"source": "file", "reference": audit,
                "observation": "Issue #2553 is assigned to owner"}]}}});
    fs::write(
        f.root.join(format!("result-{FIRST}.json")),
        serde_json::to_vec(&result).unwrap(),
    )
    .unwrap();
    let out = f.execute("recipe");
    assert!(out.status.success(), "{}", describe(&out));
    let mut state = f
        .states()
        .into_iter()
        .find(|s| s["workstream_id"] == FIRST)
        .unwrap();
    state["lifecycle"] = json!("running");
    state["revision"] = json!(3);
    state["last_exit_code"] = Value::Null;
    state.as_object_mut().unwrap().remove("result");
    state["operation_attempts"] = json!([{"schema_version": 1,
        "attempt_id": "aebff9b8-b66b-4c03-924f-d5881c07d21c", "workstream_id": FIRST,
        "repository": REPOSITORY, "revision": 3, "purpose": "operation",
        "request": {"action": "assign", "arguments": {"issue": 2553, "assignee": "owner"}},
        "output_name": "assignment", "status": "prepared"}]);
    save_state(f, &state);
    state
}

#[test]
fn interrupted_external_mutation_requires_reconciliation_instead_of_automatic_replay() {
    let f = Fixture::new();
    interrupted(&f);
    let out = f.execute("recipe");
    assert!(!out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.calls("execute").len(),
        1,
        "a recovered prepared action is not safe to replay"
    );
    assert!(
        f.states()
            .iter()
            .any(|s| s["lifecycle"] == "reconciliation_required")
    );
}

#[test]
fn reconciliation_is_revision_bound_durable_and_idempotent_without_launching() {
    let f = Fixture::new();
    let mut state = interrupted(&f);
    state["lifecycle"] = json!("reconciliation_required");
    save_state(&f, &state);
    let audit = f.tmp.join("uncertainty-audit.json");
    fs::write(
        &audit,
        serde_json::to_vec(&json!({"repository": REPOSITORY,
        "attempt_id": "aebff9b8-b66b-4c03-924f-d5881c07d21c", "outcome": "unknown",
        "observation": "No authoritative receipt for the interrupted attempt"}))
        .unwrap(),
    )
    .unwrap();
    let resolution = json!({"schema_version": 1, "resolution_id": "92fdc5a0-ac4d-4a22-9f25-c9832475f5bc",
        "workstream_id": FIRST, "repository": REPOSITORY, "expected_revision": 3,
        "attempt_id": "aebff9b8-b66b-4c03-924f-d5881c07d21c", "outcome": "unresolved",
        "evidence": [{"source": "file", "reference": audit, "observation": "The assignment outcome remains unknown"}],
        "reason": "No authoritative assignment receipt is available"});
    let path = f.root.join("resolution.json");
    fs::write(&path, serde_json::to_vec(&resolution).unwrap()).unwrap();
    let reconcile = || {
        f.command("cli")
            .args(["multitask", "reconcile"])
            .arg(&f.config)
            .arg("--resolution-file")
            .arg(&path)
            .output()
            .unwrap()
    };
    let first = reconcile();
    assert!(first.status.success(), "{}", describe(&first));
    let receipt: Value = serde_json::from_slice(&first.stdout).expect("resolution receipt");
    assert_eq!(receipt["workstream_id"], FIRST);
    assert!(receipt["revision"].as_u64().is_some_and(|v| v > 3));
    let duplicate = reconcile();
    assert!(duplicate.status.success(), "{}", describe(&duplicate));
    assert_eq!(
        serde_json::from_slice::<Value>(&duplicate.stdout).unwrap(),
        receipt
    );
    let mut stale = resolution;
    stale["resolution_id"] = json!("38645ae7-a8a8-4c03-9017-5297c8b3d508");
    fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();
    let rejected = reconcile();
    assert!(
        !rejected.status.success(),
        "stale revision must fail: {}",
        describe(&rejected)
    );
    assert_eq!(
        f.calls("execute").len(),
        1,
        "reconciliation never launches work"
    );
    assert!(
        f.calls("gh").is_empty(),
        "reconciliation does not mutate GitHub"
    );
}

#[test]
fn saved_foreign_repository_or_escaped_artefact_path_cannot_be_adopted() {
    for field in ["repository", "artefact_dir"] {
        let f = Fixture::new();
        let mut state = interrupted(&f);
        state[field] = if field == "repository" {
            json!("example/foreign")
        } else {
            json!(f.root.join("outside-runtime"))
        };
        save_state(&f, &state);
        let out = f.execute("recipe");
        assert!(!out.status.success(), "{field}: {}", describe(&out));
        assert_eq!(
            f.calls("execute").len(),
            1,
            "foreign or escaped state is not adopted"
        );
    }
}
