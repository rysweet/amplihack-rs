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
