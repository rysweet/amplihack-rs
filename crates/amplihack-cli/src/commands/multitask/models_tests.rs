//! Tests for the multitask config and state models (`models.rs`).

use super::models::*;

#[test]
fn test_sanitize_id() {
    assert_eq!(sanitize_id("123"), "123");
    assert_eq!(sanitize_id("hello-world"), "hello-world");
    assert_eq!(sanitize_id("path/../../etc"), "path_______etc");
    assert_eq!(sanitize_id("a b c"), "a_b_c");
}

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
    assert_eq!(configs[0].tracking_issue(), Ok(Some(42)));
    assert_eq!(configs[0].workstream_id(), Ok(42));
    assert_eq!(configs[0].branch, "feat/test");
    assert_eq!(configs[0].description_or_default(), "Test workstream");
}

#[test]
fn test_workstream_config_string_issue() {
    let json = r#"[{"issue": "99", "branch": "b", "task": "t"}]"#;
    let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
    assert_eq!(configs[0].tracking_issue(), Ok(Some(99)));
}

#[test]
fn test_workstream_config_default_description() {
    let json = r#"[{"issue": 7, "branch": "b", "task": "t"}]"#;
    let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
    assert_eq!(configs[0].description_or_default(), "Issue #7");
}

/// #1900: several workstreams may share the caller's issue; each has its
/// own `id`, which keys its work dir.
#[test]
fn shared_issue_with_distinct_ids() {
    let json = r#"[
        {"id": 11, "issue": 368, "branch": "a", "task": "t"},
        {"id": 12, "issue": "368", "branch": "b", "task": "t"}
    ]"#;
    let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
    assert_eq!(configs[0].tracking_issue(), Ok(Some(368)));
    assert_eq!(configs[1].tracking_issue(), Ok(Some(368)));
    assert_eq!(configs[0].workstream_id(), Ok(11));
    assert_eq!(configs[1].workstream_id(), Ok(12));
}

/// #1900: no `issue` means local tracking, keyed by `id`.
#[test]
fn id_without_issue_is_local_tracking() {
    let json = r#"[{"id": 5, "branch": "b", "task": "t",
                    "context": {"create_issue": "false"}}]"#;
    let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
    assert_eq!(configs[0].tracking_issue(), Ok(None));
    assert_eq!(configs[0].workstream_id(), Ok(5));
    assert_eq!(configs[0].context["create_issue"], "false");
    assert_eq!(configs[0].description_or_default(), "Workstream 5");
}

/// #1900: "TBD" is not a placeholder any more; it is refused with a
/// message that says what to do instead, even when `id` is set.
#[test]
fn tbd_issue_is_rejected() {
    for json in [
        r#"[{"issue": "TBD", "branch": "b", "task": "t"}]"#,
        r#"[{"issue": "TBD", "id": 3, "branch": "b", "task": "t"}]"#,
    ] {
        let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
        let err = configs[0].workstream_id().unwrap_err();
        assert!(
            err.contains("Invalid issue number") && err.contains("local tracking"),
            "{err}"
        );
    }
    let none: Vec<WorkstreamConfig> =
        serde_json::from_str(r#"[{"branch": "b", "task": "t"}]"#).unwrap();
    assert!(none[0].workstream_id().unwrap_err().contains("neither"));
}

#[test]
fn test_persisted_state_roundtrip() {
    let state = PersistedState {
        issue: 42,
        branch: "feat/test".to_string(),
        lifecycle_state: "completed".to_string(),
        ..Default::default()
    };
    let json = serde_json::to_string(&state).unwrap();
    let parsed: PersistedState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.issue, 42);
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
