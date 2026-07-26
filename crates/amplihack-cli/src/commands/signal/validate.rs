//! Boundary validation for `amplihack signal` (#921/#923).
//!
//! The fleet path fans commands out over
//! `azlin connect <vm> --resource-group <rg> ... -- '<cmd>'`, so VM and
//! resource-group names are an **injection surface**. Every function here is
//! **validate-and-reject**: a value is accepted verbatim or rejected with an
//! error — never silently sanitized into a *different* valid target.
//!
//! Loopback enforcement guarantees the signal-cli JSON-RPC daemon binds only
//! `127.0.0.0/8` / `::1` / `localhost`; a wildcard or routable bind is refused
//! so the daemon port is never exposed off-host.

use anyhow::{Result, bail};

/// Validate a VM name: first char ASCII-alphanumeric, remaining chars
/// alphanumeric / `_` / `-`, length 1..=64. Mirrors the fleet module's rule so
/// the two validators cannot drift.
pub fn validate_vm_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("invalid VM name: empty");
    };
    if !first.is_ascii_alphanumeric() {
        bail!("invalid VM name: {name:?} (must start alphanumeric)");
    }
    if name.len() > 64 || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        bail!("invalid VM name: {name:?} (allowed: alphanumeric, '_', '-', max 64)");
    }
    Ok(())
}

/// Validate an Azure resource-group name. Azure permits alphanumerics, `_`,
/// `-`, `.`, and parentheses; we apply the same strict character allowlist we
/// use for VM names plus `.`, and reject anything else (spaces, shell
/// metacharacters, empty).
pub fn validate_resource_group(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("invalid resource group: empty");
    };
    if !first.is_ascii_alphanumeric() {
        bail!("invalid resource group: {name:?} (must start alphanumeric)");
    }
    if name.len() > 90 || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        bail!("invalid resource group: {name:?} (allowed: alphanumeric, '_', '-', '.')");
    }
    Ok(())
}

/// Validate an E.164 phone number: `+` followed by 1..=15 ASCII digits.
/// Matches `amplihack_signal::config`'s rule exactly so the writer never emits
/// a config the real loader would reject.
pub fn validate_account(account: &str) -> Result<()> {
    let ok = account.starts_with('+') && {
        let digits = &account[1..];
        !digits.is_empty() && digits.len() <= 15 && digits.bytes().all(|b| b.is_ascii_digit())
    };
    if ok {
        Ok(())
    } else {
        bail!("invalid account (want E.164, e.g. +12065551234): {account:?}");
    }
}

/// Validate a Signal linked-device name: printable, non-empty, no control
/// characters or shell metacharacters, length 1..=64. Passed to
/// `signal-cli link -n <name>`.
pub fn validate_device_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        bail!("invalid device name (length 1..=64): {name:?}");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        bail!("invalid device name (allowed: alphanumeric, '_', '-', '.'): {name:?}");
    }
    Ok(())
}

/// Validate a `host:port` endpoint is **loopback-only** and well-formed.
///
/// Accepts `127.0.0.0/8`, `::1` (bracketed `[::1]:port` or bracket-less
/// `::1:port`), and the literal `localhost`, with a port in `1..=65535`.
/// Rejects wildcard (`0.0.0.0`, `::`), routable addresses, DNS names, and any
/// malformed form (missing/zero/out-of-range port).
///
/// This delegates to the single canonical validator in `amplihack-signal`
/// ([`amplihack_signal::bridge::validate_loopback_endpoint`]) so the CLI and
/// runtime loopback checks can never drift.
pub fn validate_loopback_endpoint(endpoint: &str) -> Result<()> {
    amplihack_signal::bridge::validate_loopback_endpoint(endpoint).map_err(|_| {
        anyhow::anyhow!(
            "endpoint must be loopback host:port (127.0.0.0/8, ::1, localhost) with port 1..=65535: {endpoint:?}"
        )
    })
}
