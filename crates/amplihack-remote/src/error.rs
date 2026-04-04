//! Error types for remote execution.
//!
//! Provides a hierarchy of errors covering each phase of the remote
//! execution pipeline: packaging, provisioning, transfer, execution,
//! integration, and cleanup.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Structured context attached to every [`RemoteError`] variant.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrorContext(pub HashMap<String, String>);

impl fmt::Display for ErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return Ok(());
        }
        let pairs: Vec<String> = self.0.iter().map(|(k, v)| format!("{k}={v}")).collect();
        write!(f, " (context: {})", pairs.join(", "))
    }
}

impl ErrorContext {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn insert(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.0.insert(key.into(), value.into());
        self
    }
}

/// All remote-execution errors.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    /// Error during context packaging (secret detection, archive creation).
    #[error("Packaging error: {message}{context}")]
    PackagingError {
        message: String,
        context: ErrorContext,
    },

    /// Error during VM provisioning via azlin.
    #[error("Provisioning error: {message}{context}")]
    ProvisioningError {
        message: String,
        context: ErrorContext,
    },

    /// Error during file transfer (SCP/azlin cp).
    #[error("Transfer error: {message}{context}")]
    TransferError {
        message: String,
        context: ErrorContext,
    },

    /// Error during remote command execution.
    #[error("Execution error: {message}{context}")]
    ExecutionError {
        message: String,
        context: ErrorContext,
    },

    /// Error during result integration (git fetch, merge).
    #[error("Integration error: {message}{context}")]
    IntegrationError {
        message: String,
        context: ErrorContext,
    },

    /// Error during VM cleanup (typically non-fatal).
    #[error("Cleanup error: {message}{context}")]
    CleanupError {
        message: String,
        context: ErrorContext,
    },
}

/// Convenience constructors for each variant.
impl RemoteError {
    pub fn packaging(msg: impl Into<String>) -> Self {
        Self::PackagingError {
            message: msg.into(),
            context: ErrorContext::new(),
        }
    }

    pub fn packaging_ctx(msg: impl Into<String>, ctx: ErrorContext) -> Self {
        Self::PackagingError {
            message: msg.into(),
            context: ctx,
        }
    }

    pub fn provisioning(msg: impl Into<String>) -> Self {
        Self::ProvisioningError {
            message: msg.into(),
            context: ErrorContext::new(),
        }
    }

    pub fn provisioning_ctx(msg: impl Into<String>, ctx: ErrorContext) -> Self {
        Self::ProvisioningError {
            message: msg.into(),
            context: ctx,
        }
    }

    pub fn transfer(msg: impl Into<String>) -> Self {
        Self::TransferError {
            message: msg.into(),
            context: ErrorContext::new(),
        }
    }

    pub fn transfer_ctx(msg: impl Into<String>, ctx: ErrorContext) -> Self {
        Self::TransferError {
            message: msg.into(),
            context: ctx,
        }
    }

    pub fn execution(msg: impl Into<String>) -> Self {
        Self::ExecutionError {
            message: msg.into(),
            context: ErrorContext::new(),
        }
    }

    pub fn execution_ctx(msg: impl Into<String>, ctx: ErrorContext) -> Self {
        Self::ExecutionError {
            message: msg.into(),
            context: ctx,
        }
    }

    pub fn integration(msg: impl Into<String>) -> Self {
        Self::IntegrationError {
            message: msg.into(),
            context: ErrorContext::new(),
        }
    }

    pub fn integration_ctx(msg: impl Into<String>, ctx: ErrorContext) -> Self {
        Self::IntegrationError {
            message: msg.into(),
            context: ctx,
        }
    }

    pub fn cleanup(msg: impl Into<String>) -> Self {
        Self::CleanupError {
            message: msg.into(),
            context: ErrorContext::new(),
        }
    }

    pub fn cleanup_ctx(msg: impl Into<String>, ctx: ErrorContext) -> Self {
        Self::CleanupError {
            message: msg.into(),
            context: ctx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_without_context() {
        let err = RemoteError::packaging("secrets detected");
        assert_eq!(err.to_string(), "Packaging error: secrets detected");
    }

    #[test]
    fn error_display_with_context() {
        let ctx = ErrorContext::new().insert("repo_path", "/tmp/repo");
        let err = RemoteError::packaging_ctx("bundle failed", ctx);
        let display = err.to_string();
        assert!(display.contains("bundle failed"));
        assert!(display.contains("repo_path=/tmp/repo"));
    }

    #[test]
    fn error_context_is_empty_by_default() {
        let ctx = ErrorContext::new();
        assert!(ctx.0.is_empty());
        assert_eq!(ctx.to_string(), "");
    }

    #[test]
    fn all_variants_are_constructible() {
        let _p = RemoteError::packaging("p");
        let _pr = RemoteError::provisioning("pr");
        let _t = RemoteError::transfer("t");
        let _e = RemoteError::execution("e");
        let _i = RemoteError::integration("i");
        let _c = RemoteError::cleanup("c");
    }
}
