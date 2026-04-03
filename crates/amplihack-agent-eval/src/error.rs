//! Error types for the eval framework.

use std::path::PathBuf;

/// Errors that can occur during evaluation.
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    /// Grading operation failed.
    #[error("grading failed: {reason}")]
    GradingFailed { reason: String },

    /// Requested test level was not found.
    #[error("level not found: {level}")]
    LevelNotFound { level: String },

    /// Evaluation exceeded the configured timeout.
    #[error("timeout exceeded after {seconds}s")]
    TimeoutExceeded { seconds: u64 },

    /// Harness execution error.
    #[error("harness error: {message}")]
    HarnessError { message: String },

    /// Configuration is invalid.
    #[error("config error: {message}")]
    ConfigError { message: String },

    /// IO error with optional path context.
    #[error("io error at {path:?}: {source}")]
    IoError {
        path: Option<PathBuf>,
        #[source]
        source: std::io::Error,
    },
}

impl From<std::io::Error> for EvalError {
    fn from(source: std::io::Error) -> Self {
        Self::IoError { path: None, source }
    }
}

impl EvalError {
    pub fn grading(reason: impl Into<String>) -> Self {
        Self::GradingFailed {
            reason: reason.into(),
        }
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::ConfigError {
            message: message.into(),
        }
    }

    pub fn harness(message: impl Into<String>) -> Self {
        Self::HarnessError {
            message: message.into(),
        }
    }

    pub fn timeout(seconds: u64) -> Self {
        Self::TimeoutExceeded { seconds }
    }

    pub fn level_not_found(level: impl Into<String>) -> Self {
        Self::LevelNotFound {
            level: level.into(),
        }
    }
}
