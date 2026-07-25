use std::collections::HashMap;

use super::*;

type TomlTable<'a> = Option<&'a toml::value::Table>;

/// Normalize a raw string setting: trim surrounding whitespace and map a blank
/// result to `None`.
///
/// File-based secrets and Kubernetes `envFrom` injection routinely append a
/// trailing newline, so an operator's `127.0.0.1:7583` can arrive as
/// `"127.0.0.1:7583\n"`; trimming keeps such values usable. A value that is
/// *only* whitespace is treated as unset (`None`) rather than a malformed
/// setting, so it surfaces as a clear `MissingRequired` error instead of a
/// confusing "invalid endpoint" / "invalid E.164" for an effectively-blank
/// value. Callers decide whether a blank environment value should fall through
/// to TOML or stop resolution as an explicitly-blank value.
pub(super) fn normalize(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Resolve `env > TOML` for a string-valued setting, applying [`normalize`] to
/// whichever source supplies the value.
pub(super) fn get_str(
    env: &HashMap<String, String>,
    toml_table: TomlTable<'_>,
    env_key: &str,
    toml_key: &'static str,
) -> Result<Option<String>, ConfigError> {
    if let Some(v) = env.get(env_key) {
        return Ok(normalize(v));
    }
    match toml_table.and_then(|t| t.get(toml_key)) {
        None => Ok(None),
        Some(toml::Value::String(s)) => Ok(normalize(s)),
        Some(other) => Err(ConfigError::Toml(format!(
            "invalid setting {toml_key}: expected string, got {other}"
        ))),
    }
}

/// Resolve the inbound allowlist honoring `env > TOML`. A present
/// `AMPLIHACK_SIGNAL_ALLOWLIST` env var wins outright (including an
/// intentionally empty value, which means deliberate deny-all); otherwise the
/// TOML `allowlist` array is required.
///
/// Precedence note: because env *presence* wins, a set-but-blank env value
/// collapses to deny-all and shadows any populated TOML allowlist. That fails
/// closed (safe), but is easy to trigger accidentally (e.g. `envFrom` injecting
/// an empty var), so it emits a `warn` when it shadows a non-empty TOML
/// allowlist. Unset the env var to fall through to TOML.
pub(super) fn resolve_allowlist(
    env: &HashMap<String, String>,
    toml_table: TomlTable<'_>,
) -> Result<Vec<String>, ConfigError> {
    if let Some(csv) = env.get(ENV_ALLOWLIST) {
        let list = parse_allowlist_csv(csv)?;
        if list.is_empty() && toml_has_nonempty_allowlist(toml_table) {
            tracing::warn!(
                "empty {ENV_ALLOWLIST} shadows a populated TOML allowlist; all \
                 inbound Signal senders will be denied (fail-closed). Unset \
                 {ENV_ALLOWLIST} to use the TOML allowlist."
            );
        }
        return Ok(list);
    }
    let raw = toml_table
        .and_then(|t| t.get("allowlist"))
        .ok_or(ConfigError::MissingRequired("allowlist"))?;
    let arr = raw.as_array().ok_or_else(|| {
        ConfigError::Toml(format!("invalid allowlist: expected array, got {raw}"))
    })?;
    let mut out = Vec::new();
    for item in arr {
        let s = item.as_str().ok_or_else(|| {
            ConfigError::Toml(format!(
                "invalid allowlist entry: expected string, got {item}"
            ))
        })?;
        let s = s.trim();
        if !s.is_empty() {
            validate_e164(s)?;
            out.push(s.to_string());
        }
    }
    Ok(out)
}

/// True when the TOML `allowlist` is an array containing at least one non-blank
/// string entry. Used only for the shadowing `warn` in [`resolve_allowlist`];
/// full validation still happens on the TOML fall-through path.
fn toml_has_nonempty_allowlist(toml_table: TomlTable<'_>) -> bool {
    toml_table
        .and_then(|t| t.get("allowlist"))
        .and_then(|v| v.as_array())
        .is_some_and(|arr| {
            arr.iter()
                .any(|item| item.as_str().is_some_and(|s| !s.trim().is_empty()))
        })
}

/// Resolve `own_device_id` honoring `env > TOML`. Accepts a TOML integer or
/// numeric string, requires the value to be `>= 2`, and reports the source the
/// operator actually edited on error.
pub(super) fn resolve_own_device_id(
    env: &HashMap<String, String>,
    toml_table: TomlTable<'_>,
) -> Result<Option<u32>, ConfigError> {
    // Track which source supplied the value so any error names the setting the
    // operator actually edited (the env var vs the TOML key) rather than always
    // blaming the env var. An empty/whitespace-only env value is treated as
    // unset (via `normalize`) so it falls through to TOML instead of hard-
    // failing as an unparseable number.
    let (source_key, raw) = if let Some(raw) = env.get(ENV_OWN_DEVICE_ID).and_then(|v| normalize(v))
    {
        (ENV_OWN_DEVICE_ID, raw)
    } else {
        match toml_table.and_then(|t| t.get("own_device_id")) {
            None => return Ok(None),
            Some(toml::Value::Integer(value)) => (TOML_OWN_DEVICE_ID, value.to_string()),
            Some(toml::Value::String(value)) => (TOML_OWN_DEVICE_ID, value.clone()),
            Some(other) => {
                return Err(ConfigError::Toml(format!(
                    "invalid setting own_device_id: expected integer, got {other}"
                )));
            }
        }
    };
    let value = raw
        .trim()
        .parse_ascii_u32()
        .map_err(|_| ConfigError::InvalidNumber {
            key: source_key,
            value: raw.clone(),
        })?;
    if value < 2 {
        return Err(ConfigError::InvalidNumber {
            key: source_key,
            value: value.to_string(),
        });
    }
    Ok(Some(value))
}

trait ParseAsciiDigits {
    fn parse_ascii_u32(&self) -> Result<u32, ()>;
    fn parse_ascii_u16(&self) -> Result<u16, ()>;
}

impl ParseAsciiDigits for str {
    fn parse_ascii_u32(&self) -> Result<u32, ()> {
        parse_ascii_digits(self, str::parse::<u32>)
    }

    fn parse_ascii_u16(&self) -> Result<u16, ()> {
        parse_ascii_digits(self, str::parse::<u16>)
    }
}

fn parse_ascii_digits<T>(
    s: &str,
    parse: impl FnOnce(&str) -> Result<T, std::num::ParseIntError>,
) -> Result<T, ()> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    parse(s).map_err(|_| ())
}

/// Resolve `reuse_rolling_group` honoring `env > TOML`, defaulting to `false`
/// (per-session groups) when neither source sets it. Env values are parsed via
/// [`parse_bool_env`]; the TOML value must be a native boolean.
pub(super) fn resolve_reuse_rolling_group(
    env: &HashMap<String, String>,
    toml_table: TomlTable<'_>,
) -> Result<bool, ConfigError> {
    if let Some(v) = env.get(ENV_REUSE_ROLLING_GROUP) {
        return parse_bool_env(ENV_REUSE_ROLLING_GROUP, v);
    }
    match toml_table.and_then(|t| t.get("reuse_rolling_group")) {
        Some(v) => v.as_bool().ok_or_else(|| {
            ConfigError::Toml(format!("invalid boolean setting reuse_rolling_group: {v}"))
        }),
        None => Ok(false),
    }
}

pub(super) fn resolve_rolling_group_id(
    env: &HashMap<String, String>,
    toml_table: TomlTable<'_>,
    reuse_rolling_group: bool,
) -> Result<Option<String>, ConfigError> {
    // `get_str` already trims and maps blank to `None`, so a whitespace-only id
    // resolves to `None` here and correctly fails the reuse precondition below.
    let id = get_str(env, toml_table, ENV_ROLLING_GROUP_ID, "rolling_group_id")?;
    if reuse_rolling_group && id.is_none() {
        return Err(ConfigError::MissingRequired("rolling_group_id"));
    }
    Ok(id)
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
pub(super) fn validate_e164(s: &str) -> Result<(), ConfigError> {
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

/// Validate a `host:port` endpoint: non-empty host and non-zero `u16` port.
/// IPv6 literals must use the standard bracket form (`[::1]:7583`) so the host
/// boundary is unambiguous.
pub(super) fn validate_endpoint(s: &str) -> Result<(), ConfigError> {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (host, rest) = rest
            .split_once("]:")
            .ok_or_else(|| ConfigError::InvalidEndpoint(s.to_string()))?;
        (host, rest)
    } else if let Some((host, port)) = s.rsplit_once(':') {
        if host.contains(':') {
            return Err(ConfigError::InvalidEndpoint(s.to_string()));
        }
        (host, port)
    } else {
        return Err(ConfigError::InvalidEndpoint(s.to_string()));
    };
    if !host.is_empty() && port.parse_ascii_u16().is_ok_and(|p| p != 0) {
        return Ok(());
    }
    Err(ConfigError::InvalidEndpoint(s.to_string()))
}

/// Parse common boolean env tokens. Empty is retained as a safe explicit false
/// for the isolation default; unknown non-empty tokens are configuration errors.
fn parse_bool_env(key: &'static str, v: &str) -> Result<bool, ConfigError> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" | "" => Ok(false),
        _ => Err(ConfigError::InvalidBool {
            key,
            value: v.to_string(),
        }),
    }
}
