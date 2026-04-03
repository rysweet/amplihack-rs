//! Progress file validation hardening (ported from Python PR #3904).
//!
//! Validates progress signal files written by the recipe runner to prevent
//! spoofing.  Checks filename shape, required fields, status values, state
//! transitions, timestamp freshness, and PID liveness.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

// ── Constants ────────────────────────────────────────────────────────────────

const MAX_PROGRESS_AGE_SECS: f64 = 7200.0;
const MAX_FUTURE_DRIFT_SECS: f64 = 30.0;
const MAX_STEP_NAME_LEN: usize = 256;
const MAX_SAFE_NAME_LEN: usize = 64;

static FILENAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^amplihack-progress-(?<safe_name>[a-zA-Z0-9_]{1,64})-(?<pid>\d+)\.json$")
        .expect("compiled filename regex")
});

static SAFE_CHAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^a-zA-Z0-9_]").expect("compiled safe-char regex"));

// ── Public types ─────────────────────────────────────────────────────────────

/// Allowed progress statuses (matches Python `_ALLOWED_PROGRESS_STATUSES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStatus {
    Running,
    Completed,
    Failed,
    Skipped,
    Unknown,
}

impl ProgressStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Skipped)
    }

    /// Returns the set of statuses this status may transition to.
    pub fn valid_transitions(self) -> &'static [ProgressStatus] {
        match self {
            Self::Running => &[Self::Completed, Self::Failed, Self::Skipped],
            Self::Unknown => &[Self::Running, Self::Completed, Self::Failed, Self::Skipped],
            _ => &[], // terminal
        }
    }
}

impl fmt::Display for ProgressStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Skipped => write!(f, "skipped"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Deserialized progress file payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressPayload {
    pub status: ProgressStatus,
    pub step_name: String,
    #[serde(alias = "updated_at")]
    pub timestamp: f64,
    #[serde(default)]
    pub recipe_name: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub current_step: Option<u32>,
    #[serde(default)]
    pub total_steps: Option<u32>,
    #[serde(default)]
    pub elapsed_seconds: Option<f64>,
}

/// Validation errors with descriptive messages.
#[derive(Debug, Error, PartialEq)]
pub enum ValidationError {
    #[error("filename does not match pattern amplihack-progress-{{safe_name}}-{{pid}}.json: {0}")]
    BadFilename(String),
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("step_name exceeds {MAX_STEP_NAME_LEN} chars ({0} chars)")]
    StepNameTooLong(usize),
    #[error("invalid status transition: {from} → {to}")]
    InvalidTransition { from: ProgressStatus, to: ProgressStatus },
    #[error("progress file is stale (age {age_secs:.0}s exceeds {MAX_PROGRESS_AGE_SECS}s limit)")]
    Stale { age_secs: f64 },
    #[error("timestamp is {drift_secs:.1}s in the future (max {MAX_FUTURE_DRIFT_SECS}s)")]
    FutureDated { drift_secs: f64 },
    #[error("PID {0} in filename does not match payload PID {1}")]
    PidMismatch(u32, u32),
    #[error("PID {0} is not a running process")]
    PidNotAlive(u32),
    #[error("failed to parse progress JSON: {0}")]
    ParseError(String),
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Sanitize a recipe name to the safe filename stem format.
pub fn safe_progress_name(name: &str) -> String {
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    let sanitized = SAFE_CHAR_RE.replace_all(stem, "_");
    let mut s = sanitized.into_owned();
    s.truncate(MAX_SAFE_NAME_LEN);
    s
}

/// Parse filename and return `(safe_name, pid)` or an error.
pub fn validate_filename(filename: &str) -> Result<(String, u32), ValidationError> {
    let caps = FILENAME_RE
        .captures(filename)
        .ok_or_else(|| ValidationError::BadFilename(filename.to_owned()))?;
    let safe_name = caps["safe_name"].to_owned();
    let pid: u32 = caps["pid"]
        .parse()
        .map_err(|_| ValidationError::BadFilename(filename.to_owned()))?;
    Ok((safe_name, pid))
}

/// Validate required fields and field constraints on a payload.
pub fn validate_fields(payload: &ProgressPayload) -> Result<(), ValidationError> {
    if payload.step_name.len() > MAX_STEP_NAME_LEN {
        return Err(ValidationError::StepNameTooLong(payload.step_name.len()));
    }
    if payload.timestamp <= 0.0 {
        return Err(ValidationError::MissingField("timestamp"));
    }
    Ok(())
}

/// Validate a status transition.
pub fn validate_transition(
    from: ProgressStatus,
    to: ProgressStatus,
) -> Result<(), ValidationError> {
    if from == to {
        return Ok(());
    }
    if from.valid_transitions().contains(&to) {
        Ok(())
    } else {
        Err(ValidationError::InvalidTransition { from, to })
    }
}

/// Validate timestamp freshness.
pub fn validate_age(timestamp: f64) -> Result<(), ValidationError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let drift = timestamp - now;
    if drift > MAX_FUTURE_DRIFT_SECS {
        return Err(ValidationError::FutureDated { drift_secs: drift });
    }
    let age = now - timestamp;
    if age > MAX_PROGRESS_AGE_SECS {
        return Err(ValidationError::Stale { age_secs: age });
    }
    Ok(())
}

/// Check whether a PID corresponds to a running process (signal 0).
pub fn is_pid_alive(pid: u32) -> bool {
    // SAFETY: `kill(pid, 0)` only checks existence — sends no signal.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Full validation of a progress file given its filename and raw JSON bytes.
///
/// If `previous_status` is provided, transition validation is also performed.
pub fn validate_progress_file(
    filename: &str,
    json_bytes: &[u8],
    previous_status: Option<ProgressStatus>,
) -> Result<ProgressPayload, ValidationError> {
    let (safe_name, file_pid) = validate_filename(filename)?;

    let payload: ProgressPayload =
        serde_json::from_slice(json_bytes).map_err(|e| ValidationError::ParseError(e.to_string()))?;

    // Field constraints
    validate_fields(&payload)?;

    // PID consistency
    if let Some(p) = payload.pid
        && p != file_pid
    {
        return Err(ValidationError::PidMismatch(file_pid, p));
    }

    // Recipe-name consistency
    if let Some(ref rn) = payload.recipe_name
        && safe_progress_name(rn) != safe_name
    {
        return Err(ValidationError::BadFilename(format!(
            "recipe_name '{rn}' does not match filename stem '{safe_name}'"
        )));
    }

    // Freshness
    validate_age(payload.timestamp)?;

    // PID liveness
    if !is_pid_alive(file_pid) {
        return Err(ValidationError::PidNotAlive(file_pid));
    }

    // Transition
    if let Some(prev) = previous_status {
        validate_transition(prev, payload.status)?;
    }

    Ok(payload)
}

#[cfg(test)]
mod tests {
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
}
