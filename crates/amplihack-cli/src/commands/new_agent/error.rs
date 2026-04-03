//! Typed error hierarchy for the bundle generator pipeline.
//!
//! Ported from `amplihack/bundle_generator/exceptions.py`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Structured error for bundle generator operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleError {
    pub kind: BundleErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub details: HashMap<String, String>,
}

/// Nine error variants mirroring the Python exception sub-classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleErrorKind {
    Parse,
    Extraction,
    Generation,
    Build,
    Packaging,
    Distribution,
    Validation,
    Config,
    Repo,
}

impl BundleErrorKind {
    /// Machine-readable error code (e.g. "PARSING_FAILED").
    pub fn code(self) -> &'static str {
        match self {
            Self::Parse => "PARSING_FAILED",
            Self::Extraction => "EXTRACTION_FAILED",
            Self::Generation => "GENERATION_FAILED",
            Self::Build => "BUILD_FAILED",
            Self::Packaging => "PACKAGING_FAILED",
            Self::Distribution => "DISTRIBUTION_FAILED",
            Self::Validation => "VALIDATION_FAILED",
            Self::Config => "CONFIG_ERROR",
            Self::Repo => "REPO_ERROR",
        }
    }

    /// Actionable recovery hint.
    pub fn recovery_hint(self) -> &'static str {
        match self {
            Self::Parse => "Check prompt syntax and structure. Ensure clear agent descriptions.",
            Self::Extraction => {
                "Provide clearer agent requirements. Use specific action verbs and clear role definitions."
            }
            Self::Generation => {
                "Try simplifying agent requirements or generating agents individually."
            }
            Self::Build => "Verify build toolchain is installed and all sources are present.",
            Self::Packaging => {
                "Check file permissions and available disk space. Ensure package format is supported."
            }
            Self::Distribution => {
                "Check network connectivity and authentication. Verify repository permissions."
            }
            Self::Validation => "Review validation failures and correct the identified issues.",
            Self::Config => "Verify configuration file exists and is valid JSON/YAML.",
            Self::Repo => "Check git configuration, remote URL, and authentication credentials.",
        }
    }
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.kind.code(), self.message)?;
        if !self.details.is_empty() {
            write!(f, " (")?;
            let mut first = true;
            for (k, v) in &self.details {
                if !first {
                    write!(f, ", ")?;
                }
                write!(f, "{k}={v}")?;
                first = false;
            }
            write!(f, ")")?;
        }
        Ok(())
    }
}

impl std::error::Error for BundleError {}

impl BundleError {
    pub fn new(kind: BundleErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            details: HashMap::new(),
        }
    }

    pub fn with_details(
        kind: BundleErrorKind,
        message: impl Into<String>,
        details: HashMap<String, String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            details,
        }
    }

    /// Builder-style detail adder.
    pub fn detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    // Shortcut constructors for each variant
    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(BundleErrorKind::Parse, message)
    }
    pub fn extraction(message: impl Into<String>) -> Self {
        Self::new(BundleErrorKind::Extraction, message)
    }
    pub fn generation(message: impl Into<String>) -> Self {
        Self::new(BundleErrorKind::Generation, message)
    }
    pub fn build(message: impl Into<String>) -> Self {
        Self::new(BundleErrorKind::Build, message)
    }
    pub fn packaging(message: impl Into<String>) -> Self {
        Self::new(BundleErrorKind::Packaging, message)
    }
    pub fn distribution(message: impl Into<String>) -> Self {
        Self::new(BundleErrorKind::Distribution, message)
    }
    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(BundleErrorKind::Validation, message)
    }
    pub fn config(message: impl Into<String>) -> Self {
        Self::new(BundleErrorKind::Config, message)
    }
    pub fn repo(message: impl Into<String>) -> Self {
        Self::new(BundleErrorKind::Repo, message)
    }

    /// Serialize to a flat map (mirrors Python `to_dict()`).
    pub fn to_dict(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("error_code".to_string(), self.kind.code().to_string());
        m.insert("message".to_string(), self.message.clone());
        m.insert(
            "recovery_suggestion".to_string(),
            self.kind.recovery_hint().to_string(),
        );
        for (k, v) in &self.details {
            m.insert(k.clone(), v.clone());
        }
        m
    }
}

// BundleError implements std::error::Error, so anyhow's blanket From<E: Error>
// impl provides automatic conversion — no manual From impl needed.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes_unique() {
        use std::collections::HashSet;
        let kinds = [
            BundleErrorKind::Parse,
            BundleErrorKind::Extraction,
            BundleErrorKind::Generation,
            BundleErrorKind::Build,
            BundleErrorKind::Packaging,
            BundleErrorKind::Distribution,
            BundleErrorKind::Validation,
            BundleErrorKind::Config,
            BundleErrorKind::Repo,
        ];
        let codes: HashSet<&str> = kinds.iter().map(|k| k.code()).collect();
        assert_eq!(codes.len(), kinds.len());
    }

    #[test]
    fn test_recovery_hints_non_empty() {
        for k in &[
            BundleErrorKind::Parse,
            BundleErrorKind::Extraction,
            BundleErrorKind::Generation,
            BundleErrorKind::Build,
            BundleErrorKind::Packaging,
            BundleErrorKind::Distribution,
            BundleErrorKind::Validation,
            BundleErrorKind::Config,
            BundleErrorKind::Repo,
        ] {
            assert!(!k.recovery_hint().is_empty());
        }
    }

    #[test]
    fn test_display_format() {
        let e = BundleError::parse("bad input");
        assert!(format!("{e}").contains("[PARSING_FAILED]"));
    }

    #[test]
    fn test_display_with_details() {
        let e = BundleError::generation("failed").detail("agent_name", "scanner");
        let s = format!("{e}");
        assert!(s.contains("agent_name=scanner"));
    }

    #[test]
    fn test_to_dict() {
        let e = BundleError::validation("missing section").detail("validation_type", "structure");
        let d = e.to_dict();
        assert_eq!(d["error_code"], "VALIDATION_FAILED");
        assert!(d.contains_key("recovery_suggestion"));
        assert_eq!(d["validation_type"], "structure");
    }

    #[test]
    fn test_serde_roundtrip() {
        let e = BundleError::config("missing key").detail("path", "/etc/cfg.yaml");
        let json = serde_json::to_string(&e).expect("serialize");
        let e2: BundleError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(e2.kind, BundleErrorKind::Config);
        assert_eq!(e2.details["path"], "/etc/cfg.yaml");
    }

    #[test]
    fn test_into_anyhow() {
        let e = BundleError::repo("clone failed");
        let anyhow_err = anyhow::Error::from(e);
        assert!(format!("{anyhow_err}").contains("REPO_ERROR"));
    }

    #[test]
    fn test_shortcut_constructors() {
        assert_eq!(BundleError::parse("x").kind, BundleErrorKind::Parse);
        assert_eq!(
            BundleError::extraction("x").kind,
            BundleErrorKind::Extraction
        );
        assert_eq!(
            BundleError::generation("x").kind,
            BundleErrorKind::Generation
        );
        assert_eq!(BundleError::build("x").kind, BundleErrorKind::Build);
        assert_eq!(BundleError::packaging("x").kind, BundleErrorKind::Packaging);
        assert_eq!(
            BundleError::distribution("x").kind,
            BundleErrorKind::Distribution
        );
        assert_eq!(
            BundleError::validation("x").kind,
            BundleErrorKind::Validation
        );
        assert_eq!(BundleError::config("x").kind, BundleErrorKind::Config);
        assert_eq!(BundleError::repo("x").kind, BundleErrorKind::Repo);
    }
}
