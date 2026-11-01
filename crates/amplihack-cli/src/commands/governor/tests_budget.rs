//! Issue #1811 (TDD): budgets in percentage points and the weekly-only rule
//! (D5, D7, D12).
//!
//! Wire with `#[cfg(test)] mod tests_budget;` in `governor/mod.rs`.
//!
//! Expected API:
//! - `profile::governor_view::ProfileSnapshot { name: String, active: bool,
//!   eligible: bool, five_hour: Option<(f64, i64)>, seven_day:
//!   Option<(f64, i64)>, observed_at: i64, no_session_window: bool }` (no
//!   `Debug`, no `Serialize`).
//! - `budget::budget_from_snapshot(&ProfileSnapshot) -> Option<Budget>`:
//!   `None` when there is no usable weekly window, or when a session window
//!   was read but cannot be trusted; only a `no_session_window` reading gives
//!   `session: None`.
//! - `budget::usable_points(&Budget) -> f64`: `min(session rem, weekly rem)`.
//! - `budget::headroom(&Budget, reserve_points: f64) -> Headroom` with
//!   `Headroom::{Room, SessionExhausted, WeeklyExhausted, InReserve}`.

use super::budget::{Headroom, budget_from_snapshot, headroom, invalid_reading, usable_points};
use crate::commands::profile::governor_view::ProfileSnapshot;

const NOW: i64 = 1_800_000_000;

fn snap(five: Option<(f64, i64)>, seven: Option<(f64, i64)>) -> ProfileSnapshot {
    ProfileSnapshot {
        name: "alpha".to_string(),
        active: true,
        eligible: true,
        no_session_window: seven.is_some() && five.is_none(),
        five_hour: five,
        seven_day: seven,
        observed_at: NOW,
    }
}

#[test]
fn both_windows_give_a_budget_with_a_session() {
    let b = budget_from_snapshot(&snap(Some((30.0, NOW + 100)), Some((60.0, NOW + 900)))).unwrap();
    assert_eq!(b.name, "alpha");
    assert!(b.active && b.eligible);
    let s = b.session.expect("session window");
    assert_eq!((s.used_pct, s.resets_at), (30.0, Some(NOW + 100)));
    assert_eq!(
        (b.weekly.used_pct, b.weekly.resets_at),
        (60.0, Some(NOW + 900))
    );
}

#[test]
fn weekly_only_reading_has_no_session() {
    let b = budget_from_snapshot(&snap(None, Some((40.0, NOW + 900)))).unwrap();
    assert!(b.session.is_none());
    assert_eq!(usable_points(&b), 60.0);
}

#[test]
fn reading_with_neither_window_is_unknown() {
    assert!(budget_from_snapshot(&snap(None, None)).is_none());
}

#[test]
fn eligibility_is_carried_through() {
    let mut s = snap(None, Some((10.0, NOW + 900)));
    s.eligible = false;
    s.active = false;
    let b = budget_from_snapshot(&s).unwrap();
    assert!(!b.eligible && !b.active);
}

#[test]
fn invalid_weekly_percentages_are_unknown() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5, 100.5, 1e9] {
        assert!(
            budget_from_snapshot(&snap(Some((10.0, NOW)), Some((bad, NOW + 900)))).is_none(),
            "{bad}"
        );
    }
}

/// Crusty round 1 on #1831: a damaged session reading is "unknown", as a
/// damaged weekly one is. It never yields a weekly-only budget, which would
/// drop the session limit, and `governor status` reports it.
#[test]
fn invalid_session_percentages_are_unknown_not_weekly_only() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5, 100.5, 1e9] {
        let s = snap(Some((bad, NOW)), Some((20.0, NOW + 900)));
        assert!(!s.no_session_window);
        assert!(budget_from_snapshot(&s).is_none(), "{bad}");
        assert!(invalid_reading(&s), "{bad}");
    }
}

#[test]
fn invalid_reading_is_only_a_reading_that_cannot_be_used() {
    assert!(
        !invalid_reading(&snap(None, None)),
        "no reading is not invalid"
    );
    assert!(!invalid_reading(&snap(None, Some((20.0, NOW + 900)))));
    assert!(!invalid_reading(&snap(
        Some((20.0, NOW)),
        Some((20.0, NOW + 900))
    )));
    assert!(invalid_reading(&snap(
        Some((20.0, NOW)),
        Some((f64::NAN, NOW + 900))
    )));
    assert!(
        !invalid_reading(&snap(Some((20.0, NOW)), None)),
        "a missing week is no reading"
    );
    assert!(budget_from_snapshot(&snap(Some((20.0, NOW)), None)).is_none());
}

#[test]
fn usable_points_is_the_smaller_remainder() {
    let b = budget_from_snapshot(&snap(Some((10.0, NOW + 120)), Some((99.0, NOW + 900)))).unwrap();
    assert_eq!(usable_points(&b), 1.0);
    let b = budget_from_snapshot(&snap(Some((80.0, NOW + 120)), Some((30.0, NOW + 900)))).unwrap();
    assert_eq!(usable_points(&b), 20.0);
}

#[test]
fn headroom_classifies_each_case() {
    let h = |five: Option<f64>, seven: f64, reserve: f64| {
        let b = budget_from_snapshot(&snap(five.map(|p| (p, NOW + 60)), Some((seven, NOW + 900))))
            .unwrap();
        headroom(&b, reserve)
    };
    assert_eq!(h(Some(10.0), 50.0, 10.0), Headroom::Room);
    assert_eq!(h(Some(100.0), 50.0, 10.0), Headroom::SessionExhausted);
    assert_eq!(h(Some(10.0), 100.0, 10.0), Headroom::WeeklyExhausted);
    assert_eq!(h(Some(100.0), 100.0, 10.0), Headroom::WeeklyExhausted);
    assert_eq!(h(Some(10.0), 90.0, 10.0), Headroom::InReserve);
    assert_eq!(h(Some(10.0), 90.0, 0.0), Headroom::Room);
    // The reserve is weekly only: a nearly spent session with plenty of week
    // left is not held in reserve.
    assert_eq!(h(Some(92.0), 40.0, 10.0), Headroom::Room);
    assert_eq!(h(Some(99.0), 40.0, 10.0), Headroom::Room);
    assert_eq!(h(None, 100.0, 0.0), Headroom::WeeklyExhausted);
    assert_eq!(h(None, 50.0, 0.0), Headroom::Room);
}

#[test]
fn weekly_only_profile_is_never_session_exhausted() {
    for used in [0.0, 50.0, 99.9] {
        let b = budget_from_snapshot(&snap(None, Some((used, NOW + 900)))).unwrap();
        assert_ne!(headroom(&b, 0.0), Headroom::SessionExhausted);
    }
}

#[test]
fn reset_time_never_raises_points_above_the_weekly_remainder() {
    // A session that has already reset (resets_at in the past) is still
    // capped by the weekly remainder.
    let b = budget_from_snapshot(&snap(Some((0.0, NOW - 10)), Some((97.0, NOW + 900)))).unwrap();
    assert!(usable_points(&b) <= 3.0, "{}", usable_points(&b));
}
