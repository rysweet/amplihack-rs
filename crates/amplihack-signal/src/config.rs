//! Env-first configuration loader for the Signal channel.
//!
//! Resolution order for every setting: **environment variable > TOML file
//! (`AMPLIHACK_SIGNAL_CONFIG`) > explicit error**. There are **no silent
//! defaults** for required settings; a missing required value is a hard error
//! and the channel stays off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Environment variable names (single source of truth for env + tests).
pub const ENV_ENDPOINT: &str = "AMPLIHACK_SIGNAL_ENDPOINT";
pub const ENV_ACCOUNT: &str = "AMPLIHACK_SIGNAL_ACCOUNT";
pub const ENV_ALLOWLIST: &str = "AMPLIHACK_SIGNAL_ALLOWLIST";
pub const ENV_OWN_DEVICE_ID: &str = "AMPLIHACK_SIGNAL_OWN_DEVICE_ID";
pub const ENV_REUSE_ROLLING_GROUP: &str = "AMPLIHACK_SIGNAL_REUSE_ROLLING_GROUP";
pub const ENV_ROLLING_GROUP_ID: &str = "AMPLIHACK_SIGNAL_ROLLING_GROUP_ID";
pub const ENV_CONFIG_PATH: &str = "AMPLIHACK_SIGNAL_CONFIG";

/// Errors from configuration resolution.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required setting was absent from **both** env and TOML.
    #[error("missing required Signal setting: {0}")]
    MissingRequired(&'static str),
    /// A phone number was not valid E.164 (`+` then 1..=15 digits).
    #[error("invalid E.164 number: {0}")]
    InvalidE164(String),
    /// The endpoint was not a valid `host:port`.
    #[error("invalid endpoint (want host:port): {0}")]
    InvalidEndpoint(String),
    /// A numeric setting failed to parse.
    #[error("invalid numeric setting {key}: {value}")]
    InvalidNumber { key: &'static str, value: String },
    /// The TOML config file could not be parsed.
    #[error("TOML parse error: {0}")]
    Toml(String),
    /// The TOML config file could not be read.
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

/// Fully-resolved Signal channel configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalConfig {
    /// `host:port` of the signal-cli JSON-RPC daemon.
    pub endpoint: String,
    /// E.164 account amplihack sends **as**.
    pub account: String,
    /// Permitted E.164 inbound senders. **Empty ⇒ fail-closed (deny all).**
    pub allowlist: Vec<String>,
    /// signal-cli's OWN linked-device id (must be `>= 2`) if configured, used to
    /// reject the account's own synced-back messages as echoes. Optional: the
    /// primary-phone (device `1`) gate is the main loop guard and needs no
    /// configuration.
    pub own_device_id: Option<u32>,
    /// Reuse one rolling group across sessions instead of per-session groups.
    pub reuse_rolling_group: bool,
    /// Bind rolling mode to an existing group id.
    pub rolling_group_id: Option<String>,
}

impl SignalConfig {
    /// Load configuration from the process environment and a resolved TOML file.
    ///
    /// Reads `std::env`, resolves the TOML source via
    /// [`resolve_config_source`] (explicit `AMPLIHACK_SIGNAL_CONFIG` file, then
    /// the default `~/.amplihack/signal-config.toml` written by
    /// `amplihack signal setup`), then delegates to [`SignalConfig::from_sources`].
    ///
    /// The default-path fallback is what makes onboarding zero-step: once
    /// `amplihack signal setup` has written the default config, the existing
    /// per-session SessionStart integration picks it up with no further wiring.
    pub fn load() -> Result<Self, ConfigError> {
        let env: HashMap<String, String> = std::env::vars().collect();
        let default_file = default_config_path_in(&home_dir());
        let toml_str = resolve_config_source(&env, &default_file)?;
        Self::from_sources(&env, toml_str.as_deref())
    }

    /// Pure resolver over explicit sources (no process env / file I/O).
    ///
    /// This is the unit-testable seam: `env` is an already-materialized map and
    /// `toml_str` is the already-read file contents (if any). Enforces
    /// `env > TOML > error`, validates E.164 and endpoint, and treats an
    /// absent (not merely empty) allowlist as [`ConfigError::MissingRequired`].
    pub fn from_sources(
        env: &HashMap<String, String>,
        toml_str: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let toml_val: Option<toml::Value> = match toml_str {
            Some(s) => Some(
                s.parse::<toml::Value>()
                    .map_err(|e| ConfigError::Toml(e.to_string()))?,
            ),
            None => None,
        };
        let toml_table = toml_val.as_ref().and_then(toml::Value::as_table);

        // env > TOML for a string-valued setting.
        let get_str = |env_key: &str, toml_key: &str| -> Option<String> {
            if let Some(v) = env.get(env_key) {
                return Some(v.clone());
            }
            toml_table
                .and_then(|t| t.get(toml_key))
                .and_then(toml::Value::as_str)
                .map(str::to_string)
        };
        let is_present = |env_key: &str, toml_key: &str| -> bool {
            env.contains_key(env_key) || toml_table.is_some_and(|t| t.contains_key(toml_key))
        };

        let endpoint =
            get_str(ENV_ENDPOINT, "endpoint").ok_or(ConfigError::MissingRequired("endpoint"))?;
        validate_endpoint(&endpoint)?;

        let account =
            get_str(ENV_ACCOUNT, "account").ok_or(ConfigError::MissingRequired("account"))?;
        validate_e164(&account)?;

        // The allowlist key MUST be present (absence is a hard error; an
        // explicitly-empty allowlist is a valid, deliberate fail-closed config).
        if !is_present(ENV_ALLOWLIST, "allowlist") {
            return Err(ConfigError::MissingRequired("allowlist"));
        }
        let allowlist: Vec<String> = if let Some(csv) = env.get(ENV_ALLOWLIST) {
            parse_allowlist_csv(csv)?
        } else {
            let mut out = Vec::new();
            if let Some(arr) = toml_table
                .and_then(|t| t.get("allowlist"))
                .and_then(toml::Value::as_array)
            {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        let s = s.trim();
                        if s.is_empty() {
                            continue;
                        }
                        validate_e164(s)?;
                        out.push(s.to_string());
                    }
                }
            }
            out
        };

        let own_device_id = match env.get(ENV_OWN_DEVICE_ID) {
            Some(v) => Some(
                v.trim()
                    .parse::<u32>()
                    .map_err(|_| ConfigError::InvalidNumber {
                        key: ENV_OWN_DEVICE_ID,
                        value: v.clone(),
                    })?,
            ),
            None => toml_table
                .and_then(|t| t.get("own_device_id"))
                .and_then(toml::Value::as_integer)
                .map(|i| i as u32),
        };

        // signal-cli's own linked-device id, when configured, must be a real
        // linked device (`>= 2`). Device `1` is the operator's primary phone and
        // must never be treated as the bot's own echo source.
        match own_device_id {
            Some(d) if d < 2 => {
                return Err(ConfigError::InvalidNumber {
                    key: ENV_OWN_DEVICE_ID,
                    value: d.to_string(),
                });
            }
            _ => {}
        }

        let reuse_rolling_group = match env.get(ENV_REUSE_ROLLING_GROUP) {
            Some(v) => is_truthy(v),
            None => toml_table
                .and_then(|t| t.get("reuse_rolling_group"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(false),
        };

        let rolling_group_id = get_str(ENV_ROLLING_GROUP_ID, "rolling_group_id");

        Ok(SignalConfig {
            endpoint,
            account,
            allowlist,
            own_device_id,
            reuse_rolling_group,
            rolling_group_id,
        })
    }
}

/// The default on-disk config path, relative to a home directory:
/// `<home>/.amplihack/signal-config.toml`. This is exactly where
/// `amplihack signal setup` writes its output, and where [`SignalConfig::load`]
/// looks when `AMPLIHACK_SIGNAL_CONFIG` is unset.
pub fn default_config_path_in(home: &Path) -> PathBuf {
    home.join(".amplihack").join("signal-config.toml")
}

/// Best-effort home directory for the default config path. Falls back to `.`
/// when `HOME` is unset; a non-existent default file resolves to `None`
/// (channel disabled) rather than an error, so the fallback is harmless.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve the TOML config *source* (file contents) honoring the precedence
/// `AMPLIHACK_SIGNAL_CONFIG` file, then the default
/// `~/.amplihack/signal-config.toml`, then none. Environment-variable *setting*
/// overrides still apply later in [`SignalConfig::from_sources`]; this only
/// decides which file (if any) backs the TOML layer.
///
/// * An explicit `AMPLIHACK_SIGNAL_CONFIG` that cannot be read is a hard error
///   (no silent fallback to the default path — the operator asked for a
///   specific file).
/// * A missing default file is **not** an error: it yields `Ok(None)`, meaning
///   "no TOML layer", so an unconfigured host simply leaves the channel off.
pub fn resolve_config_source(
    env: &HashMap<String, String>,
    default_file: &Path,
) -> Result<Option<String>, ConfigError> {
    if let Some(path) = env.get(ENV_CONFIG_PATH) {
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        return Ok(Some(contents));
    }
    if default_file.exists() {
        let contents = std::fs::read_to_string(default_file).map_err(|source| ConfigError::Io {
            path: default_file.display().to_string(),
            source,
        })?;
        return Ok(Some(contents));
    }
    Ok(None)
}

/// Parse a comma-separated allowlist, trimming and dropping empty entries and
/// validating every surviving entry as E.164.
fn parse_allowlist_csv(csv: &str) -> Result<Vec<String>, ConfigError> {
    let mut out = Vec::new();
    for part in csv.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        validate_e164(part)?;
        out.push(part.to_string());
    }
    Ok(out)
}

/// Validate an E.164 phone number: `+` followed by 1..=15 ASCII digits.
fn validate_e164(s: &str) -> Result<(), ConfigError> {
    let ok = s.starts_with('+') && {
        let digits = &s[1..];
        !digits.is_empty() && digits.len() <= 15 && digits.bytes().all(|b| b.is_ascii_digit())
    };
    if ok {
        Ok(())
    } else {
        Err(ConfigError::InvalidE164(s.to_string()))
    }
}

/// Validate a `host:port` endpoint: non-empty host and a parseable `u16` port.
fn validate_endpoint(s: &str) -> Result<(), ConfigError> {
    if let Some((host, port)) = s.rsplit_once(':')
        && !host.is_empty()
        && port.parse::<u16>().is_ok()
    {
        return Ok(());
    }
    Err(ConfigError::InvalidEndpoint(s.to_string()))
}

/// Interpret common truthy string values (`1`, `true`, `yes`, `on`).
fn is_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn env_only_minimal_valid_config() {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001,+15551230002"),
        ]);
        let cfg = SignalConfig::from_sources(&e, None).expect("valid config");
        assert_eq!(cfg.endpoint, "127.0.0.1:7583");
        assert_eq!(cfg.account, "+15551230000");
        assert_eq!(cfg.allowlist, vec!["+15551230001", "+15551230002"]);
        assert_eq!(cfg.own_device_id, None);
        assert!(!cfg.reuse_rolling_group);
        assert_eq!(cfg.rolling_group_id, None);
    }

    #[test]
    fn own_device_id_parsed_from_env() {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001"),
            (ENV_OWN_DEVICE_ID, "3"),
        ]);
        let cfg = SignalConfig::from_sources(&e, None).unwrap();
        assert_eq!(cfg.own_device_id, Some(3));
    }

    #[test]
    fn own_device_id_below_two_is_error() {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001"),
            (ENV_OWN_DEVICE_ID, "1"),
        ]);
        let err = SignalConfig::from_sources(&e, None).unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidNumber { key, .. } if key == ENV_OWN_DEVICE_ID),
            "expected InvalidNumber for own_device_id, got {err:?}"
        );
    }

    #[test]
    fn missing_endpoint_is_error() {
        let e = env(&[
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001"),
        ]);
        let err = SignalConfig::from_sources(&e, None).unwrap_err();
        assert!(matches!(err, ConfigError::MissingRequired("endpoint")));
    }

    #[test]
    fn missing_account_is_error() {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ALLOWLIST, "+15551230001"),
        ]);
        let err = SignalConfig::from_sources(&e, None).unwrap_err();
        assert!(matches!(err, ConfigError::MissingRequired("account")));
    }

    #[test]
    fn absent_allowlist_is_error_no_silent_default() {
        // The allowlist key is required to be *present*. Absence must error —
        // never silently default to "allow everyone".
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "+15551230000"),
        ]);
        let err = SignalConfig::from_sources(&e, None).unwrap_err();
        assert!(matches!(err, ConfigError::MissingRequired("allowlist")));
    }

    #[test]
    fn present_but_empty_allowlist_is_valid_fail_closed() {
        // Explicitly empty is a *valid* deliberate config: "accept no inbound".
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, ""),
        ]);
        let cfg = SignalConfig::from_sources(&e, None).expect("empty allowlist is valid");
        assert!(cfg.allowlist.is_empty());
    }

    #[test]
    fn invalid_e164_account_is_error() {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "5551230000"), // missing '+'
            (ENV_ALLOWLIST, "+15551230001"),
        ]);
        let err = SignalConfig::from_sources(&e, None).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidE164(_)));
    }

    #[test]
    fn invalid_e164_in_allowlist_is_error() {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001,not-a-number"),
        ]);
        let err = SignalConfig::from_sources(&e, None).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidE164(_)));
    }

    #[test]
    fn invalid_endpoint_is_error() {
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1"), // no port
            (ENV_ACCOUNT, "+15551230000"),
            (ENV_ALLOWLIST, "+15551230001"),
        ]);
        let err = SignalConfig::from_sources(&e, None).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidEndpoint(_)));
    }

    #[test]
    fn toml_supplies_values_when_env_absent() {
        let toml = r#"
            endpoint = "10.0.0.5:7583"
            account  = "+15551230000"
            allowlist = ["+15551230001", "+15551230002"]
            own_device_id = 2
            reuse_rolling_group = true
            rolling_group_id = "grp-rolling=="
        "#;
        let cfg = SignalConfig::from_sources(&HashMap::new(), Some(toml)).expect("valid toml");
        assert_eq!(cfg.endpoint, "10.0.0.5:7583");
        assert_eq!(cfg.account, "+15551230000");
        assert_eq!(cfg.allowlist, vec!["+15551230001", "+15551230002"]);
        assert_eq!(cfg.own_device_id, Some(2));
        assert!(cfg.reuse_rolling_group);
        assert_eq!(cfg.rolling_group_id.as_deref(), Some("grp-rolling=="));
    }

    #[test]
    fn env_overrides_toml_per_setting() {
        let toml = r#"
            endpoint = "10.0.0.5:7583"
            account  = "+15550000000"
            allowlist = ["+15550000001"]
        "#;
        let e = env(&[
            (ENV_ENDPOINT, "127.0.0.1:7583"),
            (ENV_ACCOUNT, "+15551230000"),
        ]);
        let cfg = SignalConfig::from_sources(&e, Some(toml)).unwrap();
        // env wins for endpoint + account; allowlist falls back to TOML.
        assert_eq!(cfg.endpoint, "127.0.0.1:7583");
        assert_eq!(cfg.account, "+15551230000");
        assert_eq!(cfg.allowlist, vec!["+15550000001"]);
    }

    #[test]
    fn reuse_rolling_group_truthy_env_values() {
        for v in ["1", "true"] {
            let e = env(&[
                (ENV_ENDPOINT, "127.0.0.1:7583"),
                (ENV_ACCOUNT, "+15551230000"),
                (ENV_ALLOWLIST, "+15551230001"),
                (ENV_REUSE_ROLLING_GROUP, v),
                (ENV_ROLLING_GROUP_ID, "grp-rolling=="),
            ]);
            let cfg = SignalConfig::from_sources(&e, None).unwrap();
            assert!(cfg.reuse_rolling_group, "value {v:?} should be truthy");
            assert_eq!(cfg.rolling_group_id.as_deref(), Some("grp-rolling=="));
        }
    }

    #[test]
    fn malformed_toml_is_error() {
        let err = SignalConfig::from_sources(&HashMap::new(), Some("this = = broken")).unwrap_err();
        assert!(matches!(err, ConfigError::Toml(_)));
    }
}
