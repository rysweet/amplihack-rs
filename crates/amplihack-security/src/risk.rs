//! Risk levels, threat types, and validation result types.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Security enforcement level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecurityLevel {
    Low,
    Medium,
    High,
    Strict,
}

impl fmt::Display for SecurityLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Strict => write!(f, "strict"),
        }
    }
}

/// Risk level assigned to a piece of content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    None,
    Low,
    Medium,
    High,
    Critical,
}

impl fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

/// Category of content being validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Prompt,
    Url,
    BashCommand,
    ToolParameters,
    Data,
}

/// Type of threat detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreatType {
    PromptInjection,
    DataExfiltration,
    SystemEscape,
    RoleHijacking,
    EncodingBypass,
    ContextManipulation,
    MaliciousUrl,
    ChainAttack,
}

/// A single detected threat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreatDetection {
    pub threat_type: ThreatType,
    pub severity: RiskLevel,
    pub description: String,
    pub pattern_id: String,
    pub mitigation: String,
}

/// Result of content validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub risk_level: RiskLevel,
    pub should_block: bool,
    pub threats: Vec<ThreatDetection>,
    pub recommendations: Vec<String>,
    pub content_type: ContentType,
    pub metadata: serde_json::Value,
}

impl ValidationResult {
    /// Create a clean (no-threat) result for the given content type.
    pub fn clean(content_type: ContentType) -> Self {
        Self {
            risk_level: RiskLevel::None,
            should_block: false,
            threats: Vec::new(),
            recommendations: Vec::new(),
            content_type,
            metadata: serde_json::json!({"validation": "passed"}),
        }
    }

    /// Summary string of detected threats.
    pub fn threat_summary(&self) -> String {
        if self.threats.is_empty() {
            return "No threats detected".to_string();
        }
        let count = self.threats.len();
        let highest = self
            .threats
            .iter()
            .map(|t| t.severity)
            .max()
            .unwrap_or(RiskLevel::None);
        format!("{count} threat(s) detected, highest severity: {highest}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_result_has_no_threats() {
        let r = ValidationResult::clean(ContentType::Prompt);
        assert_eq!(r.risk_level, RiskLevel::None);
        assert!(!r.should_block);
        assert!(r.threats.is_empty());
    }

    #[test]
    fn risk_level_ordering() {
        assert!(RiskLevel::None < RiskLevel::Low);
        assert!(RiskLevel::Low < RiskLevel::Medium);
        assert!(RiskLevel::Medium < RiskLevel::High);
        assert!(RiskLevel::High < RiskLevel::Critical);
    }

    #[test]
    fn security_level_ordering() {
        assert!(SecurityLevel::Low < SecurityLevel::Medium);
        assert!(SecurityLevel::Medium < SecurityLevel::High);
        assert!(SecurityLevel::High < SecurityLevel::Strict);
    }

    #[test]
    fn threat_summary_empty() {
        let r = ValidationResult::clean(ContentType::Data);
        assert_eq!(r.threat_summary(), "No threats detected");
    }

    #[test]
    fn threat_summary_with_threats() {
        let mut r = ValidationResult::clean(ContentType::Prompt);
        r.threats.push(ThreatDetection {
            threat_type: ThreatType::PromptInjection,
            severity: RiskLevel::Critical,
            description: "test".into(),
            pattern_id: "PO001".into(),
            mitigation: "block".into(),
        });
        assert!(r.threat_summary().contains("1 threat(s)"));
        assert!(r.threat_summary().contains("critical"));
    }
}
