//! Exhausted while off (issue #1803): auto-switch has been turned off while
//! the live login has no capacity left, so nothing moves work onto another
//! login. `watch` logs it as an ALERT on every cycle and `rotate status`
//! prints it as an `alert:` line. See "Exhausted while off" in
//! docs/reference/profile-command.md.
//!
//! Read-only: takes no lock, runs no probe and writes nothing. Missing data
//! (no ledger, no reading, a non-finite value) gives no alert. A ledger or
//! `usage.json` that fails to load is an error, which `watch` logs as
//! `rotate: alert check failed: ...` and `status` prints as
//! `alert: unavailable (...)`.

use super::ledger::RotateState;
use super::log::timestamp;
use crate::commands::profile::auto_switch::kit::{ReadingSource, Source, State, effective};
use std::fmt;
use std::path::Path;

/// The ALERT text, before the `(<profile>: <reason>)` detail.
pub const ALERT: &str = "auto-switch is off while the live login is exhausted";

/// Why the live login counts as exhausted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reason {
    /// The higher of the session and weekly percentages is at or above the
    /// auto-switch threshold.
    Threshold { pct: f64, threshold: u8 },
    /// Claude reported its usage limit, resetting at `resets_at`.
    LimitStop { resets_at: i64 },
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Threshold { pct, threshold } => {
                let whole = if pct.is_finite() && *pct > 0.0 {
                    pct.floor() as u64
                } else {
                    0
                };
                write!(f, "{whole}% used, threshold {threshold}%")
            }
            Self::LimitStop { resets_at } => {
                write!(f, "usage limit reached, resets {}", timestamp(*resets_at))
            }
        }
    }
}

/// The exhausted live login and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Exhausted {
    pub profile: String,
    pub reason: Reason,
}

impl fmt::Display for Exhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.profile, self.reason)
    }
}

/// Whether auto-switch has been turned off (by its settings file or
/// `AMPLIHACK_CLAUDE_AUTO_SWITCH=0`), or its settings file is damaged.
/// Without any setting, rotation runs.
pub(super) fn paused(root: &Path) -> bool {
    let eff = effective(root);
    !eff.on && eff.source != Source::Default
}

/// The exhausted live login under `root` at `now`, while auto-switch is off.
/// The rotate ledger names the live directory; the auto-switch ledger maps
/// it to the account whose login it holds, and that account's reading in
/// `usage.json` decides. A usage-limit failure with a future reset wins
/// over the threshold.
///
/// `Ok(None)` when auto-switch is on, a file or entry is missing, or the
/// login is under its threshold; `Err` when a ledger or `usage.json` exists
/// but fails to load.
pub fn exhausted_while_off(root: &Path, now: i64) -> Result<Option<Exhausted>, String> {
    let eff = effective(root);
    if eff.on || eff.source == Source::Default {
        return Ok(None);
    }
    let rotate = RotateState::new(root).load_ledger().map_err(failed)?;
    let Some(live) = rotate.and_then(|l| l.live) else {
        return Ok(None);
    };
    let state = State::new(root).map_err(failed)?;
    let ledger = state.load_ledger().map_err(failed)?;
    let profile = ledger.account_in(&live).to_string();
    let usage = state.try_load_usage().map_err(failed)?;
    let Some(reading) = usage.accounts.get(&profile) else {
        return Ok(None);
    };
    let limit_reset = (reading.source == ReadingSource::UsageLimitFailure)
        .then(|| {
            [reading.five_hour, reading.seven_day]
                .into_iter()
                .flatten()
                .map(|w| w.resets_at)
                .filter(|r| *r > now)
                .max()
        })
        .flatten();
    let reason = match limit_reset {
        Some(resets_at) => Reason::LimitStop { resets_at },
        None => {
            let pct = reading.worst();
            let threshold = eff.config.threshold;
            if !pct.is_finite() || pct < f64::from(threshold) {
                return Ok(None);
            }
            Reason::Threshold { pct, threshold }
        }
    };
    Ok(Some(Exhausted { profile, reason }))
}

fn failed(e: impl fmt::Display) -> String {
    e.to_string()
}

/// The `rotate status` line for [`exhausted_while_off`], if any.
pub fn status_line(root: &Path, now: i64) -> Option<String> {
    match exhausted_while_off(root, now) {
        Ok(None) => None,
        Ok(Some(e)) => Some(format!("alert: {ALERT} ({e})")),
        Err(m) => Some(format!("alert: unavailable ({m})")),
    }
}

/// The `rotate.log` line for [`exhausted_while_off`] on a paused cycle, if
/// any.
pub fn watch_line(root: &Path, now: i64) -> Option<String> {
    match exhausted_while_off(root, now) {
        Ok(None) => None,
        Ok(Some(e)) => Some(format!("ALERT: {ALERT} ({e})")),
        Err(m) => Some(format!("rotate: alert check failed: {m}")),
    }
}
