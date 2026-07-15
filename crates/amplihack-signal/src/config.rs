//! Env-first Signal configuration resolver.
//!
//! Precedence is **env > file > explicit error**. A missing *required* value is
//! always a hard [`ConfigError`], never a silent default. An *empty* allowlist
//! parses successfully to the empty set — that is the fail-closed default
//! (nobody is trusted) and is deliberately NOT an error.

use std::collections::HashMap;
use std::time::Duration;

/// Env var: `host:port` of the signal-cli JSON-RPC daemon. Required.
pub const ENV_ENDPOINT: &str = "AMPLIHACK_SIGNAL_ENDPOINT";
/// Env var: operator's own E.164 account. Required.
pub const ENV_ACCOUNT: &str = "AMPLIHACK_SIGNAL_ACCOUNT";
/// Env var: comma-separated E.164 allowlist. Empty string => fail-closed set.
pub const ENV_ALLOWLIST: &str = "AMPLIHACK_SIGNAL_ALLOWLIST";
/// Env var: optional bot own device id (>= 2) for defence-in-depth loop guard.
pub const ENV_OWN_DEVICE_ID: &str = "AMPLIHACK_SIGNAL_OWN_DEVICE_ID";
/// Env var: echo-suppression TTL in seconds. Optional (default 30).
pub const ENV_ECHO_TTL_SECS: &str = "AMPLIHACK_SIGNAL_ECHO_TTL_SECS";
/// Env var: group mode, `per-session` (default) or `rolling`.
pub const ENV_GROUP_MODE: &str = "AMPLIHACK_SIGNAL_GROUP_MODE";
/// Env var: rolling group id. Required only when group mode = rolling.
pub const ENV_ROLLING_GROUP_ID: &str = "AMPLIHACK_SIGNAL_ROLLING_GROUP_ID";
/// Env var: max inbound frame size in bytes. Optional (default 1 MiB).
pub const ENV_MAX_FRAME_BYTES: &str = "AMPLIHACK_SIGNAL_MAX_FRAME_BYTES";

/// Default echo-suppression TTL (seconds) when unset.
pub const DEFAULT_ECHO_TTL_SECS: u64 = 30;
/// Default maximum inbound frame size (1 MiB) when unset.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 1024 * 1024;

/// How per-session Signal groups are managed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMode {
    /// Create a fresh group per session and `quitGroup` on Stop.
    PerSession,
    /// Reuse a single rolling group; never create, never quit.
    Rolling,
}

/// Fully resolved, validated Signal configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalConfig {
    /// `host:port` of the signal-cli JSON-RPC daemon.
    pub endpoint: String,
    /// Operator's own E.164 account.
    pub account: String,
    /// Trusted E.164 senders. Empty = fail-closed (nobody trusted).
    pub allowlist: Vec<String>,
    /// Optional bot own device id (>= 2) for the loop guard.
    pub own_device_id: Option<u32>,
    /// Echo-suppression TTL window.
    pub echo_ttl: Duration,
    /// Group management mode.
    pub group_mode: GroupMode,
    /// Rolling group id (present iff `group_mode == Rolling`).
    pub rolling_group_id: Option<String>,
    /// Max inbound frame size accepted by the transport parser.
    pub max_frame_bytes: usize,
}

/// Configuration resolution failure. Surfaced explicitly — never swallowed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    /// A required variable was not set in env or file.
    #[error("required signal config var {0} is not set")]
    MissingRequired(String),
    /// A value was present but failed validation.
    #[error("invalid value for {var}: {reason}")]
    Invalid {
        /// The offending variable name.
        var: String,
        /// Human-readable reason.
        reason: String,
    },
}

impl SignalConfig {
    /// Resolve from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let vars: HashMap<String, String> = std::env::vars().collect();
        Self::resolve_from(&vars)
    }

    /// Resolve from an explicit variable map (env layer already merged over the
    /// optional file layer by the caller). This is the pure, test-friendly seam.
    pub fn resolve_from(_vars: &HashMap<String, String>) -> Result<Self, ConfigError> {
        todo!("implement env>file>error resolution (P1)")
    }
}

/// Validate an E.164 phone number (`+` followed by 7-15 digits).
pub fn is_valid_e164(_value: &str) -> bool {
    todo!("implement E.164 validation (P1)")
}

/// Parse a comma-separated allowlist. Empty/whitespace => empty (fail-closed).
/// Every entry must be valid E.164 or the whole parse fails.
pub fn parse_allowlist(_raw: &str) -> Result<Vec<String>, ConfigError> {
    todo!("implement allowlist parsing (P1)")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_vars() -> HashMap<String, String> {
        let mut v = HashMap::new();
        v.insert(ENV_ENDPOINT.into(), "127.0.0.1:7583".into());
        v.insert(ENV_ACCOUNT.into(), "+15551230000".into());
        v.insert(ENV_ALLOWLIST.into(), "+15551239999".into());
        v
    }

    #[test]
    fn resolves_minimal_valid_config() {
        let cfg = SignalConfig::resolve_from(&base_vars()).expect("valid config");
        assert_eq!(cfg.endpoint, "127.0.0.1:7583");
        assert_eq!(cfg.account, "+15551230000");
        assert_eq!(cfg.allowlist, vec!["+15551239999".to_string()]);
        assert_eq!(cfg.group_mode, GroupMode::PerSession);
        assert_eq!(cfg.echo_ttl, Duration::from_secs(DEFAULT_ECHO_TTL_SECS));
        assert_eq!(cfg.max_frame_bytes, DEFAULT_MAX_FRAME_BYTES);
        assert_eq!(cfg.own_device_id, None);
        assert_eq!(cfg.rolling_group_id, None);
    }

    #[test]
    fn missing_account_is_hard_error() {
        let mut vars = base_vars();
        vars.remove(ENV_ACCOUNT);
        let err = SignalConfig::resolve_from(&vars).unwrap_err();
        assert_eq!(err, ConfigError::MissingRequired(ENV_ACCOUNT.to_string()));
    }

    #[test]
    fn missing_endpoint_is_hard_error() {
        let mut vars = base_vars();
        vars.remove(ENV_ENDPOINT);
        let err = SignalConfig::resolve_from(&vars).unwrap_err();
        assert_eq!(err, ConfigError::MissingRequired(ENV_ENDPOINT.to_string()));
    }

    #[test]
    fn missing_allowlist_var_is_hard_error() {
        // The allowlist var must be *present* (even if empty) so an operator
        // never accidentally runs with an unset => undefined trust set.
        let mut vars = base_vars();
        vars.remove(ENV_ALLOWLIST);
        let err = SignalConfig::resolve_from(&vars).unwrap_err();
        assert_eq!(err, ConfigError::MissingRequired(ENV_ALLOWLIST.to_string()));
    }

    #[test]
    fn empty_allowlist_is_fail_closed_not_error() {
        let mut vars = base_vars();
        vars.insert(ENV_ALLOWLIST.into(), "".into());
        let cfg = SignalConfig::resolve_from(&vars).expect("empty allowlist is valid");
        assert!(
            cfg.allowlist.is_empty(),
            "empty allowlist must parse to the empty (fail-closed) set"
        );
    }

    #[test]
    fn allowlist_parses_multiple_trimmed_entries() {
        let mut vars = base_vars();
        vars.insert(ENV_ALLOWLIST.into(), " +15551230001 , +15551230002 ".into());
        let cfg = SignalConfig::resolve_from(&vars).unwrap();
        assert_eq!(
            cfg.allowlist,
            vec!["+15551230001".to_string(), "+15551230002".to_string()]
        );
    }

    #[test]
    fn invalid_e164_account_is_rejected() {
        let mut vars = base_vars();
        vars.insert(ENV_ACCOUNT.into(), "5551230000".into()); // missing +
        let err = SignalConfig::resolve_from(&vars).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid { ref var, .. } if var == ENV_ACCOUNT),
            "got {err:?}"
        );
    }

    #[test]
    fn invalid_e164_in_allowlist_is_rejected() {
        let mut vars = base_vars();
        vars.insert(ENV_ALLOWLIST.into(), "+15551230001,not-a-number".into());
        let err = SignalConfig::resolve_from(&vars).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { .. }), "got {err:?}");
    }

    #[test]
    fn rolling_mode_without_group_id_is_error() {
        let mut vars = base_vars();
        vars.insert(ENV_GROUP_MODE.into(), "rolling".into());
        let err = SignalConfig::resolve_from(&vars).unwrap_err();
        assert_eq!(
            err,
            ConfigError::MissingRequired(ENV_ROLLING_GROUP_ID.to_string())
        );
    }

    #[test]
    fn rolling_mode_with_group_id_resolves() {
        let mut vars = base_vars();
        vars.insert(ENV_GROUP_MODE.into(), "rolling".into());
        vars.insert(ENV_ROLLING_GROUP_ID.into(), "GROUP_ABC==".into());
        let cfg = SignalConfig::resolve_from(&vars).unwrap();
        assert_eq!(cfg.group_mode, GroupMode::Rolling);
        assert_eq!(cfg.rolling_group_id.as_deref(), Some("GROUP_ABC=="));
    }

    #[test]
    fn unknown_group_mode_is_error() {
        let mut vars = base_vars();
        vars.insert(ENV_GROUP_MODE.into(), "banana".into());
        let err = SignalConfig::resolve_from(&vars).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid { ref var, .. } if var == ENV_GROUP_MODE),
            "got {err:?}"
        );
    }

    #[test]
    fn echo_ttl_parses_from_env() {
        let mut vars = base_vars();
        vars.insert(ENV_ECHO_TTL_SECS.into(), "90".into());
        let cfg = SignalConfig::resolve_from(&vars).unwrap();
        assert_eq!(cfg.echo_ttl, Duration::from_secs(90));
    }

    #[test]
    fn invalid_echo_ttl_is_error() {
        let mut vars = base_vars();
        vars.insert(ENV_ECHO_TTL_SECS.into(), "-5".into());
        let err = SignalConfig::resolve_from(&vars).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid { ref var, .. } if var == ENV_ECHO_TTL_SECS),
            "got {err:?}"
        );
    }

    #[test]
    fn max_frame_bytes_parses_from_env() {
        let mut vars = base_vars();
        vars.insert(ENV_MAX_FRAME_BYTES.into(), "2048".into());
        let cfg = SignalConfig::resolve_from(&vars).unwrap();
        assert_eq!(cfg.max_frame_bytes, 2048);
    }

    #[test]
    fn own_device_id_must_be_at_least_two() {
        let mut vars = base_vars();
        vars.insert(ENV_OWN_DEVICE_ID.into(), "1".into());
        let err = SignalConfig::resolve_from(&vars).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid { ref var, .. } if var == ENV_OWN_DEVICE_ID),
            "device id 1 is the primary phone and must not be the bot; got {err:?}"
        );
    }

    #[test]
    fn own_device_id_valid_when_ge_two() {
        let mut vars = base_vars();
        vars.insert(ENV_OWN_DEVICE_ID.into(), "3".into());
        let cfg = SignalConfig::resolve_from(&vars).unwrap();
        assert_eq!(cfg.own_device_id, Some(3));
    }

    #[test]
    fn e164_validator_accepts_and_rejects() {
        assert!(is_valid_e164("+15551230000"));
        assert!(!is_valid_e164("15551230000"));
        assert!(!is_valid_e164("+"));
        assert!(!is_valid_e164("+1555abc0000"));
    }
}
