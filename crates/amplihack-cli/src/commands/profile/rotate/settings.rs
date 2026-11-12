//! Rotate settings: the flag, then its `AMPLIHACK_ROTATE_*` variable, then
//! (issue #2007) its `rotator.*` key in the user config file, then the
//! default (D4). Values are unsigned decimal integers in range; an error
//! names the flag, variable or key and shows the value escaped and cut.
//!
//! Only `session_at`, `week_at` and `interval` have a key; the other
//! settings go from the variable straight to the default. The Codex credit
//! floor (`rotator.codex_credit_floor`, [`codex_credit_floor`]) is a key with no flag and
//! no variable. A config value is checked against the range again here and
//! is never clamped: a value that cannot be used is an error naming the key.

use super::TuneFlags;
use crate::commands::profile::ProfileError;
use crate::commands::profile::overflow::Gate;
use std::ffi::OsString;

mod config;
use config::from_config;
#[cfg(test)]
use config::{CODEX_FLOOR_DEFAULT, CODEX_FLOOR_KEY, CODEX_FLOOR_MAX, CODEX_FLOOR_MIN};
pub use config::{
    Cfg, Origin, Threshold, Thresholds, UserConfig, codex_credit_floor, codex_credit_floor_value,
    no_config, user_file_cfg,
};

/// The environment, as a lookup: `std::env::var_os` or a test's map.
pub type Env<'a> = dyn Fn(&str) -> Option<OsString> + 'a;
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

/// A percentage as `u8`: every percentage range stops at 100.
fn narrow(t: Threshold<u64>) -> Threshold<u8> {
    Threshold {
        value: t.value as u8,
        origin: t.origin,
    }
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
        Ok(narrow(self.resolve_traced(flag, env, cfg)?).value)
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
pub fn resolve_with(
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
pub fn gate_with(env: &Env<'_>, cfg: &Cfg<'_>) -> Result<Gate, ProfileError> {
    Ok(Gate {
        wait: OVERFLOW_WAIT.resolve(None, env, cfg)?,
        unknown_reset: OVERFLOW_UNKNOWN_RESET.resolve(None, env, cfg)? == 1,
        session_at: SESSION_AT.pct(None, env, cfg)?,
        week_at: WEEK_AT.pct(None, env, cfg)?,
    })
}

/// Defaults only: no `rotator.*` keys. See [`resolve_with`].
pub fn resolve(
    tune: &TuneFlags,
    interval: Option<&str>,
    env: &Env<'_>,
) -> Result<Settings, ProfileError> {
    resolve_with(tune, interval, env, &no_config)
}

/// The thresholds with no flag given, each with its origin, for
/// `profile usage`.
pub fn thresholds(env: &Env<'_>, cfg: &Cfg<'_>) -> Thresholds {
    let pct = |spec: &Spec| spec.resolve_traced(None, env, cfg).map(narrow);
    Thresholds {
        session_at: pct(&SESSION_AT),
        week_at: pct(&WEEK_AT),
        interval: INTERVAL.resolve_traced(None, env, cfg),
        codex_credit_floor: codex_credit_floor(cfg),
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
