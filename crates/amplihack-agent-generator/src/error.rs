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
    fn display_invalid_goal() {
        let e = GeneratorError::InvalidGoal("bad goal".into());
        assert_eq!(e.to_string(), "invalid goal: bad goal");
    }

    #[test]
    fn display_planning_failed() {
        let e = GeneratorError::PlanningFailed("no phases".into());
        assert_eq!(e.to_string(), "planning failed: no phases");
    }

    #[test]
    fn display_synthesis_failed() {
        let e = GeneratorError::SynthesisFailed("missing skill".into());
        assert_eq!(e.to_string(), "synthesis failed: missing skill");
    }

    #[test]
    fn display_assembly_failed() {
        let e = GeneratorError::AssemblyFailed("bad name".into());
        assert_eq!(e.to_string(), "assembly failed: bad name");
    }

    #[test]
    fn display_packaging_failed() {
        let e = GeneratorError::PackagingFailed("io err".into());
        assert_eq!(e.to_string(), "packaging failed: io err");
    }

    #[test]
    fn from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let e = GeneratorError::from(io_err);
        assert!(matches!(e, GeneratorError::IoError(_)));
        assert!(e.to_string().contains("gone"));
    }
}
