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
    /// The usable points are at or below the protected reserve.
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

/// The budget for one profile. `None` without a usable weekly window: such a
/// profile is "unknown" and not part of the pool. A missing or invalid
/// session window gives a weekly-only budget (`session: None`).
pub fn budget_from_snapshot(s: &ProfileSnapshot) -> Option<Budget> {
    Some(Budget {
        name: s.name.clone(),
        active: s.active,
        eligible: s.eligible,
        weekly: window(s.seven_day)?,
        session: window(s.five_hour),
    })
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
    if usable_points(b) <= reserve_points {
        return Headroom::InReserve;
    }
    Headroom::Room
}
