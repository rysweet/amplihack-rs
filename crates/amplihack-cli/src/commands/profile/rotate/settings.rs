//! Rotate settings: the flag, then its `AMPLIHACK_ROTATE_*` variable, then
//! (issue #2007) its `rotator.*` key in the user config file, then the
//! default (D4). Values are unsigned decimal integers in range; an error
//! names the flag, variable or key and shows the value escaped and cut.
//!
//! Only `session_at`, `week_at` and `interval` have a key; the other
//! settings go from the variable straight to the default. The Codex credit
//! floor (`rotator.codex_floor`, [`codex_floor`]) is a key with no flag and
//! no variable. A config value is checked against the range again here and
//! is never clamped: a value that cannot be used is an error naming the key.

use super::TuneFlags;
use crate::commands::profile::ProfileError;
use crate::commands::profile::overflow::Gate;
use crate::user_config::schema::{ValueKind, kind_requirement};
use crate::user_config::{self, Value};
use amplihack_utils::codex_capacity::DEFAULT_FLOOR;
use std::ffi::OsString;
use std::path::Path;

/// The environment, as a lookup: `std::env::var_os` or a test's map.
pub type Env<'a> = dyn Fn(&str) -> Option<OsString> + 'a;
/// The user file's value for a `rotator.*` key: `Ok(None)` when it is not
/// set, `Err` with the reason when it cannot be used.
pub type Cfg<'a> = dyn Fn(&str) -> Result<Option<u64>, String> + 'a;

/// A [`Cfg`] that sets nothing, for paths that use the defaults only and
/// for tests that must not read a real config file.
pub fn no_config(_: &str) -> Result<Option<u64>, String> {
    Ok(None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub session_at: u8,
    pub week_at: u8,
    pub margin: u8,
    pub upgrade_margin: u8,
    /// Seconds; 0 disables the hold.
    pub hold: u64,
    /// Seconds between `watch` cycles.
    pub interval: u64,
    /// Seconds: the rotator and auto-switch move onto an overflow profile
    /// only when the earliest plan reset is further away than this.
    pub overflow_wait: u64,
    /// Whether a plan reset that no reading reports may count as further
    /// away than `overflow_wait`. Off by default: with no known reset time
    /// nothing moves onto an overflow profile, because that move costs
    /// money and the plan login may come back in minutes.
    pub overflow_unknown_reset: bool,
}

/// One setting: its flag, its variable, its `rotator.*` key if it has one,
/// its default and its range.
struct Spec {
    flag: &'static str,
    var: &'static str,
    key: Option<&'static str>,
    default: u64,
    min: u64,
    max: Option<u64>,
}

const SESSION_AT: Spec = Spec {
    flag: "--session-at",
    var: "AMPLIHACK_ROTATE_SESSION_AT",
    key: Some("rotator.session_at"),
    default: 85,
    min: 1,
    max: Some(100),
};
const WEEK_AT: Spec = Spec {
    flag: "--week-at",
    var: "AMPLIHACK_ROTATE_WEEK_AT",
    key: Some("rotator.week_at"),
    default: 90,
    min: 1,
    max: Some(100),
};
const MARGIN: Spec = Spec {
    flag: "--margin",
    var: "AMPLIHACK_ROTATE_MARGIN",
    key: None,
    default: 5,
    min: 0,
    max: Some(100),
};
const UPGRADE_MARGIN: Spec = Spec {
    flag: "--upgrade-margin",
    var: "AMPLIHACK_ROTATE_UPGRADE_MARGIN",
    key: None,
    default: 30,
    min: 0,
    max: Some(100),
};
const HOLD: Spec = Spec {
    flag: "--hold",
    var: "AMPLIHACK_ROTATE_HOLD",
    key: None,
    default: 3600,
    min: 0,
    max: None,
};
const OVERFLOW_WAIT: Spec = Spec {
    flag: "--overflow-wait",
    var: "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
    key: None,
    default: 1800,
    min: 0,
    max: Some(604_800),
};
const OVERFLOW_UNKNOWN_RESET: Spec = Spec {
    flag: "--overflow-unknown-reset",
    var: "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
    key: None,
    default: 0,
    min: 0,
    max: Some(1),
};
const INTERVAL: Spec = Spec {
    flag: "--interval",
    var: "AMPLIHACK_ROTATE_INTERVAL",
    key: Some("rotator.interval"),
    default: 600,
    min: 1,
    max: Some(86_400),
};

/// `rotator.codex_floor`: no flag, no variable, `codex-pick`'s default.
const CODEX_FLOOR_KEY: &str = "rotator.codex_floor";
const CODEX_FLOOR_MIN: u64 = 0;
const CODEX_FLOOR_MAX: u64 = 1_000_000;
const CODEX_FLOOR_DEFAULT: u64 = DEFAULT_FLOOR as u64;

/// The `rotator.*` keys [`UserConfig`] reads.
const CONFIG_KEYS: [&str; 4] = [
    "rotator.session_at",
    "rotator.week_at",
    "rotator.interval",
    CODEX_FLOOR_KEY,
];

/// Where a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Flag,
    Env,
    Config,
    Default,
    /// The running rotator's recorded gate (`profile usage` only).
    Rotator,
}

impl Origin {
    /// The lowercase label of `profile usage` and `--summary-json`.
    pub fn label(self) -> &'static str {
        match self {
            Origin::Flag => "flag",
            Origin::Env => "env",
            Origin::Config => "config",
            Origin::Default => "default",
            Origin::Rotator => "rotator",
        }
    }
}

/// A value and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Threshold<T> {
    pub value: T,
    pub origin: Origin,
}

/// The thresholds in force, each resolved on its own, so a bad value
/// fails only its own field.
#[derive(Debug)]
pub struct Thresholds {
    pub session_at: Result<Threshold<u8>, ProfileError>,
    pub week_at: Result<Threshold<u8>, ProfileError>,
    pub interval: Result<Threshold<u64>, ProfileError>,
    pub codex_floor: Result<Threshold<u64>, ProfileError>,
}

/// How to fix a problem with the whole user file. `config set` and
/// `config unset` cannot help, so the message points at the file; the path
/// itself is left to `config list --show-origin`.
pub const WHOLE_FILE_FIX: &str = "; fix the user config file first \
     (`amplihack config list --show-origin` shows its path and the problem)";

/// The error for a `rotator.*` value that cannot be used: the key, the
/// reason and how to fix it. A reason from [`UserConfig`] for a problem
/// with the whole file already ends with [`WHOLE_FILE_FIX`] and is used as
/// it is; any other reason gets the `config set`/`config unset` advice. It
/// never quotes the file's path or lines.
fn config_error(key: &str, why: &str) -> ProfileError {
    if why.ends_with(WHOLE_FILE_FIX) {
        return ProfileError::RotateConfig(format!("invalid rotate setting {key}: {why}"));
    }
    ProfileError::RotateConfig(format!(
        "invalid rotate setting {key}: {why}; fix it with `amplihack config set {key} <value>` \
         or `amplihack config unset {key}`"
    ))
}

/// `key` from `cfg`, checked against `min..=max` again; `Err` is the bare
/// reason.
fn config_value(key: &str, min: u64, max: u64, cfg: &Cfg<'_>) -> Result<Option<u64>, String> {
    match cfg(key)? {
        None => Ok(None),
        Some(v) if v >= min && v <= max => Ok(Some(v)),
        Some(_) => Err(kind_requirement(ValueKind::Int { min, max })),
    }
}

/// [`config_value`], with the error naming the key and the fix.
fn from_config(key: &str, min: u64, max: u64, cfg: &Cfg<'_>) -> Result<Option<u64>, ProfileError> {
    config_value(key, min, max, cfg).map_err(|why| config_error(key, &why))
}

/// A percentage as `u8`; the ranges stop at 100, so this only fails if a
/// range is ever widened past 255.
fn narrow(t: Threshold<u64>, name: &str) -> Result<Threshold<u8>, ProfileError> {
    let value = u8::try_from(t.value).map_err(|_| {
        ProfileError::RotateConfig(format!("invalid rotate setting {name}: above 255"))
    })?;
    Ok(Threshold {
        value,
        origin: t.origin,
    })
}

/// Shown value: the first 32 characters, escaped.
fn shown(raw: &str) -> String {
    let head: String = raw.chars().take(32).collect();
    format!("{head:?}")
}

impl Spec {
    fn expected(&self) -> String {
        match self.max {
            Some(max) => format!("expected a whole number from {} to {max}", self.min),
            None => format!("expected a whole number from {} up", self.min),
        }
    }

    fn parse(&self, name: &str, raw: &str) -> Result<u64, ProfileError> {
        let fail = || {
            ProfileError::RotateConfig(format!(
                "invalid rotate setting {name} {}: {}",
                shown(raw),
                self.expected()
            ))
        };
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(fail());
        }
        let v: u64 = raw.parse().map_err(|_| fail())?;
        if v < self.min || self.max.is_some_and(|max| v > max) {
            return Err(fail());
        }
        Ok(v)
    }

    fn resolve(
        &self,
        flag: Option<&str>,
        env: &Env<'_>,
        cfg: &Cfg<'_>,
    ) -> Result<u64, ProfileError> {
        self.resolve_traced(flag, env, cfg).map(|t| t.value)
    }

    /// The flag, then the variable, then the key, then the default. A layer
    /// that is set and invalid is an error; it never falls through.
    fn resolve_traced(
        &self,
        flag: Option<&str>,
        env: &Env<'_>,
        cfg: &Cfg<'_>,
    ) -> Result<Threshold<u64>, ProfileError> {
        let at = |value, origin| Threshold { value, origin };
        if let Some(raw) = flag {
            return self.parse(self.flag, raw).map(|v| at(v, Origin::Flag));
        }
        if let Some(v) = env(self.var).filter(|v| !v.is_empty()) {
            return self
                .parse(self.var, &v.to_string_lossy())
                .map(|v| at(v, Origin::Env));
        }
        if let Some(key) = self.key {
            let max = self.max.unwrap_or(u64::MAX);
            if let Some(v) = from_config(key, self.min, max, cfg)? {
                return Ok(at(v, Origin::Config));
            }
        }
        Ok(at(self.default, Origin::Default))
    }

    fn pct(&self, flag: Option<&str>, env: &Env<'_>, cfg: &Cfg<'_>) -> Result<u8, ProfileError> {
        narrow(self.resolve_traced(flag, env, cfg)?, self.flag).map(|t| t.value)
    }
}

/// The default and range of a `rotator.*` key as the rotator applies them,
/// for the test that pins the config schema to them.
#[cfg(test)]
pub fn config_spec(key: &str) -> Option<(u64, u64, u64)> {
    if key == CODEX_FLOOR_KEY {
        return Some((CODEX_FLOOR_DEFAULT, CODEX_FLOOR_MIN, CODEX_FLOOR_MAX));
    }
    [&SESSION_AT, &WEEK_AT, &INTERVAL]
        .into_iter()
        .find(|s| s.key == Some(key))
        .map(|s| (s.default, s.min, s.max.unwrap_or(u64::MAX)))
}

/// The settings for this run. `interval` is the `--interval` flag of `start`
/// and `watch`; the one-shot form has none and gets the variable, the key or
/// the default. `cfg` is the user file's `rotator.*` values
/// ([`UserConfig`], or [`no_config`] for defaults only).
pub fn resolve(
    tune: &TuneFlags,
    interval: Option<&str>,
    env: &Env<'_>,
    cfg: &Cfg<'_>,
) -> Result<Settings, ProfileError> {
    let pct = |spec: &Spec, flag: &Option<String>| spec.pct(flag.as_deref(), env, cfg);
    Ok(Settings {
        session_at: pct(&SESSION_AT, &tune.session_at)?,
        week_at: pct(&WEEK_AT, &tune.week_at)?,
        margin: pct(&MARGIN, &tune.margin)?,
        upgrade_margin: pct(&UPGRADE_MARGIN, &tune.upgrade_margin)?,
        hold: HOLD.resolve(tune.hold.as_deref(), env, cfg)?,
        interval: INTERVAL.resolve(interval, env, cfg)?,
        overflow_wait: OVERFLOW_WAIT.resolve(tune.overflow_wait.as_deref(), env, cfg)?,
        overflow_unknown_reset: OVERFLOW_UNKNOWN_RESET.resolve(
            tune.overflow_unknown_reset.as_deref(),
            env,
            cfg,
        )? == 1,
    })
}

/// The overflow [`Gate`] from its four variables
/// (`AMPLIHACK_ROTATE_OVERFLOW_WAIT`, `..._OVERFLOW_UNKNOWN_RESET`,
/// `..._SESSION_AT`, `..._WEEK_AT`), with `rotator.session_at` and
/// `rotator.week_at` under the last two, for auto-switch when no rotator
/// runs. Another `AMPLIHACK_ROTATE_*` variable or `rotator.*` key, valid or
/// not, plays no part.
pub fn gate_from_env(env: &Env<'_>, cfg: &Cfg<'_>) -> Result<Gate, ProfileError> {
    Ok(Gate {
        wait: OVERFLOW_WAIT.resolve(None, env, cfg)?,
        unknown_reset: OVERFLOW_UNKNOWN_RESET.resolve(None, env, cfg)? == 1,
        session_at: SESSION_AT.pct(None, env, cfg)?,
        week_at: WEEK_AT.pct(None, env, cfg)?,
    })
}

/// The thresholds with no flag given, each with its origin, for
/// `profile usage`.
pub fn thresholds(env: &Env<'_>, cfg: &Cfg<'_>) -> Thresholds {
    let pct = |spec: &Spec| {
        spec.resolve_traced(None, env, cfg)
            .and_then(|t| narrow(t, spec.flag))
    };
    Thresholds {
        session_at: pct(&SESSION_AT),
        week_at: pct(&WEEK_AT),
        interval: INTERVAL.resolve_traced(None, env, cfg),
        codex_floor: codex_floor(cfg),
    }
}

/// The Codex credit floor: `rotator.codex_floor`, else `codex-pick`'s
/// default [`DEFAULT_FLOOR`]. It has no flag and no variable. An account
/// whose rate limits are used up counts only with a balance above it.
pub fn codex_floor(cfg: &Cfg<'_>) -> Result<Threshold<u64>, ProfileError> {
    let at = |value, origin| Threshold { value, origin };
    Ok(
        from_config(CODEX_FLOOR_KEY, CODEX_FLOOR_MIN, CODEX_FLOOR_MAX, cfg)?
            .map_or(at(CODEX_FLOOR_DEFAULT, Origin::Default), |v| {
                at(v, Origin::Config)
            }),
    )
}

/// [`codex_floor`]'s value, with only the bare reason when it cannot be
/// used (`must be an integer from 0 to 1000000`), for the Codex block's
/// `next: not chosen (rotator.codex_floor: <reason>)` line.
pub fn codex_floor_value(cfg: &Cfg<'_>) -> Result<u64, String> {
    Ok(
        config_value(CODEX_FLOOR_KEY, CODEX_FLOOR_MIN, CODEX_FLOOR_MAX, cfg)?
            .unwrap_or(CODEX_FLOOR_DEFAULT),
    )
}

/// The `rotator.*` keys of one user config file, read once. Each lookup
/// fails closed: a file that cannot be read or parsed, a user file group or
/// others can write, a value of the wrong type or out of range, or a HOME
/// the test guard forbids gives `Err`, never the default.
#[derive(Debug, Clone)]
pub struct UserConfig {
    values: Vec<(&'static str, Result<Option<u64>, String>)>,
}

impl UserConfig {
    /// The user file under this process's HOME. With no HOME (or one the
    /// test guard discards), there is no user file and nothing is set.
    pub fn read() -> Self {
        match user_config::home_dir() {
            Some(home) => Self::at(&home),
            None => Self {
                values: CONFIG_KEYS.iter().map(|k| (*k, Ok(None))).collect(),
            },
        }
    }

    /// The user file under `home`.
    pub fn at(home: &Path) -> Self {
        let read = user_config::user_keys_detail(home, &CONFIG_KEYS);
        // A fault with the whole file makes every key from it unusable,
        // including keys the file does not set: fail closed, never default.
        let whole_file = read.iter().find_map(|r| match r {
            Err(p) if p.whole_file => Some(format!("{}{WHOLE_FILE_FIX}", p.reason)),
            _ => None,
        });
        let values = CONFIG_KEYS
            .iter()
            .zip(read)
            .map(|(key, r)| {
                let v = match (&whole_file, r) {
                    (Some(why), _) => Err(why.clone()),
                    (None, Ok(None)) => Ok(None),
                    (None, Ok(Some(Value::Int(n)))) => Ok(Some(n)),
                    (None, Ok(Some(_))) => Err("is not an integer".to_string()),
                    (None, Err(p)) => Err(p.reason),
                };
                (*key, v)
            })
            .collect();
        Self { values }
    }

    /// The value of `key`, a [`Cfg`]. Asking for a key this reader did not
    /// load is a programming error and is reported, never treated as unset.
    pub fn get(&self, key: &str) -> Result<Option<u64>, String> {
        match self.values.iter().find(|(k, _)| *k == key) {
            Some((_, v)) => v.clone(),
            None => Err(format!("{key} is not a rotator key")),
        }
    }
}

/// Whether `g` holds values the rotator could have resolved: thresholds
/// from 1 to 100 and a wait of at most a week. A recorded gate that fails
/// this is not used.
pub fn gate_in_range(g: &Gate) -> bool {
    let pct =
        |spec: &Spec, v: u8| u64::from(v) >= spec.min && spec.max.is_none_or(|m| u64::from(v) <= m);
    pct(&SESSION_AT, g.session_at)
        && pct(&WEEK_AT, g.week_at)
        && OVERFLOW_WAIT.max.is_none_or(|m| g.wait <= m)
}

impl Settings {
    /// Explicit flags for the background process, so it reads no variable.
    pub fn to_args(self, with_interval: bool) -> Vec<String> {
        let mut args = Vec::new();
        for (flag, v) in [
            (SESSION_AT.flag, u64::from(self.session_at)),
            (WEEK_AT.flag, u64::from(self.week_at)),
            (MARGIN.flag, u64::from(self.margin)),
            (UPGRADE_MARGIN.flag, u64::from(self.upgrade_margin)),
            (HOLD.flag, self.hold),
            (OVERFLOW_WAIT.flag, self.overflow_wait),
            (
                OVERFLOW_UNKNOWN_RESET.flag,
                u64::from(self.overflow_unknown_reset),
            ),
        ] {
            args.push(flag.to_string());
            args.push(v.to_string());
        }
        if with_interval {
            args.push(INTERVAL.flag.to_string());
            args.push(self.interval.to_string());
        }
        args
    }
}

/// `$HOME`, which holds the live `~/.claude`.
pub fn home_dir() -> Result<std::path::PathBuf, ProfileError> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| ProfileError::RotateRefused("HOME is not set".into()))
}
