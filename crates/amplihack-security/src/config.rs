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
        Self::from_lookup(|key| env::var(key).ok())
    }

    /// Built-in defaults, ignoring every `XPIA_*` environment variable.
    ///
    /// XPIA is off by default (opt in with `XPIA_ENABLED=true`). Tests use
    /// this so that a session which exports, for example, `XPIA_ENABLED=true`
    /// cannot change their outcome.
    pub fn defaults() -> Self {
        Self::from_lookup(|_| None)
    }

    /// Build a configuration from a variable lookup (`None` means unset).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let env_bool = |key: &str, default: bool| match lookup(key) {
            Some(v) => matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"),
            None => default,
        };
        let env_usize =
            |key: &str, default: usize| lookup(key).and_then(|v| v.parse().ok()).unwrap_or(default);
        let security_level = match lookup("XPIA_SECURITY_LEVEL")
            .unwrap_or_else(|| "MODERATE".to_string())
            .to_uppercase()
            .as_str()
        {
            "STRICT" => SecurityLevel::Strict,
            "HIGH" => SecurityLevel::High,
            "LENIENT" | "LOW" => SecurityLevel::Low,
            _ => SecurityLevel::Medium,
        };

        let mut config = Self {
            enabled: env_bool("XPIA_ENABLED", false),
            security_level,
            verbose_feedback: env_bool("XPIA_VERBOSE_FEEDBACK", false),
            block_on_high_risk: env_bool("XPIA_BLOCK_HIGH_RISK", true),
            block_on_critical: env_bool("XPIA_BLOCK_CRITICAL", true),
            validate_webfetch: env_bool("XPIA_VALIDATE_WEBFETCH", true),
            validate_bash: env_bool("XPIA_VALIDATE_BASH", true),
            validate_agents: env_bool("XPIA_VALIDATE_AGENTS", true),
            log_security_events: env_bool("XPIA_LOG_EVENTS", true),
            log_file: lookup("XPIA_LOG_FILE"),
            whitelist_domains: HashSet::new(),
            blacklist_domains: HashSet::new(),
            max_prompt_length: env_usize("XPIA_MAX_PROMPT_LENGTH", 10_000),
            max_url_length: env_usize("XPIA_MAX_URL_LENGTH", 2048),
        };

        config.load_domain_lists(&lookup);
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

    fn load_domain_lists(&mut self, lookup: &impl Fn(&str) -> Option<String>) {
        // Whitelist from env
        if let Some(domains) = lookup("XPIA_WHITELIST_DOMAINS") {
            self.whitelist_domains
                .extend(domains.split(',').map(|s| s.trim().to_lowercase()));
        }
        // Whitelist from file
        if let Some(file) = lookup("XPIA_WHITELIST_FILE") {
            load_domains_from_file(&file, &mut self.whitelist_domains);
        }
        // Default safe domains
        for d in DEFAULT_SAFE_DOMAINS {
            self.whitelist_domains.insert((*d).to_string());
        }

        // Blacklist from env
        if let Some(domains) = lookup("XPIA_BLACKLIST_DOMAINS") {
            self.blacklist_domains
                .extend(domains.split(',').map(|s| s.trim().to_lowercase()));
        }
        // Blacklist from file
        if let Some(file) = lookup("XPIA_BLACKLIST_FILE") {
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
    match fs::read_to_string(Path::new(path)) {
        Ok(content) => {
            for line in content.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    set.insert(trimmed.to_lowercase());
                }
            }
        }
        Err(e) => {
            tracing::warn!(path, error = %e, "failed to load domain list — using defaults");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_disabled() {
        // Owner decision (#1734): XPIA is off unless the user opts in.
        let cfg = XpiaConfig::defaults();
        assert!(!cfg.enabled, "XPIA must be disabled by default");
        assert!(cfg.block_on_critical);
    }

    #[test]
    fn default_safe_domains_whitelisted() {
        let cfg = XpiaConfig::defaults();
        assert!(cfg.is_whitelisted("github.com"));
        assert!(cfg.is_whitelisted("docs.python.org"));
        assert!(!cfg.is_whitelisted("evil.com"));
    }

    #[test]
    fn subdomain_whitelisting() {
        let cfg = XpiaConfig::defaults();
        assert!(cfg.is_whitelisted("api.github.com"));
        assert!(cfg.is_whitelisted("dev.azure.com"));
    }

    #[test]
    fn should_block_critical() {
        let cfg = XpiaConfig::defaults();
        assert!(cfg.should_block(RiskLevel::Critical));
        assert!(cfg.should_block(RiskLevel::High));
        assert!(!cfg.should_block(RiskLevel::Medium));
        assert!(!cfg.should_block(RiskLevel::Low));
    }

    #[test]
    fn blacklist_from_env() {
        // Test blacklist via config API instead of env vars (thread-safe)
        let mut cfg = XpiaConfig::defaults();
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
        let mut cfg = XpiaConfig::defaults();
        // Simulate loading from file
        let mut domains = std::collections::HashSet::new();
        super::load_domains_from_file(file.to_str().unwrap(), &mut domains);
        cfg.whitelist_domains.extend(domains);
        assert!(cfg.is_whitelisted("custom-domain.com"));
        assert!(cfg.is_whitelisted("trusted.org"));
    }

    #[test]
    fn should_block_with_critical_disabled() {
        let mut cfg = XpiaConfig::defaults();
        cfg.block_on_critical = false;
        assert!(!cfg.should_block(RiskLevel::Critical));
        assert!(cfg.should_block(RiskLevel::High));
    }

    #[test]
    fn should_block_with_high_disabled() {
        let mut cfg = XpiaConfig::defaults();
        cfg.block_on_high_risk = false;
        assert!(cfg.should_block(RiskLevel::Critical));
        assert!(!cfg.should_block(RiskLevel::High));
    }

    #[test]
    fn should_block_none_and_low_never_blocked() {
        let mut cfg = XpiaConfig::defaults();
        cfg.block_on_critical = true;
        cfg.block_on_high_risk = true;
        assert!(!cfg.should_block(RiskLevel::None));
        assert!(!cfg.should_block(RiskLevel::Low));
        assert!(!cfg.should_block(RiskLevel::Medium));
    }

    #[test]
    fn is_whitelisted_case_insensitive() {
        let cfg = XpiaConfig::defaults();
        assert!(cfg.is_whitelisted("GitHub.COM"));
        assert!(cfg.is_whitelisted("GITHUB.COM"));
        assert!(cfg.is_whitelisted("Api.GitHub.Com"));
    }

    #[test]
    fn is_blacklisted_case_insensitive() {
        let mut cfg = XpiaConfig::defaults();
        cfg.blacklist_domains.insert("evil.com".to_string());
        assert!(cfg.is_blacklisted("Evil.Com"));
        assert!(cfg.is_blacklisted("EVIL.COM"));
        assert!(cfg.is_blacklisted("sub.EVIL.COM"));
    }

    #[test]
    fn is_whitelisted_empty_string() {
        let cfg = XpiaConfig::defaults();
        assert!(!cfg.is_whitelisted(""));
    }

    #[test]
    fn is_blacklisted_empty_string() {
        let cfg = XpiaConfig::defaults();
        assert!(!cfg.is_blacklisted(""));
    }

    #[test]
    fn default_limits_are_sane() {
        let cfg = XpiaConfig::defaults();
        assert_eq!(cfg.max_prompt_length, 10_000);
        assert_eq!(cfg.max_url_length, 2048);
    }

    #[test]
    fn default_safe_domains_all_present() {
        let cfg = XpiaConfig::defaults();
        for domain in super::DEFAULT_SAFE_DOMAINS {
            assert!(
                cfg.is_whitelisted(domain),
                "default safe domain {domain} should be whitelisted"
            );
        }
    }

    #[test]
    fn load_domains_from_nonexistent_file() {
        let mut domains = HashSet::new();
        // Should not panic, just log a warning
        super::load_domains_from_file("/nonexistent/path/domains.txt", &mut domains);
        assert!(domains.is_empty());
    }

    #[test]
    fn load_domains_from_file_skips_empty_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("domains.txt");
        fs::write(&file, "\n\n  \n# comment\nexample.com\n\n  test.org  \n").unwrap();
        let mut domains = HashSet::new();
        super::load_domains_from_file(file.to_str().unwrap(), &mut domains);
        assert_eq!(domains.len(), 2);
        assert!(domains.contains("example.com"));
        assert!(domains.contains("test.org"));
    }

    #[test]
    fn blacklist_overrides_whitelist_check() {
        let mut cfg = XpiaConfig::defaults();
        cfg.whitelist_domains.insert("dual.com".to_string());
        cfg.blacklist_domains.insert("dual.com".to_string());
        // Both match — callers should check blacklist first
        assert!(cfg.is_whitelisted("dual.com"));
        assert!(cfg.is_blacklisted("dual.com"));
    }
}

#[cfg(test)]
mod lookup_tests {
    use super::*;

    #[test]
    fn lookup_overrides_defaults() {
        let cfg = XpiaConfig::from_lookup(|k| match k {
            "XPIA_ENABLED" => Some("false".into()),
            "XPIA_SECURITY_LEVEL" => Some("strict".into()),
            "XPIA_MAX_URL_LENGTH" => Some("99".into()),
            "XPIA_BLACKLIST_DOMAINS" => Some("Evil.com, bad.org".into()),
            _ => None,
        });
        assert!(!cfg.enabled);
        assert_eq!(cfg.security_level, SecurityLevel::Strict);
        assert_eq!(cfg.max_url_length, 99);
        assert!(cfg.is_blacklisted("sub.evil.com"));
        assert!(cfg.is_blacklisted("bad.org"));
    }

    #[test]
    fn defaults_ignore_lookup_free_environment() {
        let cfg = XpiaConfig::defaults();
        assert!(!cfg.enabled);
        assert_eq!(cfg.max_prompt_length, 10_000);
        assert!(cfg.blacklist_domains.is_empty());
    }

    fn enabled_for(value: Option<&str>) -> bool {
        XpiaConfig::from_lookup(|k| match k {
            "XPIA_ENABLED" => value.map(str::to_string),
            _ => None,
        })
        .enabled
    }

    #[test]
    fn xpia_enabled_value_table() {
        // Off: unset, empty, unrecognised, and the explicit false forms.
        // Values are not trimmed, so " true " stays off.
        for value in [
            None,
            Some(""),
            Some("maybe"),
            Some("on"),
            Some("false"),
            Some("FALSE"),
            Some("0"),
            Some("no"),
            Some(" true "),
        ] {
            assert!(
                !enabled_for(value),
                "XPIA_ENABLED={value:?} must leave XPIA disabled"
            );
        }
        // On: the accepted truthy forms, in any case.
        for value in ["true", "TRUE", "True", "1", "yes", "Yes", "YES"] {
            assert!(
                enabled_for(Some(value)),
                "XPIA_ENABLED={value:?} must enable XPIA"
            );
        }
    }

    #[test]
    fn explicit_opt_in_keeps_full_protection() {
        let cfg = XpiaConfig::from_lookup(|k| (k == "XPIA_ENABLED").then(|| "true".into()));
        assert!(cfg.enabled);
        assert!(cfg.block_on_critical);
        assert!(cfg.block_on_high_risk);
        assert!(cfg.validate_bash);
        assert!(cfg.validate_webfetch);
        assert!(cfg.validate_agents);
    }

    #[test]
    fn other_xpia_keys_cannot_enable_xpia_on_their_own() {
        // Every other XPIA_* key at its strictest value, XPIA_ENABLED unset.
        let cfg = XpiaConfig::from_lookup(|k| match k {
            "XPIA_SECURITY_LEVEL" => Some("STRICT".into()),
            "XPIA_BLOCK_HIGH_RISK"
            | "XPIA_BLOCK_CRITICAL"
            | "XPIA_VALIDATE_WEBFETCH"
            | "XPIA_VALIDATE_BASH"
            | "XPIA_VALIDATE_AGENTS"
            | "XPIA_LOG_EVENTS"
            | "XPIA_VERBOSE_FEEDBACK" => Some("true".into()),
            "XPIA_BLACKLIST_DOMAINS" => Some("evil.com".into()),
            _ => None,
        });
        assert!(
            !cfg.enabled,
            "only XPIA_ENABLED may turn XPIA on; sub-settings must not"
        );
    }
}
