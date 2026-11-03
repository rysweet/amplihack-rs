//! XPIA system health checks.

use crate::config::{EnabledValue, XpiaConfig, classify_enabled, unrecognised_enabled_message};
use crate::patterns::XpiaPatterns;
use serde::{Deserialize, Serialize};

/// Overall health status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded => write!(f, "degraded"),
            Self::Unhealthy => write!(f, "unhealthy"),
        }
    }
}

/// Individual check result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

/// Aggregated health report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub status: HealthStatus,
    pub checks: Vec<CheckResult>,
    pub pattern_count: usize,
    pub config_summary: serde_json::Value,
}

/// Run all XPIA health checks and return a report.
pub fn check_health() -> HealthReport {
    check_health_with(&XpiaConfig::from_env())
}

/// Run all XPIA health checks against the given configuration.
pub fn check_health_with(config: &XpiaConfig) -> HealthReport {
    let patterns = XpiaPatterns::new();

    let mut checks = Vec::new();

    // Check 1: XPIA enabled (#1737 item 2). Passes when the state is what a
    // recognised `XPIA_ENABLED` value asks for, on or off; fails, naming the
    // value, when the value is not recognised. A configuration built in code
    // (no raw value) follows `enabled`. The reported state is always
    // `config.enabled`, so a hand-set `enabled` that disagrees with the raw
    // value fails rather than hiding a running (or missing) scanner.
    // `config_summary.enabled` is the machine-readable answer to whether XPIA
    // is scanning.
    let asked = match &config.enabled_raw {
        Some(raw) => classify_enabled(Some(raw)),
        None => EnabledValue::Recognised(config.enabled),
    };
    let state = if config.enabled { "on" } else { "off" };
    let (passed, message) = match asked {
        EnabledValue::Unrecognised(raw) => (false, unrecognised_enabled_message(&raw)),
        EnabledValue::Recognised(asked) if asked != config.enabled => (
            false,
            format!(
                "XPIA_ENABLED asks for XPIA {} but XPIA is {state}",
                if asked { "on" } else { "off" }
            ),
        ),
        EnabledValue::Recognised(true) => (true, "XPIA defense is enabled".to_string()),
        EnabledValue::Recognised(false) => (
            true,
            "XPIA defense is disabled (default; set XPIA_ENABLED=true to opt in)".to_string(),
        ),
    };
    checks.push(CheckResult {
        name: "xpia_enabled".into(),
        passed,
        message,
    });

    // Check 2: Patterns loaded
    let pattern_count = patterns.all().len();
    checks.push(CheckResult {
        name: "patterns_loaded".into(),
        passed: pattern_count >= 15,
        message: format!("{pattern_count} attack patterns loaded"),
    });

    // Check 3: Blocking thresholds
    let blocking_configured = config.block_on_critical || config.block_on_high_risk;
    checks.push(CheckResult {
        name: "blocking_configured".into(),
        passed: blocking_configured,
        message: if blocking_configured {
            format!(
                "Blocking: critical={}, high={}",
                config.block_on_critical, config.block_on_high_risk
            )
        } else {
            "WARNING: No blocking thresholds configured".into()
        },
    });

    // Check 4: Feature flags
    let features_enabled =
        config.validate_webfetch || config.validate_bash || config.validate_agents;
    checks.push(CheckResult {
        name: "features_enabled".into(),
        passed: features_enabled,
        message: format!(
            "webfetch={}, bash={}, agents={}",
            config.validate_webfetch, config.validate_bash, config.validate_agents
        ),
    });

    // Check 5: Domain lists
    let has_whitelist = !config.whitelist_domains.is_empty();
    checks.push(CheckResult {
        name: "domain_lists".into(),
        passed: has_whitelist,
        message: format!(
            "whitelist={} domains, blacklist={} domains",
            config.whitelist_domains.len(),
            config.blacklist_domains.len()
        ),
    });

    // Check 6: Limits configured
    let limits_sane = config.max_prompt_length >= 100 && config.max_url_length >= 10;
    checks.push(CheckResult {
        name: "limits_configured".into(),
        passed: limits_sane,
        message: format!(
            "max_prompt={}, max_url={}",
            config.max_prompt_length, config.max_url_length
        ),
    });

    // Determine overall status
    let failed = checks.iter().filter(|c| !c.passed).count();
    let status = if failed == 0 {
        HealthStatus::Healthy
    } else if failed <= 2 {
        HealthStatus::Degraded
    } else {
        HealthStatus::Unhealthy
    };

    let config_summary = serde_json::json!({
        "enabled": config.enabled,
        "security_level": config.security_level.to_string(),
        "block_on_critical": config.block_on_critical,
        "block_on_high_risk": config.block_on_high_risk,
    });

    HealthReport {
        status,
        checks,
        pattern_count,
        config_summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_check_returns_report() {
        let report = check_health_with(&XpiaConfig::defaults());
        assert!(!report.checks.is_empty());
        assert!(report.pattern_count >= 15);
    }

    #[test]
    fn default_config_is_healthy() {
        let report = check_health_with(&XpiaConfig::defaults());
        assert_eq!(report.status, HealthStatus::Healthy);
    }

    #[test]
    fn disabled_xpia_is_healthy() {
        // XPIA is off by default (#1734), so off is the expected state and
        // must not make a default install look unhealthy.
        let report = check_health_with(&XpiaConfig::defaults());
        assert_eq!(report.status, HealthStatus::Healthy);
        assert_eq!(report.checks.iter().filter(|c| !c.passed).count(), 0);
        let enabled = report
            .checks
            .iter()
            .find(|c| c.name == "xpia_enabled")
            .unwrap();
        assert!(enabled.passed);
        assert!(
            enabled.message.contains("disabled"),
            "message must say XPIA is disabled: {}",
            enabled.message
        );
        assert!(
            enabled.message.contains("XPIA_ENABLED=true"),
            "message must say how to opt in: {}",
            enabled.message
        );
        assert!(
            !enabled.message.contains("unprotected"),
            "the default state must not be reported as a fault: {}",
            enabled.message
        );
        // The summary still reports the real state.
        assert_eq!(report.config_summary["enabled"], false);
    }

    #[test]
    fn enabled_xpia_is_healthy() {
        let mut config = XpiaConfig::defaults();
        config.enabled = true;
        let report = check_health_with(&config);
        assert_eq!(report.status, HealthStatus::Healthy);
        let enabled = report
            .checks
            .iter()
            .find(|c| c.name == "xpia_enabled")
            .unwrap();
        assert!(enabled.passed);
        assert_eq!(enabled.message, "XPIA defense is enabled");
        assert_eq!(report.config_summary["enabled"], true);
    }

    #[test]
    fn report_contains_all_checks() {
        let report = check_health_with(&XpiaConfig::defaults());
        let names: Vec<&str> = report.checks.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"xpia_enabled"));
        assert!(names.contains(&"patterns_loaded"));
        assert!(names.contains(&"blocking_configured"));
        assert!(names.contains(&"features_enabled"));
        assert!(names.contains(&"domain_lists"));
        assert!(names.contains(&"limits_configured"));
    }

    #[test]
    fn health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(HealthStatus::Degraded.to_string(), "degraded");
        assert_eq!(HealthStatus::Unhealthy.to_string(), "unhealthy");
    }

    #[test]
    fn health_report_serialization() {
        let report = check_health_with(&XpiaConfig::defaults());
        let json = serde_json::to_string(&report).unwrap();
        let deserialized: HealthReport = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.status, report.status);
        assert_eq!(deserialized.checks.len(), report.checks.len());
        assert_eq!(deserialized.pattern_count, report.pattern_count);
    }

    #[test]
    fn health_check_has_six_checks() {
        let report = check_health_with(&XpiaConfig::defaults());
        assert_eq!(report.checks.len(), 6);
    }

    #[test]
    fn health_config_summary_has_expected_fields() {
        let report = check_health_with(&XpiaConfig::defaults());
        assert!(report.config_summary.get("enabled").is_some());
        assert!(report.config_summary.get("security_level").is_some());
        assert!(report.config_summary.get("block_on_critical").is_some());
        assert!(report.config_summary.get("block_on_high_risk").is_some());
    }

    #[test]
    fn health_status_serde_roundtrip() {
        for status in [
            HealthStatus::Healthy,
            HealthStatus::Degraded,
            HealthStatus::Unhealthy,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: HealthStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn all_default_checks_pass() {
        let report = check_health_with(&XpiaConfig::defaults());
        for check in &report.checks {
            assert!(
                check.passed,
                "check '{}' failed with message: {}",
                check.name, check.message
            );
        }
    }
}

/// Issue #1737 item 2 (D4): `xpia_enabled` passes when the state is what a
/// recognised `XPIA_ENABLED` asks for, and fails, naming the value, when the
/// value is not recognised.
#[cfg(test)]
mod enabled_check_tests {
    use super::*;

    fn config_for(value: &str) -> XpiaConfig {
        let value = value.to_string();
        XpiaConfig::from_lookup(move |k| (k == "XPIA_ENABLED").then(|| value.clone()))
    }

    fn enabled_check(report: &HealthReport) -> &CheckResult {
        report
            .checks
            .iter()
            .find(|c| c.name == "xpia_enabled")
            .expect("xpia_enabled check present")
    }

    #[test]
    fn unrecognised_enabled_fails_and_names_value() {
        for value in ["on", "enabled", " true "] {
            let report = check_health_with(&config_for(value));
            let check = enabled_check(&report);
            assert!(!check.passed, "XPIA_ENABLED={value:?} must fail the check");
            assert_eq!(
                check.message,
                format!(
                    "XPIA_ENABLED={value:?} is not recognised; XPIA is off (set XPIA_ENABLED=true to opt in)"
                )
            );
            assert_eq!(report.status, HealthStatus::Degraded, "{value:?}");
            assert_eq!(report.config_summary["enabled"], false);
            assert_eq!(report.checks.len(), 6);
        }
    }

    #[test]
    fn recognised_on_values_pass() {
        for value in ["1", "TRUE", "yes"] {
            let report = check_health_with(&config_for(value));
            let check = enabled_check(&report);
            assert!(check.passed, "XPIA_ENABLED={value:?}: {}", check.message);
            assert_eq!(check.message, "XPIA defense is enabled");
            assert_eq!(report.config_summary["enabled"], true);
        }
    }

    #[test]
    fn recognised_off_values_pass_with_opt_in_hint() {
        for value in ["", "0", "false", "no"] {
            let report = check_health_with(&config_for(value));
            let check = enabled_check(&report);
            assert!(check.passed, "XPIA_ENABLED={value:?}: {}", check.message);
            assert_eq!(
                check.message,
                "XPIA defense is disabled (default; set XPIA_ENABLED=true to opt in)"
            );
            assert_eq!(report.status, HealthStatus::Healthy);
        }
    }

    #[test]
    fn non_utf8_placeholder_fails_and_names_it() {
        // `from_env` records a non-UTF-8 value as the placeholder, so health
        // reports it as the hook does instead of as "disabled (default)".
        let report = check_health_with(&config_for(crate::config::NON_UTF8_PLACEHOLDER));
        let check = enabled_check(&report);
        assert!(!check.passed);
        assert!(check.message.contains("<non-UTF-8>"), "{}", check.message);
        assert_eq!(report.config_summary["enabled"], false);
    }

    #[test]
    fn enabled_field_that_disagrees_with_raw_value_fails() {
        // A caller that sets `enabled` by hand after `from_lookup` must not
        // make health report a state XPIA is not in.
        let mut config = config_for("false");
        config.enabled = true;
        let check = enabled_check(&check_health_with(&config)).clone();
        assert!(!check.passed);
        assert_eq!(
            check.message,
            "XPIA_ENABLED asks for XPIA off but XPIA is on"
        );

        let mut config = config_for("true");
        config.enabled = false;
        let check = enabled_check(&check_health_with(&config)).clone();
        assert!(!check.passed);
        assert_eq!(
            check.message,
            "XPIA_ENABLED asks for XPIA on but XPIA is off"
        );
    }

    #[test]
    fn config_built_in_code_uses_enabled_field() {
        // No raw value (built in code, not from the environment): the check
        // follows `enabled` and passes either way.
        let mut config = XpiaConfig::defaults();
        assert!(enabled_check(&check_health_with(&config)).passed);
        config.enabled = true;
        let report = check_health_with(&config);
        assert!(enabled_check(&report).passed);
        assert_eq!(enabled_check(&report).message, "XPIA defense is enabled");
    }
}
