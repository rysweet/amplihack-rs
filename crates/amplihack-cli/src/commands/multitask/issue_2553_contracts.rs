//! Model-boundary units for the #2553 TDD contracts. No processes or real homes.

use super::models::{PersistedState, Workstream, WorkstreamConfig, WorkstreamScope};
use serde_json::{Value, json};

const ID: &str = "8f9f57b5-9232-49df-b985-c2c92ee997c1";

#[test]
fn configuration_accepts_an_omitted_issue_and_no_operations_branch() {
    let config: WorkstreamConfig = serde_json::from_value(json!({"workstream_id": ID,
        "key": "report", "repository": "example/operations", "classification": "Operations",
        "task": "Report status in chat.", "recipe": "operations-direct"}))
    .unwrap();
    let saved = serde_json::to_value(config).unwrap();
    assert!(saved.get("issue").is_none(), "{saved:#}");
    assert!(saved.get("branch").is_none(), "{saved:#}");
    assert_eq!(saved["tracking"], json!({"required": false}));
}

#[test]
fn direct_configuration_boundary_rejects_malformed_issues() {
    for issue in [
        json!(0),
        json!(-2),
        json!(1.5),
        json!(false),
        json!("#2553"),
        json!("18446744073709551616"),
    ] {
        let parsed = serde_json::from_value::<WorkstreamConfig>(json!({"issue": issue,
            "branch": "feat/test", "task": "Report status in chat."}));
        assert!(
            parsed.is_err(),
            "configuration must use the shared issue validator: {issue}"
        );
    }
}

#[test]
fn config_serialisation_canonicalises_issues_without_losing_u64_max() {
    for (input, expected) in [
        (json!(" 002553 "), Some(2553)),
        (json!(u64::MAX), Some(u64::MAX)),
        (json!("tBd"), None),
        (json!(null), None),
    ] {
        let parsed: WorkstreamConfig = serde_json::from_value(json!({"issue": input,
            "branch": "feat/test", "task": "Report status in chat."}))
        .unwrap();
        let saved = serde_json::to_value(parsed).unwrap();
        match expected {
            Some(number) => assert_eq!(saved["issue"].as_u64(), Some(number)),
            None => assert!(saved.get("issue").is_none(), "{saved:#}"),
        }
    }
}

#[test]
fn configuration_round_trip_preserves_identity_dependencies_and_tracking_binding() {
    let input = json!({"issue": 2553, "branch": "feat/test", "task": "Report status in chat.",
        "workstream_id": ID, "key": "report", "repository": "example/operations",
        "classification": "Operations", "depends_on": ["64a1ff65-5a30-4c03-9c60-6500744fb328"],
        "required_outputs": {"answer": "answer"}, "tracking": {"required": false}});
    let parsed: WorkstreamConfig = serde_json::from_value(input.clone()).unwrap();
    let saved = serde_json::to_value(parsed).unwrap();
    for field in [
        "workstream_id",
        "key",
        "repository",
        "classification",
        "depends_on",
        "required_outputs",
        "tracking",
    ] {
        assert_eq!(
            saved[field], input[field],
            "lost configuration field {field}"
        );
    }
}

#[test]
fn persisted_state_preserves_result_attempt_and_reconciliation_history() {
    let input = json!({"workstream_id": ID, "repository": "example/operations", "revision": 4,
        "classification": "Operations", "lifecycle": "reconciliation_required", "depends_on": [],
        "artefact_dir": "owned/runtime/leaf", "tracking": {"required": false},
        "tracking_attempts": [{"schema_version": 1, "attempt_id": "aebff9b8-b66b-4c03-924f-d5881c07d21c",
            "workstream_id": ID, "repository": "example/operations", "revision": 3, "purpose": "tracking",
            "request": {"title": "Tracking", "body": "Requested tracking issue"},
            "status": "reconciliation_required", "diagnostic": "Response lost"}],
        "resolutions": [], "blocked_by": [], "required_outputs": {"answer": "answer"}});
    let state: PersistedState = serde_json::from_value(input.clone()).unwrap();
    let saved = serde_json::to_value(state).unwrap();
    for (field, value) in input.as_object().unwrap() {
        assert_eq!(&saved[field], value, "lost persisted field {field}");
    }
    assert!(
        saved.get("issue").is_none(),
        "absence must not become issue zero"
    );
}

#[test]
fn unknown_persisted_lifecycle_is_not_treated_as_pending() {
    let parsed = serde_json::from_value::<PersistedState>(json!({"workstream_id": ID,
        "repository": "example/operations", "revision": 1, "lifecycle": "almost-succeeded"}));
    assert!(parsed.is_err(), "unknown lifecycle must fail closed");
}

#[test]
fn optional_tracking_defaults_for_legacy_configuration_without_losing_valid_issue() {
    let legacy: WorkstreamConfig = serde_json::from_value(json!({"issue": "2553",
        "branch": "feat/legacy", "task": "Change the tracked documentation."}))
    .unwrap();
    let saved = serde_json::to_value(legacy).unwrap();
    assert_eq!(saved["issue"], 2553);
    assert_eq!(saved["tracking"], json!({"required": false}));
}

#[test]
fn legacy_saved_state_adoption_preserves_recorded_paths() {
    let temp = tempfile::tempdir().unwrap();
    let mut ws = Workstream::new(
        2553,
        "feat/legacy".into(),
        "Legacy task".into(),
        "Change documentation.".into(),
        "default-workflow".into(),
        temp.path(),
        temp.path(),
    );
    let working = temp.path().join("preserved-working");
    std::fs::create_dir(&working).unwrap();
    let scope = WorkstreamScope {
        repository: "example/operations".into(),
        branch: "feat/legacy".into(),
        repo_root: working.to_string_lossy().into(),
        workdir: working.to_string_lossy().into(),
        ..Default::default()
    };
    ws.workstream_scope = scope.clone();
    let state = PersistedState {
        issue: 2553,
        branch: "feat/legacy".into(),
        work_dir: working.to_string_lossy().into(),
        log_file: temp.path().join("old.log").to_string_lossy().into(),
        progress_sidecar: temp
            .path()
            .join("old.progress.json")
            .to_string_lossy()
            .into(),
        workstream_scope: scope,
        ..Default::default()
    };
    super::state::apply_saved_state(&mut ws, &state);
    assert_eq!(ws.work_dir, working);
    assert_eq!(ws.log_file.to_string_lossy(), state.log_file);
    assert_eq!(ws.progress_file.to_string_lossy(), state.progress_sidecar);
}

#[test]
fn state_does_not_accept_identity_as_a_substitute_for_repository_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let mut ws = Workstream::new(
        2553,
        "feat/ours".into(),
        "Our task".into(),
        "Report status.".into(),
        "operations-direct".into(),
        temp.path(),
        temp.path(),
    );
    ws.workstream_scope = WorkstreamScope {
        repository: "example/ours".into(),
        branch: "feat/ours".into(),
        workstream_id: ID.into(),
        ..Default::default()
    };
    let state = PersistedState {
        issue: 2553,
        branch: "feat/foreign".into(),
        workstream_scope: WorkstreamScope {
            repository: "example/foreign".into(),
            branch: "feat/foreign".into(),
            workstream_id: ID.into(),
            ..Default::default()
        },
        ..Default::default()
    };
    super::state::apply_saved_state(&mut ws, &state);
    assert_eq!(ws.workstream_scope.repository, "example/ours");
    assert_eq!(ws.branch, "feat/ours");
}

#[test]
fn zero_exit_without_a_validated_result_does_not_prove_success() {
    let temp = tempfile::tempdir().unwrap();
    let mut ws = Workstream::new(
        2553,
        String::new(),
        "Answer".into(),
        "Answer questions.".into(),
        "operations-direct".into(),
        temp.path(),
        temp.path(),
    );
    super::state::finalize_workstream(&mut ws, 0);
    let saved: Value = serde_json::from_slice(&std::fs::read(&ws.state_file).unwrap()).unwrap();
    assert_ne!(
        saved["lifecycle_state"], "completed",
        "exit status alone is not a task result"
    );
    assert_ne!(saved["lifecycle"], "succeeded");
}
