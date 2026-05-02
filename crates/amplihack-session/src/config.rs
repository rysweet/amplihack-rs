//! Configuration, state, and error types for sessions.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Configuration for [`crate::ClaudeSession`] behavior.
///
/// Defaults mirror the Python `SessionConfig` dataclass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionConfig {
    pub timeout: Duration,
    pub max_retries: u32,
    pub retry_delay: Duration,
    pub heartbeat_interval: Duration,
    pub enable_logging: bool,
    pub log_level: String,
    pub session_id: Option<String>,
    pub auto_save_interval: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(300),
            max_retries: 3,
            retry_delay: Duration::from_secs(1),
            heartbeat_interval: Duration::from_secs(30),
            enable_logging: true,
            log_level: "INFO".to_string(),
            session_id: None,
            auto_save_interval: Duration::from_secs(60),
        }
    }
}

/// Activity / lifecycle state for an in-flight session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionState {
    pub session_id: String,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    pub is_active: bool,
    pub command_count: u64,
    pub error_count: u64,
    pub last_error: Option<String>,
    pub metadata: serde_json::Value,
}

impl SessionState {
    /// Construct a freshly-started state for `session_id`.
    pub fn new(session_id: impl Into<String>) -> Self {
        let now = chrono::Utc::now();
        Self {
            session_id: session_id.into(),
            start_time: now,
            last_activity: now,
            is_active: true,
            command_count: 0,
            error_count: 0,
            last_error: None,
            metadata: serde_json::Value::Object(Default::default()),
        }
    }
}

/// All errors surfaced by the `amplihack-session` crate.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session is not active")]
    NotActive,

    #[error("session timed out after {timeout:?}")]
    Timeout { timeout: Duration },

    #[error("session not found: {0}")]
    NotFound(String),

    #[error("no checkpoints available")]
    NoCheckpoints,

    #[error("checkpoint index out of range: {0}")]
    CheckpointOutOfRange(i64),

    #[error("invalid session id: {0}")]
    InvalidSessionId(String),

    #[error("file too large: {size} bytes (max {max} bytes) at {path}")]
    TooLarge { path: PathBuf, size: u64, max: u64 },

    #[error("file corruption detected: {0}")]
    Corruption(String),

    #[error("retry attempts exhausted: {0}")]
    RetryExhausted(String),

    #[error("path escapes base directory: {0}")]
    PathEscape(PathBuf),

    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("json error at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl SessionError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.into(),
            source,
        }
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, SessionError>;
