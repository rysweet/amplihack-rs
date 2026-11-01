//! Passive readings (issue #1844): a fresh reading in auto-switch's
//! `usage.json` stands in for a probe. Claude's status line writes these
//! readings as sessions run, and `profile usage` and the rotator record
//! what they read, so a login read recently needs no request at all.
//!
//! Read-only: nothing here creates state, takes a lock, makes a request or
//! prints.

use super::auto_switch::{self, Window};
use super::name::ProfileName;
use super::usage_parse::{UsageReport, UsageState, UsageWindow};
use std::path::Path;

/// The stored reading for the account whose login is in `dir`'s directory,
/// as a report, when it was observed at most `max_age` seconds before `now`
/// and has a plausible window. `max_age` 0 turns passive readings off.
///
/// `usage.json` has no field for "no session limit", so the report keeps
/// the session limit; a missing session window reads as not yet known.
pub(super) fn fresh(root: &Path, dir: &ProfileName, max_age: u64, now: i64) -> Option<UsageReport> {
    let max_age = i64::try_from(max_age).ok()?;
    let (five_hour, seven_day) = auto_switch::fresh_reading(root, dir.as_str(), max_age, now)?;
    let session = five_hour.map(|w| window(w, "session"));
    let week_all = seven_day.map(|w| window(w, "all models"));
    let full = |w: &Option<UsageWindow>| w.as_ref().is_some_and(|w| w.used_percentage >= 100.0);
    let state = if full(&session) {
        UsageState::SessionExhausted
    } else if full(&week_all) {
        UsageState::WeeklyExhausted
    } else {
        UsageState::Ok
    };
    Some(UsageReport {
        state,
        session,
        week_all,
        week_models: Vec::new(),
        session_limit: true,
    })
}

fn window(w: Window, label: &str) -> UsageWindow {
    UsageWindow {
        label: label.to_string(),
        used_percentage: w.used_percentage,
        resets_text: chrono::DateTime::from_timestamp(w.resets_at, 0)
            .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        resets_at: Some(w.resets_at),
    }
}
