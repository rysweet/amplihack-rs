use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("teaching error: {0}")]
    Teaching(String),
    #[error("security error: {0}")]
    Security(String),
    #[error("code synthesis error: {0}")]
    CodeSynthesis(String),
    #[error("learning error: {0}")]
    Learning(String),
    #[error("routing error: {0}")]
    Routing(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("agent not ready: {0}")]
    NotReady(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, DomainError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teaching_error_display() {
        let err = DomainError::Teaching("concept unclear".into());
        assert_eq!(err.to_string(), "teaching error: concept unclear");
    }

    #[test]
    fn security_error_display() {
        let err = DomainError::Security("access denied".into());
        assert_eq!(err.to_string(), "security error: access denied");
    }

    #[test]
    fn code_synthesis_error_display() {
        let err = DomainError::CodeSynthesis("parse failure".into());
        assert_eq!(err.to_string(), "code synthesis error: parse failure");
    }

    #[test]
    fn learning_error_display() {
        let err = DomainError::Learning("memory full".into());
        assert_eq!(err.to_string(), "learning error: memory full");
    }

    #[test]
    fn routing_error_display() {
        let err = DomainError::Routing("no match".into());
        assert_eq!(err.to_string(), "routing error: no match");
    }

    #[test]
    fn invalid_input_error_display() {
        let err = DomainError::InvalidInput("empty string".into());
        assert_eq!(err.to_string(), "invalid input: empty string");
    }

    #[test]
    fn not_ready_error_display() {
        let err = DomainError::NotReady("initializing".into());
        assert_eq!(err.to_string(), "agent not ready: initializing");
    }

    #[test]
    fn io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err: DomainError = io_err.into();
        assert!(err.to_string().contains("file missing"));
    }

    #[test]
    fn json_error_conversion() {
        let json_err = serde_json::from_str::<String>("not json").unwrap_err();
        let err: DomainError = json_err.into();
        assert!(!err.to_string().is_empty());
    }
}
