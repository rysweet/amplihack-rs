//! Core types for the fleet_local module.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

// ── Error enum (10 variants — SEC-11: Display shows category only) ────────────

/// Typed errors for the local fleet dashboard.
///
/// `Display` shows category-level messages only — never raw paths, PIDs, or
/// internal state.  Reserve `Debug` for log files.
#[derive(Debug, thiserror::Error)]
pub enum FleetLocalError {
    /// File-system I/O error.
    #[error("IO error reading session data")]
    Io(#[from] std::io::Error),

    /// Lock file name produced an empty string after sanitization.
    #[error("Invalid session identifier")]
    InvalidSession,

    /// PID in lock file is outside the range `1..=4_194_304`.
    #[error("PID out of valid range")]
    PidOutOfRange,

    /// Attempted to adopt a session owned by a different UID.
    #[error("Permission denied: session belongs to another user")]
    PermissionDenied(String),

    /// JSON parse / serialize failure.
    #[error("JSON serialization error")]
    Json(#[from] serde_json::Error),

    /// Input that was expected to be valid UTF-8 was not.
    #[error("Invalid UTF-8 input")]
    InvalidUtf8,

    /// Background refresh thread failed to collect state.
    #[error("Refresh failed: {0}")]
    RefreshFailed(String),

    /// A session referenced by the caller was not found.
    #[error("Session not found")]
    SessionNotFound,

    /// PID reuse detected: `/proc/{pid}/comm` did not match expected process.
    #[error("PID reuse detected; adoption aborted")]
    PidReuse,

    /// Editor hard limit exceeded (lines or bytes per line).
    #[error("Editor limit exceeded")]
    EditorLimitExceeded,
}

// ── SessionStatus ─────────────────────────────────────────────────────────────

/// Observed status of a local Claude session.
///
/// Determined by PID validity and `/proc/{pid}/comm` (Linux) or sysctl
/// (macOS).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SessionStatus {
    /// Process is running and its comm matches the expected Claude binary.
    Active,
    /// Process exists but has been quiet for a while.
    Idle,
    /// Process no longer exists (PID not in /proc or sysctl).
    Dead,
    /// Cannot determine status (e.g., permission error checking /proc).
    #[default]
    Unknown,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStatus::Active => write!(f, "Active"),
            SessionStatus::Idle => write!(f, "Idle"),
            SessionStatus::Dead => write!(f, "Dead"),
            SessionStatus::Unknown => write!(f, "Unknown"),
        }
    }
}

// ── FleetSessionEntry ─────────────────────────────────────────────────────────

/// A single row in the local session dashboard.
///
/// Produced by `collect_observed_fleet_state()` from one lock file.
#[derive(Debug, Clone)]
pub struct FleetSessionEntry {
    /// Sanitized session identifier (lock filename, stripped of path chars).
    pub session_id: String,
    /// PID read from the lock file (validated: `1..=4_194_304`).
    pub pid: u32,
    /// Observed process status.
    pub status: SessionStatus,
    /// Project directory, if discoverable from the lock file or /proc.
    pub project_path: Option<PathBuf>,
}

// ── Background refresh messages ───────────────────────────────────────────────

/// Messages sent from the fast refresh thread (T4, 500 ms) to the main loop.
#[derive(Debug)]
pub enum RefreshMsg {
    /// Updated session list.
    Sessions(Vec<FleetSessionEntry>),
    /// Error collecting state; dashboard shows stale data.
    Error(String),
    /// Tmux capture output for one local session (from T5 slow refresh thread).
    CaptureUpdate {
        /// Sanitized session identifier.
        session_id: String,
        /// OSC-stripped terminal output (≤ `CAPTURE_CACHE_ENTRY_MAX_BYTES`).
        output: String,
    },
}

/// Messages sent from the slow refresh thread (T5, 5 s) to the main loop.
#[derive(Debug)]
pub enum SlowRefreshMsg {
    /// Updated tmux capture output for one session.
    CaptureUpdate {
        /// Sanitized session identifier.
        session_id: String,
        /// OSC-stripped terminal output (≤ 64 KiB).
        output: String,
    },
}

// Suppress unused import warning — Sender is used by dependents via
// FleetLocalError and RefreshMsg's containing module.
const _: () = {
    fn _assert_sender_used(_: Sender<RefreshMsg>) {}
};

#[cfg(test)]
mod tests {
    use super::*;

    // ── SessionStatus ──────────────────────────────────────────────────────

    #[test]
    fn session_status_default_is_unknown() {
        assert_eq!(SessionStatus::default(), SessionStatus::Unknown);
    }

    #[test]
    fn session_status_display_active() {
        assert_eq!(SessionStatus::Active.to_string(), "Active");
    }

    #[test]
    fn session_status_display_idle() {
        assert_eq!(SessionStatus::Idle.to_string(), "Idle");
    }

    #[test]
    fn session_status_display_dead() {
        assert_eq!(SessionStatus::Dead.to_string(), "Dead");
    }

    #[test]
    fn session_status_display_unknown() {
        assert_eq!(SessionStatus::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn session_status_all_variants_are_distinct() {
        let statuses = [
            SessionStatus::Active,
            SessionStatus::Idle,
            SessionStatus::Dead,
            SessionStatus::Unknown,
        ];
        let displays: Vec<_> = statuses.iter().map(|s| s.to_string()).collect();
        let unique: std::collections::HashSet<_> = displays.iter().collect();
        assert_eq!(unique.len(), statuses.len(), "duplicate display strings");
    }

    // ── FleetLocalError display ────────────────────────────────────────────

    #[test]
    fn fleet_local_error_display_does_not_expose_raw_paths() {
        let errors = [
            FleetLocalError::InvalidSession,
            FleetLocalError::PidOutOfRange,
            FleetLocalError::InvalidUtf8,
            FleetLocalError::SessionNotFound,
            FleetLocalError::PidReuse,
            FleetLocalError::EditorLimitExceeded,
        ];
        for err in &errors {
            let msg = err.to_string();
            assert!(!msg.is_empty(), "error Display must not be empty: {err:?}");
            assert!(
                !msg.contains("//") && !msg.starts_with('/'),
                "error Display must not expose raw paths: {msg:?}"
            );
        }
    }

    // ── PID_MAX constant ───────────────────────────────────────────────────

    #[test]
    fn pid_max_constant_matches_spec_value() {
        assert_eq!(super::super::PID_MAX, 4_194_304);
    }

    // ── sanitize_session_id integration ───────────────────────────────────

    #[test]
    fn sanitize_session_id_strips_path_separators_before_use() {
        use amplihack_types::paths::sanitize_session_id;
        let safe = sanitize_session_id("normal-session-id-123");
        assert_eq!(safe, "normal-session-id-123");
    }

    #[test]
    fn sanitize_session_id_removes_traversal_sequences() {
        use amplihack_types::paths::sanitize_session_id;
        let safe = sanitize_session_id("../../../etc/passwd");
        assert_eq!(safe, "_________etc_passwd");
    }

    // ── RefreshMsg ─────────────────────────────────────────────────────────

    #[test]
    fn refresh_msg_capture_update_variant_exists() {
        let msg = RefreshMsg::CaptureUpdate {
            session_id: "session-01".to_string(),
            output: "some output".to_string(),
        };
        let RefreshMsg::CaptureUpdate { session_id, output } = msg else {
            unreachable!("Expected CaptureUpdate, got {msg:?}");
        };
        assert_eq!(session_id, "session-01");
        assert_eq!(output, "some output");
    }
}
