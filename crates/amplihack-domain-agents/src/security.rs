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

    pub fn audit(&self, _code: &str) -> Result<AuditReport> {
        todo!()
    }

    pub fn scan_vulnerabilities(&self, _code: &str) -> Result<Vec<Vulnerability>> {
        todo!()
    }

    pub fn risk_assessment(&self, _code: &str) -> Result<RiskAssessment> {
        todo!()
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
