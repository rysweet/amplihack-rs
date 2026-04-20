//! Error types for the eval framework.

/// Errors that can occur during evaluation.
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    /// A benchmark case name was empty or otherwise invalid.
    #[error("invalid benchmark: {reason}")]
    InvalidBenchmark { reason: String },

    /// Score is outside the valid [0.0, 1.0] range.
    #[error("invalid score {value}: must be in [0.0, 1.0]")]
    InvalidScore { value: f64 },

    /// JSON serialisation/deserialisation failed.
    #[error("serialisation error: {0}")]
    Serialisation(#[from] serde_json::Error),

    /// File I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl EvalError {
    pub fn invalid_benchmark(reason: impl Into<String>) -> Self {
        Self::InvalidBenchmark {
            reason: reason.into(),
        }
    }

    pub fn invalid_score(value: f64) -> Self {
        Self::InvalidScore { value }
    }
}
