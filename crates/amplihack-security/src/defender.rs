//! Core XPIA validation logic.
//!
//! Validates content, URLs, and bash commands against attack patterns.

use crate::config::XpiaConfig;
use crate::patterns::{PatternCategory, XpiaPatterns};
use crate::risk::{ContentType, RiskLevel, ThreatDetection, ThreatType, ValidationResult};
use tracing::{info, warn};

/// Core XPIA defender that validates content against attack patterns.
pub struct XpiaDefender {
    config: XpiaConfig,
    patterns: XpiaPatterns,
}

impl XpiaDefender {
    pub fn new(config: XpiaConfig) -> Self {
        info!(security_level = %config.security_level, "XPIA Defender initialized");
        Self {
            config,
            patterns: XpiaPatterns::new(),
        }
    }

    /// Create a defender with default (env-driven) configuration.
    pub fn from_env() -> Self {
        Self::new(XpiaConfig::from_env())
    }

    /// Whether the defender is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Validate arbitrary content for prompt injection.
    pub fn validate_content(&self, content: &str, content_type: ContentType) -> ValidationResult {
        if !self.config.enabled {
            return ValidationResult::clean(content_type);
        }

        let hits = self.patterns.scan(content);
        if hits.is_empty() {
            return ValidationResult::clean(content_type);
        }

        let threats: Vec<ThreatDetection> = hits
            .iter()
            .map(|p| ThreatDetection {
                threat_type: category_to_threat(p.category),
                severity: severity_str_to_risk(p.severity),
                description: p.description.to_string(),
                pattern_id: p.id.to_string(),
                mitigation: p.mitigation.to_string(),
            })
            .collect();

        let max_risk = threats
            .iter()
            .map(|t| t.severity)
            .max()
            .unwrap_or(RiskLevel::None);

        let should_block = self.config.should_block(max_risk);

        if should_block {
            warn!(
                risk = %max_risk,
                threats = threats.len(),
                "XPIA: content blocked"
            );
        }

        let recommendations = build_recommendations(&threats);

        ValidationResult {
            risk_level: max_risk,
            should_block,
            threats,
            recommendations,
            content_type,
            metadata: serde_json::json!({"validation": "completed"}),
        }
    }

    /// Validate a URL for security risks.
    pub fn validate_url(&self, url: &str) -> ValidationResult {
        if !self.config.enabled || !self.config.validate_webfetch {
            return ValidationResult::clean(ContentType::Url);
        }

        if url.len() > self.config.max_url_length {
            return ValidationResult {
                risk_level: RiskLevel::High,
                should_block: true,
                threats: vec![ThreatDetection {
                    threat_type: ThreatType::MaliciousUrl,
                    severity: RiskLevel::High,
                    description: format!(
                        "URL exceeds max length ({} > {})",
                        url.len(),
                        self.config.max_url_length
                    ),
                    pattern_id: "URL_LENGTH".into(),
                    mitigation: "Shorten URL".into(),
                }],
                recommendations: vec!["Reduce URL length".into()],
                content_type: ContentType::Url,
                metadata: serde_json::json!({"url_length": url.len()}),
            };
        }

        // Extract domain for whitelist/blacklist check
        if let Some(domain) = extract_domain(url)
            && self.config.is_blacklisted(&domain)
        {
            return ValidationResult {
                risk_level: RiskLevel::Critical,
                should_block: true,
                threats: vec![ThreatDetection {
                    threat_type: ThreatType::MaliciousUrl,
                    severity: RiskLevel::Critical,
                    description: format!("Domain '{domain}' is blacklisted"),
                    pattern_id: "BLACKLIST".into(),
                    mitigation: "Do not access blacklisted domains".into(),
                }],
                recommendations: vec!["Use a trusted domain".into()],
                content_type: ContentType::Url,
                metadata: serde_json::json!({"domain": domain}),
            };
        }

        // Run standard content validation on the URL string
        self.validate_content(url, ContentType::Url)
    }

    /// Validate a bash command for injection risks.
    pub fn validate_bash(&self, command: &str) -> ValidationResult {
        if !self.config.enabled || !self.config.validate_bash {
            return ValidationResult::clean(ContentType::BashCommand);
        }

        self.validate_content(command, ContentType::BashCommand)
    }

    /// Validate a WebFetch request (URL + prompt).
    pub fn validate_webfetch(&self, url: &str, prompt: &str) -> ValidationResult {
        if !self.config.enabled || !self.config.validate_webfetch {
            return ValidationResult::clean(ContentType::Url);
        }

        // Validate URL first
        let url_result = self.validate_url(url);
        if url_result.should_block {
            return url_result;
        }

        // Validate prompt content
        let prompt_result = self.validate_content(prompt, ContentType::Prompt);
        if prompt_result.should_block {
            return prompt_result;
        }

        // Merge results — take the higher risk
        if prompt_result.risk_level > url_result.risk_level {
            prompt_result
        } else {
            url_result
        }
    }

    /// Access the underlying configuration.
    pub fn config(&self) -> &XpiaConfig {
        &self.config
    }
}

fn extract_domain(url: &str) -> Option<String> {
    // Simple domain extraction without pulling in the `url` crate
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = without_scheme.split('/').next()?;
    let domain = host.split(':').next()?;
    if domain.is_empty() {
        None
    } else {
        Some(domain.to_lowercase())
    }
}

fn category_to_threat(cat: PatternCategory) -> ThreatType {
    match cat {
        PatternCategory::PromptOverride => ThreatType::PromptInjection,
        PatternCategory::InstructionInjection => ThreatType::PromptInjection,
        PatternCategory::ContextManipulation => ThreatType::ContextManipulation,
        PatternCategory::DataExfiltration => ThreatType::DataExfiltration,
        PatternCategory::SystemEscape => ThreatType::SystemEscape,
        PatternCategory::RoleHijacking => ThreatType::RoleHijacking,
        PatternCategory::EncodingBypass => ThreatType::EncodingBypass,
        PatternCategory::ChainAttacks => ThreatType::ChainAttack,
    }
}

fn severity_str_to_risk(s: &str) -> RiskLevel {
    match s {
        "critical" => RiskLevel::Critical,
        "high" => RiskLevel::High,
        "medium" => RiskLevel::Medium,
        "low" => RiskLevel::Low,
        _ => RiskLevel::Medium,
    }
}

fn build_recommendations(threats: &[ThreatDetection]) -> Vec<String> {
    let mut recs = Vec::new();
    for t in threats.iter().take(3) {
        if !t.mitigation.is_empty() {
            recs.push(t.mitigation.clone());
        }
    }
    if recs.is_empty() {
        recs.push("Review content for potential security issues".into());
    }
    recs
}

#[cfg(test)]
#[path = "tests/defender_tests.rs"]
mod tests;
