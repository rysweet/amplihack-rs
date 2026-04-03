use thiserror::Error;

/// Errors produced by the goal-agent generation pipeline.
#[derive(Debug, Error)]
pub enum GeneratorError {
    #[error("invalid goal: {0}")]
    InvalidGoal(String),

    #[error("planning failed: {0}")]
    PlanningFailed(String),

    #[error("synthesis failed: {0}")]
    SynthesisFailed(String),

    #[error("assembly failed: {0}")]
    AssemblyFailed(String),

    #[error("packaging failed: {0}")]
    PackagingFailed(String),

    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, GeneratorError>;
