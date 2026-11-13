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
/// A reading with a week window and no session window has no session limit
/// (#1818), as a direct read of the same answer has and as
/// `auto_switch::no_session_window_profiles` counts it, so a weekly-only
/// login reads the same whichever way it was read.
pub(super) fn fresh(root: &Path, dir: &ProfileName, max_age: u64, now: i64) -> Option<UsageReport> {
    #[cfg(unix)]
    if max_age > 0 && root.join(".auto-switch.d/observation-order.json").exists() {
        return generation_reading(root, dir);
    }
    let max_age = i64::try_from(max_age).ok()?;
    let reading = auto_switch::fresh_reading(root, dir.as_str(), max_age, now)?;
    let session = reading.session.map(|w| window(w, "session"));
    let week_all = reading.week.map(|w| window(w, "all models"));
    let full = |w: &Option<UsageWindow>| w.as_ref().is_some_and(|w| w.used_percentage >= 100.0);
    let state = if full(&session) {
        UsageState::SessionExhausted
    } else if full(&week_all) {
        UsageState::WeeklyExhausted
    } else {
        UsageState::Ok
    };
    Some(UsageReport {
        observation: Default::default(),
        state,
        session,
        week_all,
        week_models: Vec::new(),
        session_limit: !reading.no_session_window,
        observed_at: Some(reading.observed_at),
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

/// Current-generation observations have no age-based probe schedule. Once a
/// generation is registered, legacy readings can never restore retired data.
#[cfg(unix)]
fn generation_reading(root: &Path, dir: &ProfileName) -> Option<UsageReport> {
    use amplihack_utils::usage_observation::{ObservationStore, ReadOutcome};
    let result = (|| -> Result<Option<UsageReport>, super::ProfileError> {
        let state = super::auto_switch::kit::State::new(root)?;
        let _lock = state.lock()?;
        let store = ObservationStore::open(root)?;
        let mut guard = store.under_state_lock();
        let ledger = state.load_ledger()?;
        let account = ledger.account_in(dir.as_str());
        let Some(binding) = guard.binding(account)? else {
            return Ok(None);
        };
        if guard.retire_if_changed(&binding)? {
            return Ok(None);
        }
        let ReadOutcome::Usable(record) = guard.read(&binding)? else {
            return Ok(None);
        };
        let e = record.evidence;
        let window = |used_percentage, resets_at, label: &str| UsageWindow {
            used_percentage,
            resets_at,
            label: label.into(),
            resets_text: None,
        };
        let session = e
            .session_used
            .map(|p| window(p, e.session_reset_at, "session"));
        let week_all = e
            .week_used
            .map(|p| window(p, e.week_reset_at, "all models"));
        let state = if e.limit_reached || e.session_used.is_some_and(|p| p >= 100.0) {
            UsageState::SessionExhausted
        } else if e.week_used.is_some_and(|p| p >= 100.0) {
            UsageState::WeeklyExhausted
        } else {
            UsageState::Ok
        };
        Ok(Some(UsageReport {
            observation: e.clone(),
            state,
            session,
            week_all,
            week_models: Vec::new(),
            session_limit: e.session_window
                != amplihack_utils::usage_observation::SessionWindow::Absent,
            observed_at: Some(record.capture.captured_at),
        }))
    })();
    match result {
        Ok(report) => report,
        Err(e) => {
            tracing::warn!("passive usage observation refused: {e}");
            None
        }
    }
}
