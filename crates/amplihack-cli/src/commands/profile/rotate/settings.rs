//! Rotate settings: the flag, then its `AMPLIHACK_ROTATE_*` variable, then
//! the default (D4). Values are unsigned decimal integers in range; an error
//! names the flag or variable and shows the value escaped and cut.

use super::TuneFlags;
use crate::commands::profile::ProfileError;
use crate::commands::profile::overflow::Gate;
use std::ffi::OsString;

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

/// One setting: its flag, its variable, its default and its range.
struct Spec {
    flag: &'static str,
    var: &'static str,
    default: u64,
    min: u64,
    max: Option<u64>,
}

const SESSION_AT: Spec = Spec {
    flag: "--session-at",
    var: "AMPLIHACK_ROTATE_SESSION_AT",
    default: 85,
    min: 1,
    max: Some(100),
};
const WEEK_AT: Spec = Spec {
    flag: "--week-at",
    var: "AMPLIHACK_ROTATE_WEEK_AT",
    default: 90,
    min: 1,
    max: Some(100),
};
const MARGIN: Spec = Spec {
    flag: "--margin",
    var: "AMPLIHACK_ROTATE_MARGIN",
    default: 5,
    min: 0,
    max: Some(100),
};
const UPGRADE_MARGIN: Spec = Spec {
    flag: "--upgrade-margin",
    var: "AMPLIHACK_ROTATE_UPGRADE_MARGIN",
    default: 30,
    min: 0,
    max: Some(100),
};
const HOLD: Spec = Spec {
    flag: "--hold",
    var: "AMPLIHACK_ROTATE_HOLD",
    default: 3600,
    min: 0,
    max: None,
};
const OVERFLOW_WAIT: Spec = Spec {
    flag: "--overflow-wait",
    var: "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
    default: 1800,
    min: 0,
    max: Some(604_800),
};
const OVERFLOW_UNKNOWN_RESET: Spec = Spec {
    flag: "--overflow-unknown-reset",
    var: "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
    default: 0,
    min: 0,
    max: Some(1),
};
const INTERVAL: Spec = Spec {
    flag: "--interval",
    var: "AMPLIHACK_ROTATE_INTERVAL",
    default: 600,
    min: 1,
    max: Some(86_400),
};

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
        env: &dyn Fn(&str) -> Option<OsString>,
    ) -> Result<u64, ProfileError> {
        if let Some(raw) = flag {
            return self.parse(self.flag, raw);
        }
        match env(self.var).filter(|v| !v.is_empty()) {
            Some(v) => self.parse(self.var, &v.to_string_lossy()),
            None => Ok(self.default),
        }
    }
}

/// The settings for this run. `interval` is the `--interval` flag of `start`
/// and `watch`; the one-shot form has none and gets the default.
pub fn resolve(
    tune: &TuneFlags,
    interval: Option<&str>,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Settings, ProfileError> {
    // Percentages and margins are at most 100, so they fit in a u8.
    let pct =
        |spec: &Spec, flag: &Option<String>| spec.resolve(flag.as_deref(), env).map(|v| v as u8);
    Ok(Settings {
        session_at: pct(&SESSION_AT, &tune.session_at)?,
        week_at: pct(&WEEK_AT, &tune.week_at)?,
        margin: pct(&MARGIN, &tune.margin)?,
        upgrade_margin: pct(&UPGRADE_MARGIN, &tune.upgrade_margin)?,
        hold: HOLD.resolve(tune.hold.as_deref(), env)?,
        interval: INTERVAL.resolve(interval, env)?,
        overflow_wait: OVERFLOW_WAIT.resolve(tune.overflow_wait.as_deref(), env)?,
        overflow_unknown_reset: OVERFLOW_UNKNOWN_RESET
            .resolve(tune.overflow_unknown_reset.as_deref(), env)?
            == 1,
    })
}

/// The overflow [`Gate`] from its four variables only
/// (`AMPLIHACK_ROTATE_OVERFLOW_WAIT`, `..._OVERFLOW_UNKNOWN_RESET`,
/// `..._SESSION_AT`, `..._WEEK_AT`), for auto-switch when no rotator runs.
/// Another `AMPLIHACK_ROTATE_*` variable, valid or not, plays no part.
pub fn gate_from_env(env: &dyn Fn(&str) -> Option<OsString>) -> Result<Gate, ProfileError> {
    Ok(Gate {
        wait: OVERFLOW_WAIT.resolve(None, env)?,
        unknown_reset: OVERFLOW_UNKNOWN_RESET.resolve(None, env)? == 1,
        session_at: SESSION_AT.resolve(None, env)? as u8,
        week_at: WEEK_AT.resolve(None, env)? as u8,
    })
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
