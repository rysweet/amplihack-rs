//! Small recovery/ownership contracts that need no subprocess or environment.

use super::models::{PersistedState, Workstream, WorkstreamConfig, WorkstreamKey, derive_key};
use serde_json::json;

#[test]
fn attaching_tracking_cannot_change_an_explicit_immutable_key() {
    let mut value =
        json!({"key": "research-1", "branch": "feat/research", "task": "Report status"});
    let untracked: WorkstreamConfig = serde_json::from_value(value.clone()).unwrap();
    value["issue"] = json!(2553);
    let tracked: WorkstreamConfig = serde_json::from_value(value).unwrap();
    assert_eq!(derive_key(&untracked, 1).unwrap().as_str(), "research-1");
    assert_eq!(
        derive_key(&tracked, 1).unwrap(),
        derive_key(&untracked, 1).unwrap()
    );
}

#[test]
fn mismatched_saved_identity_cannot_supply_lifecycle_branch_or_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let mut workstream = Workstream::new(
        WorkstreamKey::new("ours").unwrap(),
        None,
        "feat/ours".into(),
        "Our work".into(),
        "Report status".into(),
        "operations-direct".into(),
        temp.path(),
        temp.path(),
    );
    let foreign = PersistedState {
        key: "foreign".into(),
        branch: "feat/foreign".into(),
        lifecycle_state: "completed".into(),
        worktree_path: temp.path().join("foreign").display().to_string(),
        ..Default::default()
    };
    super::state::apply_saved_state(&mut workstream, &foreign);
    assert_eq!(workstream.branch, "feat/ours");
    assert_eq!(workstream.lifecycle_state, "pending");
    assert!(workstream.worktree_path.is_empty());
}

#[cfg(unix)]
#[test]
fn atomic_persistence_refuses_a_symlinked_temporary_file_without_overwriting_its_target() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("caller-data");
    std::fs::write(&target, b"preserve me").unwrap();
    let state = temp.path().join("ws-owned.json");
    std::os::unix::fs::symlink(&target, state.with_extension("tmp")).unwrap();
    let result = super::utils::atomic_write(&state, b"new state");
    assert_eq!(std::fs::read(&target).unwrap(), b"preserve me");
    assert!(
        result.is_err(),
        "an existing symlink is an ownership collision"
    );
}

#[test]
fn issue_parser_rejects_overflow_and_nonintegral_values_at_the_shared_boundary() {
    for invalid in [
        json!("18446744073709551616"),
        json!("#18446744073709551616"),
        json!(2553.0),
        json!(false),
        json!({}),
        json!([]),
        json!("not-known"),
    ] {
        assert!(
            super::issue::parse_optional(&invalid).is_err(),
            "accepted {invalid}"
        );
    }
}

#[test]
fn cancellation_without_a_live_child_preserves_recovery_state_and_owned_artefacts() {
    let temp = tempfile::tempdir().unwrap();
    let mut workstream = Workstream::new(
        WorkstreamKey::new("cancelled").unwrap(),
        None,
        "".into(),
        "Status report".into(),
        "Report status".into(),
        "operations-direct".into(),
        temp.path(),
        temp.path(),
    );
    std::fs::create_dir_all(&workstream.work_dir).unwrap();
    std::fs::write(workstream.work_dir.join("answer"), b"partial answer").unwrap();
    workstream.lifecycle_state = "running".into();
    let state_file = workstream.state_file.clone();
    let work_dir = workstream.work_dir.clone();
    super::state::cleanup_running(std::slice::from_mut(&mut workstream), &Default::default());
    let saved: PersistedState =
        serde_json::from_slice(&std::fs::read(state_file).unwrap()).unwrap();
    assert_eq!(saved.lifecycle_state, "interrupted_resumable");
    assert!(!saved.cleanup_eligible);
    assert_eq!(saved.key, "cancelled");
    assert_eq!(
        std::fs::read(work_dir.join("answer")).unwrap(),
        b"partial answer"
    );
}
