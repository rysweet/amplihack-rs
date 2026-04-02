use crate::state_machine::ProcessState;

/// Errors that can occur during delegation operations.
#[derive(Debug, thiserror::Error)]
pub enum DelegationError {
    /// The overall delegation exceeded its time budget.
    #[error("delegation timed out after {0} seconds")]
    Timeout(u64),

    /// The delegation failed for a described reason.
    #[error("delegation failed: {0}")]
    Failed(String),

    /// A subprocess encountered an error.
    #[error("subprocess error: {0}")]
    Subprocess(String),

    /// A subprocess exceeded its time budget.
    #[error("subprocess timed out after {0} seconds")]
    SubprocessTimeout(u64),

    /// An invalid state transition was attempted.
    #[error("invalid state transition from {from} to {to}")]
    InvalidTransition {
        /// The state the machine was in.
        from: ProcessState,
        /// The state the caller tried to move to.
        to: ProcessState,
    },

    /// An I/O error propagated from the OS.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A JSON serialization/deserialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// A generic validation error.
    #[error("validation error: {0}")]
    Validation(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, DelegationError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_error_display() {
        let err = DelegationError::Timeout(120);
        assert_eq!(err.to_string(), "delegation timed out after 120 seconds");
    }

    #[test]
    fn failed_error_display() {
        let err = DelegationError::Failed("no output".into());
        assert_eq!(err.to_string(), "delegation failed: no output");
    }

    #[test]
    fn subprocess_error_display() {
        let err = DelegationError::Subprocess("exit code 1".into());
        assert_eq!(err.to_string(), "subprocess error: exit code 1");
    }

    #[test]
    fn subprocess_timeout_display() {
        let err = DelegationError::SubprocessTimeout(60);
        assert_eq!(err.to_string(), "subprocess timed out after 60 seconds");
    }

    #[test]
    fn invalid_transition_display() {
        let err = DelegationError::InvalidTransition {
            from: ProcessState::Completed,
            to: ProcessState::Running,
        };
        assert!(err.to_string().contains("completed"));
        assert!(err.to_string().contains("running"));
    }

    #[test]
    fn io_error_converts() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err: DelegationError = io.into();
        assert!(matches!(err, DelegationError::Io(_)));
    }

    #[test]
    fn validation_error_display() {
        let err = DelegationError::Validation("empty goal".into());
        assert_eq!(err.to_string(), "validation error: empty goal");
    }
}
