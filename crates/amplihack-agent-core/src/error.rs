//! Agent-core error types.
//!
//! Matches Python `amplihack/agents/goal_seeking/` error handling.

use crate::models::AgentState;

// ---------------------------------------------------------------------------
// AgentError
// ---------------------------------------------------------------------------

/// Errors that can occur during agent operations.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// The requested session was not found.
    #[error("session not found: {0}")]
    SessionNotFound(String),

    /// An invalid state transition was attempted.
    #[error("invalid state transition from {from} to {to}")]
    InvalidState {
        /// The state the agent was in.
        from: AgentState,
        /// The state the caller tried to move to.
        to: AgentState,
    },

    /// A task execution failed.
    #[error("task failed: {0}")]
    TaskFailed(String),

    /// The task queue is full.
    #[error("queue full: {0}")]
    QueueFull(String),

    /// A memory operation failed.
    #[error("memory error: {0}")]
    MemoryError(String),

    /// A configuration error.
    #[error("config error: {0}")]
    ConfigError(String),

    /// An operation timed out.
    #[error("timeout after {0} seconds")]
    TimeoutError(u64),

    /// An I/O error propagated from the OS.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A JSON serialization/deserialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, AgentError>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_not_found_display() {
        let err = AgentError::SessionNotFound("abc-123".into());
        assert_eq!(err.to_string(), "session not found: abc-123");
    }

    #[test]
    fn invalid_state_display() {
        let err = AgentError::InvalidState {
            from: AgentState::Idle,
            to: AgentState::Acting,
        };
        assert!(err.to_string().contains("idle"));
        assert!(err.to_string().contains("acting"));
    }

    #[test]
    fn task_failed_display() {
        let err = AgentError::TaskFailed("exit code 1".into());
        assert_eq!(err.to_string(), "task failed: exit code 1");
    }

    #[test]
    fn queue_full_display() {
        let err = AgentError::QueueFull("queue is at capacity (10)".into());
        assert_eq!(err.to_string(), "queue full: queue is at capacity (10)");
    }

    #[test]
    fn memory_error_display() {
        let err = AgentError::MemoryError("store full".into());
        assert_eq!(err.to_string(), "memory error: store full");
    }

    #[test]
    fn config_error_display() {
        let err = AgentError::ConfigError("missing model".into());
        assert_eq!(err.to_string(), "config error: missing model");
    }

    #[test]
    fn timeout_error_display() {
        let err = AgentError::TimeoutError(30);
        assert_eq!(err.to_string(), "timeout after 30 seconds");
    }

    #[test]
    fn io_error_converts() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err: AgentError = io.into();
        assert!(matches!(err, AgentError::Io(_)));
    }

    #[test]
    fn json_error_converts() {
        let bad: std::result::Result<serde_json::Value, _> = serde_json::from_str("{bad");
        let err: AgentError = bad.unwrap_err().into();
        assert!(matches!(err, AgentError::Json(_)));
    }
}
