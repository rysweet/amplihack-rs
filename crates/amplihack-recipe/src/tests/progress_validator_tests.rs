use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn make_payload(status: &str, step: &str, ts: f64, pid: u32) -> String {
    format!(
        r#"{{"status":"{status}","step_name":"{step}","timestamp":{ts},"pid":{pid},"recipe_name":"test_recipe"}}"#
    )
}

// ── Filename validation ──────────────────────────────────────────────

#[test]
fn valid_filename() {
    let (name, pid) = validate_filename("amplihack-progress-my_recipe-1234.json").unwrap();
    assert_eq!(name, "my_recipe");
    assert_eq!(pid, 1234);
}

#[test]
fn reject_bad_filename_no_prefix() {
    assert!(validate_filename("bad-file.json").is_err());
}

#[test]
fn reject_bad_filename_special_chars() {
    assert!(validate_filename("amplihack-progress-../../etc-99.json").is_err());
}

#[test]
fn reject_empty_safe_name() {
    assert!(validate_filename("amplihack-progress--42.json").is_err());
}

#[test]
fn reject_safe_name_too_long() {
    let long_name = "a".repeat(65);
    let fname = format!("amplihack-progress-{long_name}-1.json");
    assert!(validate_filename(&fname).is_err());
}

// ── Field validation ─────────────────────────────────────────────────

#[test]
fn reject_step_name_too_long() {
    let pid = std::process::id();
    let long_step = "x".repeat(257);
    let json = format!(
        r#"{{"status":"running","step_name":"{long_step}","timestamp":{ts},"pid":{pid},"recipe_name":"test_recipe"}}"#,
        ts = now_ts()
    );
    let fname = format!("amplihack-progress-test_recipe-{pid}.json");
    let err = validate_progress_file(&fname, json.as_bytes(), None).unwrap_err();
    assert!(matches!(err, ValidationError::StepNameTooLong(257)));
}

#[test]
fn reject_invalid_status_in_json() {
    let pid = std::process::id();
    let json = format!(
        r#"{{"status":"bogus","step_name":"s","timestamp":{ts},"pid":{pid},"recipe_name":"test_recipe"}}"#,
        ts = now_ts()
    );
    let fname = format!("amplihack-progress-test_recipe-{pid}.json");
    assert!(matches!(
        validate_progress_file(&fname, json.as_bytes(), None),
        Err(ValidationError::ParseError(_))
    ));
}

// ── Transition validation ────────────────────────────────────────────

#[test]
fn valid_running_to_completed() {
    assert!(validate_transition(ProgressStatus::Running, ProgressStatus::Completed).is_ok());
}

#[test]
fn reject_completed_to_running() {
    assert!(matches!(
        validate_transition(ProgressStatus::Completed, ProgressStatus::Running),
        Err(ValidationError::InvalidTransition { .. })
    ));
}

#[test]
fn same_status_transition_ok() {
    assert!(validate_transition(ProgressStatus::Running, ProgressStatus::Running).is_ok());
}

// ── Age validation ───────────────────────────────────────────────────

#[test]
fn reject_stale_timestamp() {
    let old = now_ts() - 8000.0;
    let err = validate_age(old).unwrap_err();
    assert!(matches!(err, ValidationError::Stale { .. }));
}

#[test]
fn reject_future_timestamp() {
    let future = now_ts() + 120.0;
    let err = validate_age(future).unwrap_err();
    assert!(matches!(err, ValidationError::FutureDated { .. }));
}

#[test]
fn accept_recent_timestamp() {
    assert!(validate_age(now_ts() - 10.0).is_ok());
}

// ── PID validation ───────────────────────────────────────────────────

#[test]
fn current_pid_is_alive() {
    assert!(is_pid_alive(std::process::id()));
}

#[test]
fn bogus_pid_is_not_alive() {
    // PID 4_000_000 is virtually guaranteed not to exist.
    assert!(!is_pid_alive(4_000_000));
}

// ── Full validation ──────────────────────────────────────────────────

#[test]
fn full_valid_payload() {
    let pid = std::process::id();
    let json = make_payload("running", "step-0", now_ts(), pid);
    let fname = format!("amplihack-progress-test_recipe-{pid}.json");
    let p = validate_progress_file(&fname, json.as_bytes(), None).unwrap();
    assert_eq!(p.status, ProgressStatus::Running);
    assert_eq!(p.step_name, "step-0");
}

#[test]
fn reject_pid_mismatch() {
    let pid = std::process::id();
    let json = make_payload("running", "s", now_ts(), 99999);
    let fname = format!("amplihack-progress-test_recipe-{pid}.json");
    let err = validate_progress_file(&fname, json.as_bytes(), None).unwrap_err();
    assert!(matches!(err, ValidationError::PidMismatch(_, _)));
}

#[test]
fn reject_recipe_name_mismatch() {
    let pid = std::process::id();
    let json = format!(
        r#"{{"status":"running","step_name":"s","timestamp":{ts},"pid":{pid},"recipe_name":"other_recipe"}}"#,
        ts = now_ts()
    );
    let fname = format!("amplihack-progress-test_recipe-{pid}.json");
    let err = validate_progress_file(&fname, json.as_bytes(), None).unwrap_err();
    assert!(matches!(err, ValidationError::BadFilename(_)));
}

// ── safe_progress_name ───────────────────────────────────────────────

#[test]
fn safe_name_strips_special_chars() {
    assert_eq!(safe_progress_name("my-recipe/v2!"), "v2_");
    assert_eq!(safe_progress_name("hello world"), "hello_world");
}

// ── Workstream sidecar (PR #4075) ────────────────────────────────────

#[test]
fn workstream_state_round_trip() {
    let ws = WorkstreamState {
        workstream_id: "ws-1".into(),
        status: ProgressStatus::Running,
        last_step: Some("step-3".into()),
        timestamp: 1700000000.0,
        error_message: None,
        elapsed_seconds: Some(42.0),
    };
    let json = serde_json::to_string(&ws).unwrap();
    let parsed: WorkstreamState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.workstream_id, "ws-1");
    assert_eq!(parsed.status, ProgressStatus::Running);
    assert_eq!(parsed.elapsed_seconds, Some(42.0));
}

#[test]
fn read_workstream_state_missing_file() {
    let states = read_workstream_state(std::path::Path::new("/nonexistent/file.json"));
    assert!(states.is_empty());
}

#[test]
fn read_workstream_state_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ws-state.json");
    let data = r#"[{"workstream_id":"w1","status":"completed","timestamp":1.0}]"#;
    std::fs::write(&path, data).unwrap();
    let states = read_workstream_state(&path);
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].workstream_id, "w1");
}

#[test]
fn merge_workstream_creates_progress_file() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.json");
    let progress_path = dir.path().join("progress.json");

    let state_data = serde_json::json!([
        {"workstream_id": "ws-a", "status": "running", "timestamp": 1.0},
        {"workstream_id": "ws-b", "status": "completed", "timestamp": 2.0}
    ]);
    std::fs::write(&state_path, state_data.to_string()).unwrap();

    merge_workstream_state_into_progress(&state_path, &progress_path).unwrap();

    let result: Vec<WorkstreamState> =
        serde_json::from_str(&std::fs::read_to_string(&progress_path).unwrap()).unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn merge_workstream_updates_existing() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.json");
    let progress_path = dir.path().join("progress.json");

    // Pre-existing progress
    let existing = serde_json::json!([
        {"workstream_id": "ws-a", "status": "running", "timestamp": 1.0}
    ]);
    std::fs::write(&progress_path, existing.to_string()).unwrap();

    // New state with updated status
    let new_state = serde_json::json!([
        {"workstream_id": "ws-a", "status": "completed", "timestamp": 2.0},
        {"workstream_id": "ws-c", "status": "running", "timestamp": 3.0}
    ]);
    std::fs::write(&state_path, new_state.to_string()).unwrap();

    merge_workstream_state_into_progress(&state_path, &progress_path).unwrap();

    let result: Vec<WorkstreamState> =
        serde_json::from_str(&std::fs::read_to_string(&progress_path).unwrap()).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].workstream_id, "ws-a");
    assert_eq!(result[0].status, ProgressStatus::Completed);
    assert_eq!(result[1].workstream_id, "ws-c");
}

#[test]
fn merge_workstream_noop_when_state_empty() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.json");
    let progress_path = dir.path().join("progress.json");
    std::fs::write(&state_path, "[]").unwrap();

    merge_workstream_state_into_progress(&state_path, &progress_path).unwrap();
    assert!(!progress_path.exists());
}

#[test]
fn workstream_env_paths_unset() {
    // These env vars are not expected to be set in test env.
    // Just verify the functions return None gracefully.
    unsafe {
        std::env::remove_var("AMPLIHACK_WORKSTREAM_PROGRESS_FILE");
        std::env::remove_var("AMPLIHACK_WORKSTREAM_STATE_FILE");
    }
    assert!(workstream_progress_sidecar_path().is_none());
    assert!(workstream_state_file_path().is_none());
}
