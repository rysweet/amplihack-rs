//! Error hierarchy for the bundle generator pipeline.

use thiserror::Error;

/// Errors from bundle generator operations.
#[derive(Debug, Error)]
pub enum BundleGeneratorError {
    /// Prompt parsing failed.
    #[error("[PARSING_FAILED] {message}")]
    Parsing {
        /// Human-readable description.
        message: String,
        /// Fragment of the prompt that caused the issue.
        prompt_fragment: Option<String>,
        /// Character position.
        position: Option<usize>,
    },

    /// Intent extraction failed.
    #[error("[EXTRACTION_FAILED] {message}")]
    Extraction {
        /// Human-readable description.
        message: String,
        /// Terms that could not be interpreted.
        ambiguous_terms: Vec<String>,
        /// Extraction confidence (0.0–1.0).
        confidence: Option<f64>,
    },

    /// Agent content generation failed.
    #[error("[GENERATION_FAILED] {message}")]
    Generation {
        /// Human-readable description.
        message: String,
        /// Name of the agent being generated.
        agent_name: Option<String>,
        /// Pipeline stage that failed.
        stage: Option<String>,
    },

    /// Bundle validation failed.
    #[error("[VALIDATION_FAILED] {message}")]
    Validation {
        /// Human-readable description.
        message: String,
        /// Validation category.
        validation_type: String,
        /// Individual failures.
        failures: Vec<String>,
    },

    /// Bundle packaging failed.
    #[error("[PACKAGING_FAILED] {message}")]
    Packaging {
        /// Human-readable description.
        message: String,
        /// Target format.
        format: Option<String>,
        /// File path involved.
        path: Option<String>,
    },

    /// Distribution failed.
    #[error("[DISTRIBUTION_FAILED] {message}")]
    Distribution {
        /// Human-readable description.
        message: String,
        /// Target platform.
        platform: Option<String>,
        /// HTTP status code, if applicable.
        http_status: Option<u16>,
    },

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl BundleGeneratorError {
    /// Suggested recovery action for this error.
    pub fn recovery_suggestion(&self) -> &str {
        match self {
            Self::Parsing { .. } => {
                "Check prompt syntax and structure. Ensure clear agent descriptions."
            }
            Self::Extraction { .. } => {
                "Provide clearer agent requirements. Use specific action verbs and clear role definitions."
            }
            Self::Generation { .. } => {
                "Try simplifying agent requirements or generating agents individually."
            }
            Self::Validation { .. } => {
                "Review validation failures and correct the identified issues."
            }
            Self::Packaging { .. } => "Check file permissions and available disk space.",
            Self::Distribution { .. } => {
                "Check network connectivity and authentication. Verify repository permissions."
            }
            Self::Io(_) | Self::Json(_) => "Check file system state and retry.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_recovery_suggestions() {
        let err = BundleGeneratorError::Parsing {
            message: "bad".into(),
            prompt_fragment: None,
            position: None,
        };
        assert!(!err.recovery_suggestion().is_empty());
    }
}
