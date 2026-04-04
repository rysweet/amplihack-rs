use thiserror::Error;

/// Errors produced by the multilspy LSP client.
#[derive(Debug, Error)]
pub enum MultilspyError {
    #[error("language server process failed: {0}")]
    ProcessError(String),

    #[error("JSON-RPC error (code {code}): {message}")]
    JsonRpcError { code: i64, message: String },

    #[error("unsupported language: {0}")]
    UnsupportedLanguage(String),

    #[error("server not started")]
    ServerNotStarted,

    #[error("server already started")]
    ServerAlreadyStarted,

    #[error("request timed out")]
    Timeout,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("file not found: {0}")]
    FileNotFound(String),

    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_process() {
        let err = MultilspyError::ProcessError("crashed".into());
        assert_eq!(err.to_string(), "language server process failed: crashed");
    }

    #[test]
    fn error_display_jsonrpc() {
        let err = MultilspyError::JsonRpcError {
            code: -32600,
            message: "invalid request".into(),
        };
        assert_eq!(
            err.to_string(),
            "JSON-RPC error (code -32600): invalid request"
        );
    }

    #[test]
    fn error_display_unsupported() {
        let err = MultilspyError::UnsupportedLanguage("brainfuck".into());
        assert_eq!(err.to_string(), "unsupported language: brainfuck");
    }

    #[test]
    fn error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err: MultilspyError = io_err.into();
        assert!(matches!(err, MultilspyError::Io(_)));
    }

    #[test]
    fn error_from_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err: MultilspyError = json_err.into();
        assert!(matches!(err, MultilspyError::Json(_)));
    }
}
