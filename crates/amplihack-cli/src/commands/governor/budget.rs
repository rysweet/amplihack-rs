//! Budgets in percentage points (D5, D7, D12). A profile's usable points are
//! the smaller of its session and weekly remainders, so a session that is
//! about to reset never adds capacity beyond what the week has left.

use super::model::{Budget, WindowBudget};
use crate::commands::profile::governor_view::ProfileSnapshot;

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

/// A percentage the governor can trust: finite and within `0..=100`.
fn window(raw: Option<(f64, i64)>) -> Option<WindowBudget> {
    let (used_pct, resets_at) = raw?;
    (used_pct.is_finite() && (0.0..=100.0).contains(&used_pct)).then_some(WindowBudget {
        used_pct,
        resets_at: Some(resets_at),
    })
}

/// The budget for one profile, or `None` when the profile is "unknown" and
/// not part of the pool. A profile is unknown without a usable weekly window,
/// and also when a session window was read but cannot be trusted
/// (non-finite or outside `0..=100`): a damaged session reading never drops
/// the session limit. Only a reading that [`ProfileSnapshot::no_session_window`]
/// marks (a week window and no session window, issue #1818) gives a
/// weekly-only budget (`session: None`).
pub fn budget_from_snapshot(s: &ProfileSnapshot) -> Option<Budget> {
    let session = if s.no_session_window {
        None
    } else {
        Some(window(s.five_hour)?)
    };
    Some(Budget {
        name: s.name.clone(),
        active: s.active,
        eligible: s.eligible,
        weekly: window(s.seven_day)?,
        session,
    })
}

/// A window of the profile's reading holds a percentage the governor cannot
/// trust (non-finite or outside `0..=100`), so [`budget_from_snapshot`]
/// leaves the profile out of the pool. `governor status` shows it as
/// `invalid reading`, not `no reading`.
pub fn invalid_reading(s: &ProfileSnapshot) -> bool {
    [s.five_hour, s.seven_day]
        .into_iter()
        .flatten()
        .any(|w| window(Some(w)).is_none())
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
