//! Issue #2211: the pure parts of live reading freshness (`fresh::live`, in
//! `live_fresh.rs`), the `projected` reason and the target gate.
//!
//! The contract these tests fix for `fresh::live`:
//!
//! - `Series::default()`; `note(&mut self, name, at, session, t)` adds the
//!   point `(at, session)` for live login `name` at clock `t`. It ignores a
//!   non-finite session, `at <= last.at` and `at > t`; it clears the series
//!   first when `name` differs from the series' login or `session` falls
//!   below the last point. It keeps at most `MAX_POINTS` (4) points.
//!   `clear()`, `len()`, and `rate() -> Option<f64>` (points per second
//!   from the first to the last point; `None` for fewer than 2 points, a
//!   time difference of 0 or a non-finite result).
//! - `projected(session, rate, interval) -> f64`: `session + rate *
//!   interval`, clamped to `[0, 100]`.
//! - `near(last, rate, &Settings) -> bool`: with a rate,
//!   `last + rate * interval >= session_at`; with none,
//!   `last >= session_at - margin`.
//! - `map_reason(decision, Basis, &Settings, &real_reading) -> Decision`:
//!   relabels only `Swap{Threshold}`: always for `Basis::Substituted`, and
//!   for `Basis::Projected` only when the real reading alone needs no swap.
//!   `Basis::Real` changes nothing.
//!
//! No test reads a file, a login or the clock.

use super::super::decide::{Decision, Reading, Reason};
use super::super::fresh::gated_target;
use super::super::fresh::live::{
    Basis, FROZEN_CYCLES, MAX_POINTS, Series, map_reason, near, projected,
};
use super::super::settings::{self, Settings};
use crate::commands::profile::name::ProfileName;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

/// `--interval 120 --session-at 70 --margin 5`, other settings default.
fn s() -> Settings {
    let mut s = settings::resolve(&super::super::TuneFlags::default(), None, &|_| None).unwrap();
    s.interval = 120;
    s.session_at = 70;
    s.margin = 5;
    s
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

const T0: i64 = 1_793_000_000;

// --- Reason::Projected ------------------------------------------------------

#[test]
fn projected_is_a_reason_named_projected() {
    assert_eq!(Reason::Projected.as_str(), "projected");
}

#[test]
fn the_target_gate_applies_to_a_projected_swap() {
    let d = Decision::Swap {
        to: n("spare"),
        reason: Reason::Projected,
    };
    assert_eq!(gated_target(&d), Some(&n("spare")));
}

#[test]
fn the_limits_are_four_points_and_two_unchanged_cycles() {
    assert_eq!(MAX_POINTS, 4);
    assert_eq!(FROZEN_CYCLES, 2);
}

// --- Series and rate ---------------------------------------------------------

#[test]
fn an_empty_or_single_point_series_has_no_rate() {
    let mut sr = Series::default();
    assert_eq!(sr.rate(), None);
    sr.note(&n("kkane"), T0, 22.0, T0);
    assert_eq!(sr.len(), 1);
    assert_eq!(sr.rate(), None);
}

#[test]
fn the_rate_is_from_the_first_to_the_last_point() {
    let mut sr = Series::default();
    sr.note(&n("kkane"), T0, 50.0, T0);
    sr.note(&n("kkane"), T0 + 120, 63.0, T0 + 120);
    let r = sr.rate().unwrap();
    assert!(close(r * 120.0, 13.0), "{r}");
    assert!(close(projected(63.0, r, 120), 76.0));
}

#[test]
fn the_kkane_history_projects_59_3_at_50_and_73_3_at_63() {
    let mut sr = Series::default();
    for (i, v) in [22.0, 32.0, 42.0, 50.0].into_iter().enumerate() {
        let at = T0 + 120 * i as i64;
        sr.note(&n("kkane"), at, v, at);
    }
    let p50 = projected(50.0, sr.rate().unwrap(), 120);
    assert!(close(p50, 59.3), "{p50}");
    assert!(p50 < 70.0);
    sr.note(&n("kkane"), T0 + 480, 63.0, T0 + 480);
    assert_eq!(sr.len(), MAX_POINTS, "the oldest point (22) is dropped");
    let p63 = projected(63.0, sr.rate().unwrap(), 120);
    assert!(close(p63, 73.3), "{p63}");
    assert!(p63 >= 70.0);
}

#[test]
fn a_slow_rise_projects_below_the_threshold() {
    let mut sr = Series::default();
    sr.note(&n("kkane"), T0, 20.0, T0);
    sr.note(&n("kkane"), T0 + 120, 22.0, T0 + 120);
    assert!(close(projected(22.0, sr.rate().unwrap(), 120), 24.0));
}

#[test]
fn a_fall_in_session_starts_a_new_series() {
    let mut sr = Series::default();
    sr.note(&n("kkane"), T0, 60.0, T0);
    sr.note(&n("kkane"), T0 + 120, 65.0, T0 + 120);
    sr.note(&n("kkane"), T0 + 240, 3.0, T0 + 240);
    assert_eq!(sr.len(), 1, "a new session window");
    assert_eq!(sr.rate(), None);
}

#[test]
fn another_live_login_starts_a_new_series() {
    let mut sr = Series::default();
    sr.note(&n("kkane"), T0, 50.0, T0);
    sr.note(&n("llowe"), T0 + 120, 63.0, T0 + 120);
    assert_eq!(sr.len(), 1);
    assert_eq!(sr.rate(), None, "no rate across two logins");
}

#[test]
fn clear_empties_the_series() {
    let mut sr = Series::default();
    sr.note(&n("kkane"), T0, 50.0, T0);
    sr.note(&n("kkane"), T0 + 120, 63.0, T0 + 120);
    sr.clear();
    assert_eq!(sr.len(), 0);
    assert_eq!(sr.rate(), None);
}

#[test]
fn a_point_not_later_than_the_last_is_ignored() {
    let mut sr = Series::default();
    sr.note(&n("kkane"), T0 + 120, 50.0, T0 + 120);
    sr.note(&n("kkane"), T0 + 120, 63.0, T0 + 120);
    sr.note(&n("kkane"), T0, 63.0, T0 + 120);
    assert_eq!(sr.len(), 1);
    assert_eq!(sr.rate(), None);
}

#[test]
fn a_point_from_the_future_is_ignored() {
    let mut sr = Series::default();
    sr.note(&n("kkane"), T0, 50.0, T0);
    sr.note(&n("kkane"), T0 + 600, 90.0, T0 + 120);
    assert_eq!(sr.len(), 1);
}

#[test]
fn a_non_finite_session_is_ignored() {
    let mut sr = Series::default();
    sr.note(&n("kkane"), T0, 50.0, T0);
    sr.note(&n("kkane"), T0 + 120, f64::NAN, T0 + 120);
    sr.note(&n("kkane"), T0 + 240, f64::INFINITY, T0 + 240);
    assert_eq!(sr.len(), 1);
    assert_eq!(sr.rate(), None);
}

#[test]
fn the_projection_is_clamped_to_0_and_100() {
    assert_eq!(projected(90.0, 1.0, 120), 100.0);
    assert_eq!(projected(10.0, -1.0, 120), 0.0);
}

// --- Near the threshold ----------------------------------------------------

#[test]
fn near_with_a_rate_uses_one_interval_of_the_rate() {
    let s = s();
    assert!(near(64.0, Some(6.0 / 120.0), &s), "64 + 6 = 70");
    assert!(!near(64.0, Some(4.0 / 120.0), &s), "64 + 4 = 68");
}

#[test]
fn near_with_no_rate_uses_the_margin() {
    let s = s();
    assert!(near(65.0, None, &s), "70 - 5 = 65");
    assert!(near(66.0, None, &s));
    assert!(!near(64.0, None, &s));
}

// --- map_reason ------------------------------------------------------------

fn threshold() -> Decision {
    Decision::Swap {
        to: n("spare"),
        reason: Reason::Threshold,
    }
}

fn projected_swap() -> Decision {
    Decision::Swap {
        to: n("spare"),
        reason: Reason::Projected,
    }
}

fn m(session: f64, week: f64) -> Reading {
    Reading::Measured { session, week }
}

#[test]
fn a_threshold_swap_on_a_projection_below_the_real_threshold_is_projected() {
    let got = map_reason(threshold(), Basis::Projected, &s(), &m(63.0, 41.0));
    assert_eq!(got, projected_swap());
}

#[test]
fn a_projection_when_the_real_reading_already_needs_a_swap_stays_threshold() {
    let got = map_reason(threshold(), Basis::Projected, &s(), &m(75.0, 41.0));
    assert_eq!(got, threshold(), "the real reading is at the threshold");
    let got = map_reason(threshold(), Basis::Projected, &s(), &m(10.0, 95.0));
    assert_eq!(got, threshold(), "the week is past --week-at");
}

#[test]
fn a_substituted_reading_always_gives_projected() {
    for real in [Reading::Failed, m(66.0, 41.0)] {
        let got = map_reason(threshold(), Basis::Substituted, &s(), &real);
        assert_eq!(got, projected_swap(), "{real:?}");
    }
}

#[test]
fn a_real_basis_changes_nothing() {
    let got = map_reason(threshold(), Basis::Real, &s(), &m(63.0, 41.0));
    assert_eq!(got, threshold());
}

#[test]
fn map_reason_never_creates_or_suppresses_a_swap_or_relabels_other_reasons() {
    let s = s();
    let real = m(63.0, 41.0);
    for basis in [Basis::Real, Basis::Projected, Basis::Substituted] {
        assert_eq!(map_reason(Decision::Stay, basis, &s, &real), Decision::Stay);
        assert_eq!(
            map_reason(Decision::Alert, basis, &s, &real),
            Decision::Alert
        );
        for reason in [
            Reason::Upgrade,
            Reason::FailedProbe,
            Reason::OverflowIn,
            Reason::OverflowOut,
            Reason::PlanRoom,
        ] {
            let d = Decision::Swap {
                to: n("spare"),
                reason,
            };
            assert_eq!(map_reason(d.clone(), basis, &s, &real), d, "{reason:?}");
        }
    }
}
