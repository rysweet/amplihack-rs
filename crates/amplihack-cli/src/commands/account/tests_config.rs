//! Issue #2006: the picker's settings from the environment.
//!
//! Wired from `account/mod.rs` as `#[cfg(test)] mod tests_config;`.
//!
//! Contract (docs/reference/account-pick.md, "Environment variables"):
//!
//! - `config::PickConfig { job_cost_pct: f64, lease_ttl_secs: u64,
//!   lease_dir: PathBuf, switch: Switch }`.
//! - `config::PickConfig::from_reader(read, home) -> (PickConfig, Vec<String>)`
//!   where `read: impl Fn(&str) -> Option<OsString>` looks up one variable
//!   and `home: Option<&Path>` is the user's home. The `Vec` holds one
//!   warning per invalid value, each quoting the value with `{:?}`.
//! - `config::Switch::{On, Off, Codex}` with `allows(Cli) -> bool`.
//! - `config::{DEFAULT_JOB_COST_PCT (15.0), DEFAULT_LEASE_TTL_SECS (120)}`.
//!
//! The reader is injected, so no test changes the process environment.

use super::Cli;
use super::config::{self, PickConfig, Switch};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const HOME: &str = "/home/fixture";

fn cfg(vars: &[(&str, &str)]) -> (PickConfig, Vec<String>) {
    let map: HashMap<String, OsString> = vars
        .iter()
        .map(|(k, v)| ((*k).to_string(), OsString::from(*v)))
        .collect();
    PickConfig::from_reader(|k| map.get(k).cloned(), Some(Path::new(HOME)))
}

#[test]
fn defaults_are_the_documented_values() {
    assert_eq!(config::DEFAULT_JOB_COST_PCT, 15.0);
    assert_eq!(config::DEFAULT_LEASE_TTL_SECS, 120);
    let (c, warnings) = cfg(&[]);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(c.job_cost_pct, 15.0);
    assert_eq!(c.lease_ttl_secs, 120);
    assert_eq!(
        c.lease_dir,
        PathBuf::from(HOME).join(".amplihack/state/account-leases")
    );
    assert_eq!(c.switch, Switch::On);
}

#[test]
fn valid_values_are_taken() {
    let (c, warnings) = cfg(&[
        ("AMPLIHACK_ACCOUNT_JOB_COST_PCT", "7.5"),
        ("AMPLIHACK_ACCOUNT_LEASE_TTL_SECS", "86400"),
        ("AMPLIHACK_ACCOUNT_LEASE_DIR", "/var/tmp/leases"),
        ("AMPLIHACK_ACCOUNT_PICK", "off"),
    ]);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(c.job_cost_pct, 7.5);
    assert_eq!(c.lease_ttl_secs, 86400);
    assert_eq!(c.lease_dir, PathBuf::from("/var/tmp/leases"));
    assert_eq!(c.switch, Switch::Off);
}

#[test]
fn job_cost_bounds_are_above_zero_and_at_most_100() {
    for ok in ["100", "0.01", "15", "1"] {
        let (c, w) = cfg(&[("AMPLIHACK_ACCOUNT_JOB_COST_PCT", ok)]);
        assert!(w.is_empty(), "{ok}: {w:?}");
        assert_eq!(c.job_cost_pct, ok.parse::<f64>().unwrap());
    }
    for bad in [
        "0", "-5", "NaN", "nan", "inf", "-inf", "1e9", "100.01", "abc", " 15",
    ] {
        let (c, w) = cfg(&[("AMPLIHACK_ACCOUNT_JOB_COST_PCT", bad)]);
        assert_eq!(c.job_cost_pct, 15.0, "{bad:?} must fall back to 15");
        assert_eq!(w.len(), 1, "{bad:?} gives exactly one warning: {w:?}");
        assert!(
            w[0].contains("AMPLIHACK_ACCOUNT_JOB_COST_PCT") && w[0].contains(&format!("{bad:?}")),
            "the warning names the variable and quotes the value: {}",
            w[0]
        );
    }
}

#[test]
fn ttl_bounds_are_1_to_86400() {
    for (raw, want) in [("1", 1), ("120", 120), ("86400", 86400)] {
        let (c, w) = cfg(&[("AMPLIHACK_ACCOUNT_LEASE_TTL_SECS", raw)]);
        assert!(w.is_empty(), "{raw}: {w:?}");
        assert_eq!(c.lease_ttl_secs, want);
    }
    for bad in ["0", "86401", "-1", "1.5", "x", "18446744073709551616"] {
        let (c, w) = cfg(&[("AMPLIHACK_ACCOUNT_LEASE_TTL_SECS", bad)]);
        assert_eq!(c.lease_ttl_secs, 120, "{bad:?} falls back to 120");
        assert_eq!(w.len(), 1, "{bad:?}: {w:?}");
        assert!(w[0].contains(&format!("{bad:?}")), "{}", w[0]);
    }
}

#[test]
fn lease_dir_must_be_absolute() {
    let (c, w) = cfg(&[("AMPLIHACK_ACCOUNT_LEASE_DIR", "relative/leases")]);
    assert_eq!(
        c.lease_dir,
        PathBuf::from(HOME).join(".amplihack/state/account-leases")
    );
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("\"relative/leases\""), "{}", w[0]);
}

#[test]
fn empty_values_count_as_unset_without_a_warning() {
    let (c, w) = cfg(&[
        ("AMPLIHACK_ACCOUNT_JOB_COST_PCT", ""),
        ("AMPLIHACK_ACCOUNT_LEASE_TTL_SECS", ""),
        ("AMPLIHACK_ACCOUNT_LEASE_DIR", ""),
        ("AMPLIHACK_ACCOUNT_PICK", ""),
    ]);
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(c.job_cost_pct, 15.0);
    assert_eq!(c.lease_ttl_secs, 120);
    assert_eq!(c.switch, Switch::On);
}

#[test]
fn the_switch_is_matched_without_regard_to_case() {
    for (raw, want) in [
        ("on", Switch::On),
        ("ON", Switch::On),
        ("Off", Switch::Off),
        ("OFF", Switch::Off),
        ("codex", Switch::Codex),
        ("CODEX", Switch::Codex),
    ] {
        let (c, w) = cfg(&[("AMPLIHACK_ACCOUNT_PICK", raw)]);
        assert!(w.is_empty(), "{raw}: {w:?}");
        assert_eq!(c.switch, want, "{raw}");
    }
}

#[test]
fn an_unknown_switch_value_falls_back_to_on_with_a_warning() {
    let (c, w) = cfg(&[("AMPLIHACK_ACCOUNT_PICK", "claude")]);
    assert_eq!(c.switch, Switch::On);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("\"claude\""), "{}", w[0]);
}

#[test]
fn a_warning_escapes_control_characters_in_the_value() {
    let (_, w) = cfg(&[("AMPLIHACK_ACCOUNT_JOB_COST_PCT", "1\n2\x1b[31m")]);
    assert_eq!(w.len(), 1);
    assert!(!w[0].contains('\n'), "raw newline printed: {:?}", w[0]);
    assert!(!w[0].contains('\x1b'), "raw escape printed: {:?}", w[0]);
}

#[test]
fn switch_allows_by_cli() {
    assert!(Switch::On.allows(Cli::Codex));
    assert!(Switch::On.allows(Cli::Claude));
    assert!(Switch::Codex.allows(Cli::Codex));
    assert!(!Switch::Codex.allows(Cli::Claude));
    assert!(!Switch::Off.allows(Cli::Codex));
    assert!(!Switch::Off.allows(Cli::Claude));
}

#[test]
fn cli_names_and_env_keys() {
    assert_eq!(Cli::Codex.as_str(), "codex");
    assert_eq!(Cli::Claude.as_str(), "claude");
    assert_eq!(Cli::Codex.env_key(), "CODEX_HOME");
    assert_eq!(Cli::Claude.env_key(), "CLAUDE_CONFIG_DIR");
}
