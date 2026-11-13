use super::issue::IssueNumber;
use super::models::*;

#[test]
fn test_workstream_config_parsing() {
    let json = r#"[
            {
                "issue": 42,
                "branch": "feat/test",
                "description": "Test workstream",
                "task": "Do something",
                "recipe": "default-workflow"
            }
        ]"#;
    let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].issue().unwrap().map(IssueNumber::get), Some(42));
    assert_eq!(configs[0].branch, "feat/test");
    assert_eq!(configs[0].description_or_default(), "Test workstream");
}

#[test]
fn test_workstream_config_string_issue() {
    let json = r#"[{"issue": "99", "branch": "b", "task": "t"}]"#;
    let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
    assert_eq!(configs[0].issue().unwrap().map(IssueNumber::get), Some(99));
}

#[test]
fn test_workstream_config_default_description() {
    let json = r#"[{"issue": 7, "branch": "b", "task": "t"}]"#;
    let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
    assert_eq!(configs[0].description_or_default(), "Issue #7");
}

#[test]
fn test_persisted_state_roundtrip() {
    let state = PersistedState {
        key: "42".to_string(),
        issue: Some(42),
        branch: "feat/test".to_string(),
        lifecycle_state: "completed".to_string(),
        ..Default::default()
    };
    let json = serde_json::to_string(&state).unwrap();
    let parsed: PersistedState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.issue, Some(42));
    assert_eq!(parsed.branch, "feat/test");
    assert_eq!(parsed.lifecycle_state, "completed");
}

#[test]
fn test_derive_cleanup_eligible() {
    assert!(Workstream::derive_cleanup_eligible("completed"));
    assert!(Workstream::derive_cleanup_eligible("failed_terminal"));
    assert!(Workstream::derive_cleanup_eligible("abandoned"));
    assert!(!Workstream::derive_cleanup_eligible("running"));
    assert!(!Workstream::derive_cleanup_eligible("pending"));
    assert!(!Workstream::derive_cleanup_eligible("failed_resumable"));
}

#[test]
fn test_default_constants() {
    assert_eq!(DEFAULT_MAX_RUNTIME, 7200);
    assert_eq!(DEFAULT_TIMEOUT_POLICY, "continue-preserve");
}
