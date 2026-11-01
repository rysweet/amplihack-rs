//! Whether the live login is exhausted while auto-switch is off (issue
//! #1803), judged only from data already on disk. See "Exhausted live login"
//! in docs/reference/profile-command.md.
//!
//! The paused loop must stay passive, so this runs no probe, takes no lock
//! and writes nothing. Any missing or damaged input counts as not exhausted.

use super::daemon::DaemonState;
use super::gate_record;
use super::ledger::RotateState;
use super::settings;
use crate::commands::profile::auto_switch::kit::{Reading, ReadingSource, State, adjusted};
use std::path::Path;

/// The line the paused loop logs on every pass.
pub const ALERT: &str = "ALERT: auto-switch is off while the live login is exhausted";
/// The line `rotate status` prints.
pub const STATUS: &str = "alert: auto-switch is off while the live login is exhausted";

/// A window at or over its own threshold that has not reset, or a recorded
/// usage limit whose window has not reset.
pub fn reading_exhausted(r: &Reading, session_at: u8, week_at: u8, now: i64) -> bool {
    let (five, seven) = (r.five_hour.as_ref(), r.seven_day.as_ref());
    if r.source == ReadingSource::UsageLimitFailure
        && [five, seven]
            .into_iter()
            .flatten()
            .any(|w| w.resets_at > now)
    {
        return true;
    }
    adjusted(five, now) >= f64::from(session_at) || adjusted(seven, now) >= f64::from(week_at)
}

/// Whether the rotate ledger's `live` profile has an exhausted reading in
/// `usage.json`.
pub fn live_exhausted(root: &Path, session_at: u8, week_at: u8, now: i64) -> bool {
    let Ok(Some(ledger)) = RotateState::new(root).load_ledger() else {
        return false;
    };
    let (Some(live), Ok(state)) = (ledger.live, State::new(root)) else {
        return false;
    };
    state
        .load_usage()
        .accounts
        .get(&live)
        .is_some_and(|r| reading_exhausted(r, session_at, week_at, now))
}

/// The session and week thresholds `rotate status` judges with: the running
/// rotator's own, or the defaults.
pub fn thresholds(rs: &RotateState, daemon: &DaemonState) -> (u8, u8) {
    if let DaemonState::Running(pid) = daemon
        && let Ok(Some(record)) = gate_record::load(rs)
        && record.pid == *pid
    {
        return (record.session_at, record.week_at);
    }
    settings::default_thresholds()
}
