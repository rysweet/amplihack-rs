//! Env-first configuration loader for the Signal channel.
//!
//! Resolution order for every setting: **environment variable > TOML file
//! (`AMPLIHACK_SIGNAL_CONFIG`) > explicit error**. There are **no silent
//! defaults** for required settings; a missing required value is a hard error
//! and the channel stays off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

mod resolver;

/// Environment variable names (single source of truth for env + tests).
pub const ENV_ENDPOINT: &str = "AMPLIHACK_SIGNAL_ENDPOINT";
pub const ENV_ACCOUNT: &str = "AMPLIHACK_SIGNAL_ACCOUNT";
pub const ENV_ALLOWLIST: &str = "AMPLIHACK_SIGNAL_ALLOWLIST";
pub const ENV_OWN_DEVICE_ID: &str = "AMPLIHACK_SIGNAL_OWN_DEVICE_ID";
pub const ENV_REUSE_ROLLING_GROUP: &str = "AMPLIHACK_SIGNAL_REUSE_ROLLING_GROUP";
pub const ENV_ROLLING_GROUP_ID: &str = "AMPLIHACK_SIGNAL_ROLLING_GROUP_ID";
pub const ENV_CONFIG_PATH: &str = "AMPLIHACK_SIGNAL_CONFIG";

/// Diagnostic label for `own_device_id` when its value came from the TOML file
/// rather than the environment variable. Used so an `InvalidNumber` error names
/// the config key the operator actually edited instead of always blaming the
/// env var.
pub(crate) const TOML_OWN_DEVICE_ID: &str = "own_device_id";

/// Errors from configuration resolution.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required setting was absent from **both** env and TOML.
    #[error("missing required Signal setting: {0}")]
    MissingRequired(&'static str),
    /// A phone number was not valid E.164 (`+` then 1..=15 digits).
    #[error("invalid E.164 number")]
    InvalidE164(String),
    /// The endpoint was not a valid `host:port`.
    #[error("invalid endpoint (want host:port): {0}")]
    InvalidEndpoint(String),
    /// A numeric setting failed to parse.
    #[error("invalid numeric setting {key}: {value}")]
    InvalidNumber { key: &'static str, value: String },
    /// A boolean setting failed to parse.
    #[error("invalid boolean setting {key}: {value}")]
    InvalidBool { key: &'static str, value: String },
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
        let env: HashMap<String, String> = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
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

        let endpoint = resolver::get_str(env, toml_table, ENV_ENDPOINT, "endpoint")?
            .ok_or(ConfigError::MissingRequired("endpoint"))?;
        resolver::validate_endpoint(&endpoint)?;

        let account = resolver::get_str(env, toml_table, ENV_ACCOUNT, "account")?
            .ok_or(ConfigError::MissingRequired("account"))?;
        resolver::validate_e164(&account)?;

        let allowlist = resolver::resolve_allowlist(env, toml_table)?;
        let own_device_id = resolver::resolve_own_device_id(env, toml_table)?;
        let reuse_rolling_group = resolver::resolve_reuse_rolling_group(env, toml_table)?;
        let rolling_group_id =
            resolver::resolve_rolling_group_id(env, toml_table, reuse_rolling_group)?;
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
    match std::fs::read_to_string(default_file) {
        Ok(contents) => Ok(Some(contents)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Io {
            path: default_file.display().to_string(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests;
