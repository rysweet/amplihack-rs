//! The picker's settings from the environment (issue #2006).
//!
//! Every value falls back to its default when it is invalid, with one
//! warning that quotes the value with `{:?}`, so a control character in it
//! is never printed raw. An empty value counts as unset and gives no warning.

use super::Cli;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

pub const JOB_COST_ENV: &str = "AMPLIHACK_ACCOUNT_JOB_COST_PCT";
pub const LEASE_TTL_ENV: &str = "AMPLIHACK_ACCOUNT_LEASE_TTL_SECS";
pub const LEASE_DIR_ENV: &str = "AMPLIHACK_ACCOUNT_LEASE_DIR";
pub const SWITCH_ENV: &str = "AMPLIHACK_ACCOUNT_PICK";

/// Headroom points subtracted per running job or lease.
pub const DEFAULT_JOB_COST_PCT: f64 = 15.0;
/// Lease expiry, in seconds.
pub const DEFAULT_LEASE_TTL_SECS: u64 = 120;
const MAX_LEASE_TTL_SECS: u64 = 86_400;
/// The lease directory under the user's home.
const DEFAULT_LEASE_SUBDIR: &str = ".amplihack/state/account-leases";

/// Whether launch paths pick an account (`AMPLIHACK_ACCOUNT_PICK`). The
/// standalone `amplihack account pick` command ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    /// Pick for Codex and Claude launches (the default).
    On,
    /// Never pick; every launch keeps its old behaviour.
    Off,
    /// Pick for Codex launches only; Claude launches keep `~/.claude`.
    Codex,
}

impl Switch {
    pub fn allows(self, cli: Cli) -> bool {
        match self {
            Self::On => true,
            Self::Off => false,
            Self::Codex => cli == Cli::Codex,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PickConfig {
    pub job_cost_pct: f64,
    pub lease_ttl_secs: u64,
    pub lease_dir: PathBuf,
    pub switch: Switch,
}

impl PickConfig {
    /// The settings from the process environment and `$HOME`, with the
    /// warnings to print.
    pub fn from_env() -> (Self, Vec<String>) {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from);
        Self::from_reader(|k| std::env::var_os(k), home.as_deref())
    }

    /// The settings from `read` (one variable at a time) and the user's
    /// `home`, with one warning per invalid value.
    pub fn from_reader(
        read: impl Fn(&str) -> Option<OsString>,
        home: Option<&Path>,
    ) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let get = |k: &str| read(k).filter(|v| !v.is_empty());

        let job_cost_pct = setting(
            get(JOB_COST_ENV),
            parse_cost,
            DEFAULT_JOB_COST_PCT,
            JOB_COST_ENV,
            "a number above 0 and at most 100",
            &DEFAULT_JOB_COST_PCT.to_string(),
            &mut warnings,
        );
        let lease_ttl_secs = setting(
            get(LEASE_TTL_ENV),
            parse_ttl,
            DEFAULT_LEASE_TTL_SECS,
            LEASE_TTL_ENV,
            &format!("a whole number from 1 to {MAX_LEASE_TTL_SECS}"),
            &DEFAULT_LEASE_TTL_SECS.to_string(),
            &mut warnings,
        );
        let default_dir = home
            .map(|h| h.join(DEFAULT_LEASE_SUBDIR))
            .unwrap_or_else(|| std::env::temp_dir().join("amplihack-account-leases"));
        let shown_dir = default_dir.display().to_string();
        let lease_dir = setting(
            get(LEASE_DIR_ENV),
            |raw| Some(PathBuf::from(raw)).filter(|p| p.is_absolute()),
            default_dir,
            LEASE_DIR_ENV,
            "an absolute path",
            &shown_dir,
            &mut warnings,
        );
        let switch = setting(
            get(SWITCH_ENV),
            |raw| match raw.to_string_lossy().to_ascii_lowercase().as_str() {
                "on" => Some(Switch::On),
                "off" => Some(Switch::Off),
                "codex" => Some(Switch::Codex),
                _ => None,
            },
            Switch::On,
            SWITCH_ENV,
            "one of on, off or codex",
            "on",
            &mut warnings,
        );

        (
            Self {
                job_cost_pct,
                lease_ttl_secs,
                lease_dir,
                switch,
            },
            warnings,
        )
    }
}

/// `raw` parsed by `parse`, or `default` when it is unset; an invalid value
/// also gives `default`, with a warning that quotes it.
fn setting<T>(
    raw: Option<OsString>,
    parse: impl Fn(&OsStr) -> Option<T>,
    default: T,
    key: &str,
    expected: &str,
    shown_default: &str,
    warnings: &mut Vec<String>,
) -> T {
    let Some(raw) = raw else { return default };
    parse(&raw).unwrap_or_else(|| {
        warnings.push(format!(
            "warning: {key}={:?} is not {expected}; using {shown_default}",
            raw.to_string_lossy()
        ));
        default
    })
}

/// A finite number above 0 and at most 100, written without spaces.
fn parse_cost(raw: &OsStr) -> Option<f64> {
    let s = raw.to_str()?;
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    s.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= 100.0)
}

/// A whole number from 1 to 86400.
fn parse_ttl(raw: &OsStr) -> Option<u64> {
    let s = raw.to_str()?;
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u64>()
        .ok()
        .filter(|v| (1..=MAX_LEASE_TTL_SECS).contains(v))
}
