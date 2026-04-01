//! XPIA configuration loaded from environment variables.

use crate::risk::{RiskLevel, SecurityLevel};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;

/// Default safe domains always included in the whitelist.
const DEFAULT_SAFE_DOMAINS: &[&str] = &[
    "github.com",
    "microsoft.com",
    "azure.com",
    "openai.com",
    "anthropic.com",
    "stackoverflow.com",
    "python.org",
    "nodejs.org",
    "npmjs.com",
    "pypi.org",
    "docs.python.org",
    "developer.mozilla.org",
    "w3.org",
];

/// Complete XPIA configuration with environment variable support.
#[derive(Debug, Clone)]
pub struct XpiaConfig {
    pub enabled: bool,
    pub security_level: SecurityLevel,
    pub verbose_feedback: bool,

    // Blocking thresholds
    pub block_on_high_risk: bool,
    pub block_on_critical: bool,

    // Feature flags
    pub validate_webfetch: bool,
    pub validate_bash: bool,
    pub validate_agents: bool,

    // Logging
    pub log_security_events: bool,
    pub log_file: Option<String>,

    // Domain lists
    pub whitelist_domains: HashSet<String>,
    pub blacklist_domains: HashSet<String>,

    // Limits
    pub max_prompt_length: usize,
    pub max_url_length: usize,
}

impl Default for XpiaConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl XpiaConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        let security_level = match env_str("XPIA_SECURITY_LEVEL", "MODERATE")
            .to_uppercase()
            .as_str()
        {
            "STRICT" => SecurityLevel::Strict,
            "HIGH" => SecurityLevel::High,
            "LENIENT" | "LOW" => SecurityLevel::Low,
            _ => SecurityLevel::Medium,
        };

        let mut config = Self {
            enabled: env_bool("XPIA_ENABLED", true),
            security_level,
            verbose_feedback: env_bool("XPIA_VERBOSE_FEEDBACK", false),
            block_on_high_risk: env_bool("XPIA_BLOCK_HIGH_RISK", true),
            block_on_critical: env_bool("XPIA_BLOCK_CRITICAL", true),
            validate_webfetch: env_bool("XPIA_VALIDATE_WEBFETCH", true),
            validate_bash: env_bool("XPIA_VALIDATE_BASH", true),
            validate_agents: env_bool("XPIA_VALIDATE_AGENTS", true),
            log_security_events: env_bool("XPIA_LOG_EVENTS", true),
            log_file: env::var("XPIA_LOG_FILE").ok(),
            whitelist_domains: HashSet::new(),
            blacklist_domains: HashSet::new(),
            max_prompt_length: env_usize("XPIA_MAX_PROMPT_LENGTH", 10_000),
            max_url_length: env_usize("XPIA_MAX_URL_LENGTH", 2048),
        };

        config.load_domain_lists();
        config
    }

    /// Determine whether a given risk level should trigger blocking.
    pub fn should_block(&self, risk: RiskLevel) -> bool {
        match risk {
            RiskLevel::Critical => self.block_on_critical,
            RiskLevel::High => self.block_on_high_risk,
            _ => false,
        }
    }

    fn load_domain_lists(&mut self) {
        // Whitelist from env
        if let Ok(domains) = env::var("XPIA_WHITELIST_DOMAINS") {
            self.whitelist_domains
                .extend(domains.split(',').map(|s| s.trim().to_lowercase()));
        }
        // Whitelist from file
        if let Ok(file) = env::var("XPIA_WHITELIST_FILE") {
            load_domains_from_file(&file, &mut self.whitelist_domains);
        }
        // Default safe domains
        for d in DEFAULT_SAFE_DOMAINS {
            self.whitelist_domains.insert((*d).to_string());
        }

        // Blacklist from env
        if let Ok(domains) = env::var("XPIA_BLACKLIST_DOMAINS") {
            self.blacklist_domains
                .extend(domains.split(',').map(|s| s.trim().to_lowercase()));
        }
        // Blacklist from file
        if let Ok(file) = env::var("XPIA_BLACKLIST_FILE") {
            load_domains_from_file(&file, &mut self.blacklist_domains);
        }
    }

    /// Check whether a domain is whitelisted.
    pub fn is_whitelisted(&self, domain: &str) -> bool {
        let lower = domain.to_lowercase();
        self.whitelist_domains
            .iter()
            .any(|d| lower == *d || lower.ends_with(&format!(".{d}")))
    }

    /// Check whether a domain is blacklisted.
    pub fn is_blacklisted(&self, domain: &str) -> bool {
        let lower = domain.to_lowercase();
        self.blacklist_domains
            .iter()
            .any(|d| lower == *d || lower.ends_with(&format!(".{d}")))
    }
}

fn load_domains_from_file(path: &str, set: &mut HashSet<String>) {
    if let Ok(content) = fs::read_to_string(Path::new(path)) {
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                set.insert(trimmed.to_lowercase());
            }
        }
    }
}

fn env_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .map(|v| v.to_lowercase() != "false")
        .unwrap_or(default)
}

fn env_str(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_enabled() {
        let cfg = XpiaConfig::from_env();
        assert!(cfg.enabled);
        assert!(cfg.block_on_critical);
    }

    #[test]
    fn default_safe_domains_whitelisted() {
        let cfg = XpiaConfig::from_env();
        assert!(cfg.is_whitelisted("github.com"));
        assert!(cfg.is_whitelisted("docs.python.org"));
        assert!(!cfg.is_whitelisted("evil.com"));
    }

    #[test]
    fn subdomain_whitelisting() {
        let cfg = XpiaConfig::from_env();
        assert!(cfg.is_whitelisted("api.github.com"));
        assert!(cfg.is_whitelisted("dev.azure.com"));
    }

    #[test]
    fn should_block_critical() {
        let cfg = XpiaConfig::from_env();
        assert!(cfg.should_block(RiskLevel::Critical));
        assert!(cfg.should_block(RiskLevel::High));
        assert!(!cfg.should_block(RiskLevel::Medium));
        assert!(!cfg.should_block(RiskLevel::Low));
    }

    #[test]
    fn blacklist_from_env() {
        // Test blacklist via config API instead of env vars (thread-safe)
        let mut cfg = XpiaConfig::from_env();
        cfg.blacklist_domains.insert("evil.com".to_string());
        cfg.blacklist_domains.insert("malware.org".to_string());
        assert!(cfg.is_blacklisted("evil.com"));
        assert!(cfg.is_blacklisted("sub.malware.org"));
        assert!(!cfg.is_blacklisted("github.com"));
    }

    #[test]
    fn whitelist_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("whitelist.txt");
        fs::write(&file, "custom-domain.com\n# comment\ntrusted.org\n").unwrap();
        let mut cfg = XpiaConfig::from_env();
        // Simulate loading from file
        let mut domains = std::collections::HashSet::new();
        super::load_domains_from_file(file.to_str().unwrap(), &mut domains);
        cfg.whitelist_domains.extend(domains);
        assert!(cfg.is_whitelisted("custom-domain.com"));
        assert!(cfg.is_whitelisted("trusted.org"));
    }
}
