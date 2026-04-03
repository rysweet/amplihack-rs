use chrono::Utc;

use crate::error::Result;
use crate::models::{AuditReport, RiskAssessment, SecurityConfig, Vulnerability};

pub struct SecurityAuditor {
    config: SecurityConfig,
}

impl SecurityAuditor {
    pub fn new(config: SecurityConfig) -> Self {
        Self { config }
    }

    pub fn with_defaults() -> Self {
        Self::new(SecurityConfig::default())
    }

    pub fn config(&self) -> &SecurityConfig {
        &self.config
    }

    pub fn audit(&self, code: &str) -> Result<AuditReport> {
        let vulnerabilities = self.find_vulnerabilities(code);
        let risk_score = match vulnerabilities.len() {
            0 => 0.0,
            1..=3 => 0.5,
            _ => 1.0,
        };
        let summary = if vulnerabilities.is_empty() {
            "No vulnerabilities detected".into()
        } else {
            format!("Found {} potential vulnerabilities", vulnerabilities.len())
        };
        Ok(AuditReport {
            vulnerabilities,
            risk_score,
            summary,
            scanned_at: Utc::now(),
        })
    }

    pub fn scan_vulnerabilities(&self, code: &str) -> Result<Vec<Vulnerability>> {
        Ok(self.find_vulnerabilities(code))
    }

    pub fn risk_assessment(&self, code: &str) -> Result<RiskAssessment> {
        let vulns = self.find_vulnerabilities(code);
        let count = vulns.len();
        let (level, score) = match count {
            0 => ("low", 0.0),
            1..=3 => ("medium", 0.5),
            _ => ("high", 1.0),
        };
        let factors: Vec<String> = vulns.iter().map(|v| v.description.clone()).collect();
        let recommendations: Vec<String> = vulns.iter().map(|v| v.recommendation.clone()).collect();
        Ok(RiskAssessment {
            overall_risk: level.into(),
            risk_score: score,
            factors,
            recommendations,
        })
    }

    fn find_vulnerabilities(&self, code: &str) -> Vec<Vulnerability> {
        let mut vulns = Vec::new();
        let lower = code.to_lowercase();
        let mut id_counter = 1u32;

        // SQL injection patterns
        let has_sql = lower.contains("select") || lower.contains("insert") || lower.contains("delete");
        let has_concat = code.contains("format!") || code.contains('+') || code.contains("concat");
        if has_sql && has_concat {
            vulns.push(Vulnerability {
                id: format!("VULN-{id_counter:03}"),
                severity: "high".into(),
                description: "Potential SQL injection via string concatenation".into(),
                location: "detected in source".into(),
                recommendation: "Use parameterized queries instead of string concatenation".into(),
            });
            id_counter += 1;
        }

        // XSS patterns
        if lower.contains("innerhtml") || lower.contains("document.write") || lower.contains("<script") {
            vulns.push(Vulnerability {
                id: format!("VULN-{id_counter:03}"),
                severity: "high".into(),
                description: "Potential XSS vulnerability".into(),
                location: "detected in source".into(),
                recommendation: "Sanitize user input before inserting into DOM".into(),
            });
            id_counter += 1;
        }

        // Hardcoded secrets
        for pattern in &["password =", "secret =", "api_key =", "token ="] {
            if lower.contains(pattern) {
                vulns.push(Vulnerability {
                    id: format!("VULN-{id_counter:03}"),
                    severity: "medium".into(),
                    description: format!("Hardcoded secret detected: {pattern}"),
                    location: "detected in source".into(),
                    recommendation: "Use environment variables or a secrets manager".into(),
                });
                id_counter += 1;
            }
        }

        // Unsafe code patterns
        if lower.contains("unsafe") || lower.contains("eval(") || lower.contains("exec(") {
            vulns.push(Vulnerability {
                id: format!("VULN-{id_counter:03}"),
                severity: "high".into(),
                description: "Use of unsafe or dynamic execution pattern".into(),
                location: "detected in source".into(),
                recommendation: "Avoid unsafe blocks and dynamic code execution where possible".into(),
            });
        }

        vulns
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_config() {
        let config = SecurityConfig {
            scan_depth: 5,
            severity_threshold: "high".into(),
            include_info: true,
        };
        let auditor = SecurityAuditor::new(config.clone());
        assert_eq!(auditor.config(), &config);
    }

    #[test]
    fn with_defaults() {
        let auditor = SecurityAuditor::with_defaults();
        assert_eq!(auditor.config().scan_depth, 3);
        assert!(!auditor.config().include_info);
    }
}
