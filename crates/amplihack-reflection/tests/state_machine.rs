// crates/amplihack-reflection/tests/state_machine.rs
//
// TDD: failing tests for ReflectionStateMachine port of
// amplifier-bundle/tools/amplihack/reflection/state_machine.py.

use amplihack_reflection::state_machine::{
    ReflectionState, ReflectionStateData, ReflectionStateMachine,
};
use tempfile::TempDir;

fn make_sm(sid: &str) -> (ReflectionStateMachine, TempDir) {
    let dir = TempDir::new().unwrap();
    let sm = ReflectionStateMachine::new(sid, dir.path()).unwrap();
    (sm, dir)
}

#[test]
fn initial_state_is_idle_when_no_file_exists() {
    let (sm, _d) = make_sm("sess");
    let s = sm.read_state().unwrap();
    assert_eq!(s.state, ReflectionState::Idle);
    assert_eq!(s.session_id.as_deref(), Some("sess"));
}

#[test]
fn write_then_read_roundtrip() {
    let (sm, _d) = make_sm("sess-rt");
    let mut data = ReflectionStateData::new("sess-rt");
    data.state = ReflectionState::Analyzing;
    data.analysis = Some(serde_json::json!({"patterns": []}));
    sm.write_state(&data).unwrap();
    let got = sm.read_state().unwrap();
    assert_eq!(got.state, ReflectionState::Analyzing);
    assert!(got.analysis.is_some());
}

#[test]
fn corrupt_state_file_resets_to_idle() {
    let dir = TempDir::new().unwrap();
    let sm = ReflectionStateMachine::new("corr", dir.path()).unwrap();
    std::fs::write(sm.state_file_path(), b"not json {{").unwrap();
    let s = sm.read_state().unwrap();
    assert_eq!(s.state, ReflectionState::Idle);
}

#[test]
fn valid_transitions_accepted() {
    let (sm, _d) = make_sm("t");
    for (from, to) in [
        (ReflectionState::Idle, ReflectionState::Analyzing),
        (
            ReflectionState::Analyzing,
            ReflectionState::AwaitingApproval,
        ),
        (
            ReflectionState::AwaitingApproval,
            ReflectionState::CreatingIssue,
        ),
        (
            ReflectionState::CreatingIssue,
            ReflectionState::AwaitingWorkDecision,
        ),
        (
            ReflectionState::AwaitingWorkDecision,
            ReflectionState::StartingWork,
        ),
        (ReflectionState::StartingWork, ReflectionState::Completed),
    ] {
        assert!(
            sm.can_transition(from, to),
            "{from:?} -> {to:?} should be allowed"
        );
    }
}

#[test]
fn invalid_transition_rejected() {
    let (sm, _d) = make_sm("inv");
    assert!(!sm.can_transition(ReflectionState::Idle, ReflectionState::Completed));
    assert!(!sm.can_transition(ReflectionState::Completed, ReflectionState::Analyzing));
}

#[test]
fn reset_returns_to_idle() {
    let (sm, _d) = make_sm("reset");
    let mut d = ReflectionStateData::new("reset");
    d.state = ReflectionState::CreatingIssue;
    sm.write_state(&d).unwrap();
    sm.reset().unwrap();
    assert_eq!(sm.read_state().unwrap().state, ReflectionState::Idle);
}
