//! Passive readings (issue #1844): a fresh reading in auto-switch's
//! `usage.json` stands in for a probe. Claude's status line writes these
//! readings as sessions run, and `profile usage` and the rotator record
//! what they read, so a login read recently needs no request at all.
//!
//! Read-only: nothing here creates state, takes a lock, makes a request or
//! prints.

use super::auto_switch::{self, Window};
use super::name::ProfileName;
use super::usage_history::Point;
use super::usage_parse::{UsageReport, UsageState, UsageWindow};
use std::path::Path;

/// A stored reading eligible for display alongside a failed probe.
pub(super) fn last_good(
    root: &Path,
    dir: &ProfileName,
    now: i64,
    history: Option<&Point>,
) -> Option<UsageReport> {
    let passive = fresh(
        root,
        dir,
        super::rotate::fresh::MAX_READING_AGE_SECS as u64,
        now,
    );
    if let Some(recorded) = history
        && passive
            .as_ref()
            .is_none_or(|report| recorded.observed_at > report.observed_at)
    {
        let state = if recorded.s == Some(100) {
            UsageState::SessionExhausted
        } else if recorded.w == 100 {
            UsageState::WeeklyExhausted
        } else {
            UsageState::Ok
        };
        return Some(UsageReport {
            state,
            session: recorded
                .s
                .map(|s| display_window("session", f64::from(s), None)),
            week_all: Some(display_window(
                "all models",
                f64::from(recorded.w),
                recorded.w_reset,
            )),
            week_models: Vec::new(),
            session_limit: !recorded.no_session_limit,
            observed_at: recorded.observed_at,
        });
    }
    passive
}

/// The stored reading for the account whose login is in `dir`'s directory,
/// as a report, when it was observed at most `max_age` seconds before `now`
/// and has a plausible window. `max_age` 0 turns passive readings off.
///
/// A reading with a week window and no session window has no session limit
/// (#1818), as a direct read of the same answer has and as
/// `auto_switch::no_session_window_profiles` counts it, so a weekly-only
/// login reads the same whichever way it was read.
pub(super) fn fresh(root: &Path, dir: &ProfileName, max_age: u64, now: i64) -> Option<UsageReport> {
    let max_age = i64::try_from(max_age).ok()?;
    let reading = auto_switch::fresh_reading(root, dir.as_str(), max_age, now)?;
    let session = reading.session.map(|w| window(w, "session"));
    let week_all = reading.week.map(|w| window(w, "all models"));
    let week_models: Vec<_> = reading
        .week_models
        .into_iter()
        .map(|m| {
            let mut w = window(
                Window {
                    used_percentage: m.used_percentage,
                    // fresh_reading only returns models with a future reset.
                    resets_at: m.resets_at.expect("unexpired model week"),
                },
                &m.label,
            );
            w.observed_at = Some(m.observed_at);
            w
        })
        .collect();
    let full = |w: &Option<UsageWindow>| w.as_ref().is_some_and(|w| w.used_percentage >= 100.0);
    let state = if full(&session) {
        UsageState::SessionExhausted
    } else if full(&week_all) || week_models.iter().any(|w| w.used_percentage >= 100.0) {
        UsageState::WeeklyExhausted
    } else {
        UsageState::Ok
    };
    Some(UsageReport {
        state,
        session,
        week_all,
        week_models,
        session_limit: !reading.no_session_window,
        observed_at: Some(reading.observed_at),
    })
}

fn window(w: Window, label: &str) -> UsageWindow {
    display_window(label, w.used_percentage, Some(w.resets_at))
}

fn display_window(label: &str, used_percentage: f64, resets_at: Option<i64>) -> UsageWindow {
    UsageWindow {
        observed_at: None,
        label: label.to_string(),
        used_percentage,
        resets_text: resets_at
            .and_then(|reset| chrono::DateTime::from_timestamp(reset, 0))
            .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        resets_at,
    }
}
