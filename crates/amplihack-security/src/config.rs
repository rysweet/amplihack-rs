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
    /// The raw `XPIA_ENABLED` value this configuration was built from, or
    /// `None` when it was unset or the configuration was built in code.
    /// Health uses it to name a value that is not recognised (#1737).
    pub enabled_raw: Option<String>,
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
        Self::from_lookup(|key| match key {
            // A non-UTF-8 `XPIA_ENABLED` is recorded as a placeholder rather
            // than treated as unset, so health and the hook agree on it.
            "XPIA_ENABLED" => enabled_raw_from_os(env::var_os(key).as_deref()),
            _ => env::var(key).ok(),
        })
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
        let env_bool = |key: &str, default: bool| lookup(key).map_or(default, |v| parse_bool(&v));
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

        // Only a recognised on value turns XPIA on; unset is off (#1734).
        let enabled_raw = lookup("XPIA_ENABLED");
        let enabled = matches!(
            classify_enabled(enabled_raw.as_deref()),
            EnabledValue::Recognised(true)
        );

        let mut config = Self {
            enabled,
            enabled_raw,
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

/// `1`, `true` or `yes` in any case is true; any other value is false.
fn parse_bool(value: &str) -> bool {
    matches!(value.to_lowercase().as_str(), "1" | "true" | "yes")
}

/// How an `XPIA_ENABLED` value is read (#1737 item 1).
///
/// Only `Recognised(true)` turns XPIA on, and exactly when [`parse_bool`]
/// would. Everything that is neither a recognised on nor a recognised off
/// value is `Unrecognised`, which leaves XPIA off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnabledValue {
    /// `1`, `true` or `yes` (on), or unset, empty, `0`, `false` or `no`
    /// (off), in any case and without trimming.
    Recognised(bool),
    /// Any other value, kept as given (for example `on` or `" true "`).
    Unrecognised(String),
}

/// Shown in place of an `XPIA_ENABLED` value that is not valid UTF-8.
pub const NON_UTF8_PLACEHOLDER: &str = "<non-UTF-8>";

/// Convert a raw `XPIA_ENABLED` value into the string used for classification
/// and messages. A non-UTF-8 value becomes [`NON_UTF8_PLACEHOLDER`], never
/// `None`, so it is reported as unrecognised rather than read as unset.
pub fn enabled_raw_from_os(value: Option<&std::ffi::OsStr>) -> Option<String> {
    value.map(|v| {
        v.to_str()
            .map_or_else(|| NON_UTF8_PLACEHOLDER.to_string(), str::to_string)
    })
}

/// Classify a raw `XPIA_ENABLED` value. `None` means unset, which is off.
pub fn classify_enabled(value: Option<&str>) -> EnabledValue {
    let Some(raw) = value else {
        return EnabledValue::Recognised(false);
    };
    match raw.to_lowercase().as_str() {
        "1" | "true" | "yes" => EnabledValue::Recognised(true),
        "" | "0" | "false" | "no" => EnabledValue::Recognised(false),
        _ => EnabledValue::Unrecognised(raw.to_string()),
    }
}

/// Longest part of an unrecognised value echoed back, in characters.
const MAX_ECHOED_CHARS: usize = 64;

/// The message for an unrecognised `XPIA_ENABLED` value.
///
/// The value is cut to 64 characters (on a character boundary, marked with
/// `…`) and then escaped with `{:?}`, so no control character reaches a
/// terminal or log.
pub fn unrecognised_enabled_message(raw: &str) -> String {
    let shown = match raw.char_indices().nth(MAX_ECHOED_CHARS) {
        Some((cut, _)) => format!("{}…", &raw[..cut]),
        None => raw.to_string(),
    };
    format!(
        "XPIA_ENABLED={shown:?} is not recognised; XPIA is off (set XPIA_ENABLED=true to opt in)"
    )
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

/// Issue #1737 item 1 (D5, D6): `classify_enabled` tells a recognised value
/// from an unrecognised one, so health and the hook can name what the user
/// typed. These tests define the contract; `parse_bool` stays as it is.
#[cfg(test)]
mod classify_tests {
    use super::*;

    #[test]
    fn classify_enabled_recognised_on() {
        for value in ["1", "true", "TRUE", "True", "yes", "Yes", "YES"] {
            assert_eq!(
                classify_enabled(Some(value)),
                EnabledValue::Recognised(true),
                "XPIA_ENABLED={value:?}"
            );
        }
    }

    #[test]
    fn classify_enabled_recognised_off() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("FALSE"),
            Some("no"),
            Some("No"),
        ] {
            assert_eq!(
                classify_enabled(value),
                EnabledValue::Recognised(false),
                "XPIA_ENABLED={value:?}"
            );
        }
    }

    #[test]
    fn classify_enabled_unrecognised_values_are_not_trimmed() {
        for value in ["on", "enabled", " true ", " 1", "maybe", "off", "true\n"] {
            assert_eq!(
                classify_enabled(Some(value)),
                EnabledValue::Unrecognised(value.to_string()),
                "XPIA_ENABLED={value:?}"
            );
        }
    }

    #[test]
    fn enabled_raw_from_os_maps_unset_and_utf8_values() {
        assert_eq!(enabled_raw_from_os(None), None);
        assert_eq!(
            enabled_raw_from_os(Some(std::ffi::OsStr::new("true"))),
            Some("true".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn enabled_raw_from_os_records_non_utf8_as_placeholder() {
        use std::os::unix::ffi::OsStrExt;
        let raw = enabled_raw_from_os(Some(std::ffi::OsStr::from_bytes(b"tr\xffue")));
        assert_eq!(raw.as_deref(), Some(NON_UTF8_PLACEHOLDER));
        assert_eq!(
            classify_enabled(raw.as_deref()),
            EnabledValue::Unrecognised(NON_UTF8_PLACEHOLDER.to_string())
        );
    }

    #[test]
    fn classify_enabled_agrees_with_parse_bool() {
        // Only Recognised(true) may turn XPIA on, and exactly when parse_bool
        // says so.
        for value in [
            "", "0", "1", "true", "TRUE", "yes", "no", "false", "on", "enabled", " true ", " 1",
        ] {
            let on = matches!(
                classify_enabled(Some(value)),
                EnabledValue::Recognised(true)
            );
            assert_eq!(on, parse_bool(value), "XPIA_ENABLED={value:?}");
        }
    }

    #[test]
    fn from_lookup_records_the_raw_value() {
        let cfg = XpiaConfig::from_lookup(|k| (k == "XPIA_ENABLED").then(|| "on".into()));
        assert_eq!(cfg.enabled_raw.as_deref(), Some("on"));
        assert!(!cfg.enabled, "an unrecognised value leaves XPIA off");

        let unset = XpiaConfig::from_lookup(|_| None);
        assert_eq!(unset.enabled_raw, None);
        assert_eq!(XpiaConfig::defaults().enabled_raw, None);
    }

    #[test]
    fn unrecognised_message_has_the_exact_wording() {
        assert_eq!(
            unrecognised_enabled_message("on"),
            "XPIA_ENABLED=\"on\" is not recognised; XPIA is off (set XPIA_ENABLED=true to opt in)"
        );
        assert_eq!(
            unrecognised_enabled_message(" true "),
            "XPIA_ENABLED=\" true \" is not recognised; XPIA is off (set XPIA_ENABLED=true to opt in)"
        );
    }

    #[test]
    fn unrecognised_message_escapes_control_characters() {
        let msg = unrecognised_enabled_message("\u{1b}[31mtrue\n\0");
        assert!(!msg.contains('\u{1b}'), "raw ESC leaked: {msg:?}");
        assert!(!msg.contains('\n'), "raw LF leaked: {msg:?}");
        assert!(!msg.contains('\0'), "raw NUL leaked: {msg:?}");
        assert!(!msg.contains('\r'));
        assert!(msg.contains("\\u{1b}"), "{msg:?}");
        assert!(msg.contains("\\n"), "{msg:?}");
    }

    #[test]
    fn unrecognised_message_caps_long_values() {
        let long = "x".repeat(10_000);
        let msg = unrecognised_enabled_message(&long);
        let expected = format!("{:?}", format!("{}…", "x".repeat(64)));
        assert!(
            msg.contains(&format!("XPIA_ENABLED={expected} ")),
            "value must be cut to 64 chars plus an ellipsis: {msg}"
        );
        assert!(
            msg.len() < 300,
            "message must stay short: {} bytes",
            msg.len()
        );
    }

    #[test]
    fn unrecognised_message_cuts_on_a_char_boundary() {
        // 100 four-byte chars: a byte-index cut at 64 would split one and panic.
        let wide = "🦀".repeat(100);
        let msg = unrecognised_enabled_message(&wide);
        let kept = format!("{}…", "🦀".repeat(64));
        assert!(msg.contains(&kept), "{msg}");
        assert!(!msg.contains(&"🦀".repeat(65)), "{msg}");
    }

    #[test]
    fn short_value_is_not_marked_as_cut() {
        let exactly = "y".repeat(64);
        let msg = unrecognised_enabled_message(&exactly);
        assert!(!msg.contains('…'), "{msg}");
    }
}
