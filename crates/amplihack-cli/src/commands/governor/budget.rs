//! Budgets in percentage points (D5, D7, D12). A profile's usable points are
//! the smaller of its session and weekly remainders, so a session that is
//! about to reset never adds capacity beyond what the week has left.

use super::model::{Budget, WindowBudget};
use crate::commands::profile::governor_view::{ProfileSnapshot, plausible};

/// Why a profile can or cannot take a start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Headroom {
    Room,
    SessionExhausted,
    WeeklyExhausted,
    /// The weekly remainder is at or below the protected reserve (D7). The
    /// session remainder never counts towards the reserve: a busy session
    /// resets within hours and is handled by session exhaustion instead.
    InReserve,
}

/// A window the governor can trust, by the rotator's own rule
/// ([`plausible`], `usage::plausible` from issue #1844) applied at the time
/// the reading was observed: a finite percentage, clamped to `0..=100`, and a
/// reset no more than one day before or eight days after `observed_at`. A
/// reading that was plausible when taken stays usable after its reset has
/// passed; [`super::inputs::budgets`] then treats the window as rolled over.
pub(super) fn window(raw: Option<(f64, i64)>, observed_at: i64) -> Option<WindowBudget> {
    let (used_pct, resets_at) = raw?;
    plausible(used_pct, resets_at, observed_at).map(|(used_pct, resets_at)| WindowBudget {
        used_pct,
        resets_at: Some(resets_at),
    })
}

/// The budget for one profile, or `None` when the profile is "unknown" and
/// not part of the pool. A profile is unknown without a usable weekly window,
/// and also when a session window was read but fails [`window`] (a
/// non-finite percentage or an absurd reset time): a damaged session reading never drops
/// the session limit. Only a reading that [`ProfileSnapshot::no_session_window`]
/// marks (a week window and no session window, issue #1818) gives a
/// weekly-only budget (`session: None`).
pub fn budget_from_snapshot(s: &ProfileSnapshot) -> Option<Budget> {
    let session = if s.no_session_window {
        None
    } else {
        Some(window(s.five_hour, s.observed_at)?)
    };
    Some(Budget {
        name: s.name.clone(),
        active: s.active,
        eligible: s.eligible,
        weekly: window(s.seven_day, s.observed_at)?,
        session,
    })
}

/// A window of the profile's reading fails [`window`]: its percentage is not
/// finite or its reset is more than one day before or eight days after the
/// time it was observed. [`budget_from_snapshot`] leaves the profile out of
/// the pool. A percentage outside `0..=100` is clamped, not invalid: 150%
/// reads as an exhausted 100%, as it does for the rotator. `governor status` shows it as
/// `invalid reading`, not `no reading`.
pub fn invalid_reading(s: &ProfileSnapshot) -> bool {
    [s.five_hour, s.seven_day]
        .into_iter()
        .flatten()
        .any(|w| window(Some(w), s.observed_at).is_none())
}

/// The profile has a reading that is too old to use: it was observed more
/// than `max_age` seconds before `now`, or after `now`, the same age test as
/// the rotator's `fresh_reading` (issue #1844). Use only rises until a
/// window resets, so an old reading is a lower bound on use:
/// [`super::inputs::budgets`] keeps the profile only while one of its
/// windows is exhausted with the reset still ahead, and otherwise leaves it
/// out of the pool. A profile without a reading is never stale.
/// `governor status` shows it as `stale reading`.
pub fn stale_reading(s: &ProfileSnapshot, max_age: u64, now: i64) -> bool {
    if s.five_hour.is_none() && s.seven_day.is_none() {
        return false;
    }
    let max_age = i64::try_from(max_age).unwrap_or(i64::MAX);
    !now.checked_sub(s.observed_at)
        .is_some_and(|age| (0..=max_age).contains(&age))
}

fn remaining(w: &WindowBudget) -> f64 {
    (100.0 - w.used_pct).clamp(0.0, 100.0)
}

/// `min(session remainder, weekly remainder)`, in points.
pub fn usable_points(b: &Budget) -> f64 {
    let weekly = remaining(&b.weekly);
    b.session
        .as_ref()
        .map_or(weekly, |s| weekly.min(remaining(s)))
}

/// The weekly remainder alone, in points.
pub fn weekly_remaining(b: &Budget) -> f64 {
    remaining(&b.weekly)
}

/// Classifies one profile against `reserve_points` (0 for protected work).
pub fn headroom(b: &Budget, reserve_points: f64) -> Headroom {
    if b.weekly.used_pct >= 100.0 {
        return Headroom::WeeklyExhausted;
    }
    if b.session.is_some_and(|s| s.used_pct >= 100.0) {
        return Headroom::SessionExhausted;
    }
    if weekly_remaining(b) <= reserve_points {
        return Headroom::InReserve;
    }
    Headroom::Room
}
