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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_messages() {
        let cases: Vec<(GeneratorError, &str)> = vec![
            (GeneratorError::InvalidGoal("bad".into()), "invalid goal: bad"),
            (GeneratorError::PlanningFailed("oops".into()), "planning failed: oops"),
            (GeneratorError::SynthesisFailed("fail".into()), "synthesis failed: fail"),
            (GeneratorError::AssemblyFailed("boom".into()), "assembly failed: boom"),
            (GeneratorError::PackagingFailed("err".into()), "packaging failed: err"),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn io_error_transparent() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err: GeneratorError = io.into();
        assert!(err.to_string().contains("gone"));
    }
}
