//! Issue #2211: the pure parts of live reading freshness (`fresh::live`, in
//! `live_fresh.rs`), the `projected` reason and the target gate.
//!
//! The contract these tests fix for `fresh::live`:
//!
//! - `Series::default()`; `note(&mut self, name, at, session, resets, t)`
//!   adds the point `(at, session)`, whose session window resets at
//!   `resets`, for live login `name` at clock `t`. It ignores a non-finite
//!   session, `at <= last.at` and `at > t`. It clears the series first when
//!   `name` differs from the series' login, or when `window(last.resets,
//!   resets)` is `Later`. It ignores a figure from an `Earlier` window and a
//!   lower figure in the `Same` window: use never falls within a window.
//!   With an `Unknown` window a lower figure clears the series (the step
//!   reads a lower cached figure again first, so only a direct read gets
//!   there). It keeps at most `MAX_POINTS` (4) points.
//! - `window(last, new) -> Window`: `Later` when both reset times are known
//!   and `new` is more than `SAME_WINDOW_SECS` (3600) after `last`,
//!   `Earlier` when more than that before, `Same` otherwise, `Unknown` when
//!   either is missing. `contradicts(&last, session, resets)`: an `Earlier`
//!   window, or a lower figure in the `Same` or an `Unknown` window.
//! - `last_known(name, t) -> Option<Point>`: the last point for `name` while
//!   its window can still be open at `t`: before its reset time when known,
//!   and less than `SESSION_WINDOW_SECS` (5 h) after it was read.
//!   `clear()`, `len()`, and `rate(min_span) -> Option<f64>` (points per
//!   second from the first to the last point; `None` for fewer than 2
//!   points, a time difference of 0 or less, a time difference below
//!   `min_span` seconds, or a non-finite result). The rotator passes
//!   `--interval` as `min_span`, for the projection and for `near` alike.
//! - `projected(session, rate, interval) -> f64`: `session + rate *
//!   interval`, clamped to `[0, 100]`.
//! - `near(last, rate, &Settings) -> bool`: `last >= session_at - margin`,
//!   or, with a rate, `last + rate * interval >= session_at`. A rate only
//!   widens the margin band: a rate of 0 from frozen points never narrows it.
//! - `map_reason(decision, Basis, &Settings, &real_reading) -> Decision`:
//!   relabels only `Swap{Threshold}`, and only when the real reading alone
//!   needs no swap; under `Basis::Substituted` a failed real reading counts
//!   as needing none. There is no basis for the reading as measured: with
//!   no outlook, nothing is relabelled.
//! - `with_outlook(real_verdict, on_outlook, Basis, &Settings, &real_reading)
//!   -> Verdict`: for a failed reading, `real_verdict` (the failed-probe
//!   rules' threshold swap, which damping delays, or their ALERT), with its
//!   threshold swap relabelled by `map_reason` (so undamped) only when the
//!   outlook gives a threshold swap to that same target; a swap the outlook
//!   names to another login, and an ALERT on the outlook, never replace it
//!   (crusty round 8: another login's headroom says nothing about `real`'s
//!   target). The caller decides the outlook under organisation affinity
//!   with the cycle's one need (`observe::affinity::outlook`,
//!   `outlook_failed`); `tests_issue_2211_cycle` checks that in two
//!   organisations. A measured reading at a threshold, or one that gives a
//!   swap on its own, keeps `real_verdict`. Otherwise (below both
//!   thresholds) a threshold swap on the outlook, labelled by `map_reason`,
//!   or the ALERT; only a stay on the outlook, which organisation affinity
//!   gives when just a confirmed crossing has capacity, keeps
//!   `real_verdict`. It never returns a plan-room or overflow
//!   swap that `real_verdict` did not already hold, and never retargets.
//! - `fresh::live_time(&live_probe, measured_at) -> Option<i64>`: the live
//!   reading time the target gate compares with, `read_at` capped at
//!   `measured_at`, so a live re-read made after the measurement never makes
//!   the measurement's own target readings stale.
//!
//! Pure tests use fixed timestamps; the regression fixtures below use only
//! temporary homes and synthetic credentials, with explicitly advanced clocks.

use super::super::decide::{Decision, Reading, Reason};
use super::super::fresh::gated_target;
use super::super::fresh::live::{
    Basis, FROZEN_CYCLES, MAX_POINTS, Point, SAME_WINDOW_SECS, SESSION_WINDOW_SECS, Series, Window,
    contradicts, map_reason, near, projected, window, with_outlook,
};
use super::super::overflow::{Tag, Verdict};
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
    assert_eq!(sr.rate(120), None);
    sr.note(&n("work"), T0, 22.0, None, T0);
    assert_eq!(sr.len(), 1);
    assert_eq!(sr.rate(120), None);
}

#[test]
fn the_rate_is_from_the_first_to_the_last_point() {
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 50.0, None, T0);
    sr.note(&n("work"), T0 + 120, 63.0, None, T0 + 120);
    let r = sr.rate(120).unwrap();
    assert!(close(r * 120.0, 13.0), "{r}");
    assert!(close(projected(63.0, r, 120), 76.0));
}

#[test]
fn a_rate_needs_points_at_least_the_minimum_span_apart() {
    // A direct 50% at T, then a status line at 51% written five seconds
    // later: 0.2 points a second, which would project 51 + 24 = 75.
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 50.0, None, T0);
    sr.note(&n("work"), T0 + 5, 51.0, None, T0 + 120);
    assert_eq!(sr.len(), 2);
    assert_eq!(sr.rate(120), None, "5 s is less than one interval");
    assert!(sr.rate(5).is_some(), "the same points over a 5 s minimum");
    sr.note(&n("work"), T0 + 119, 52.0, None, T0 + 120);
    assert_eq!(sr.rate(120), None, "119 s is still less than one interval");
    sr.note(&n("work"), T0 + 120, 52.0, None, T0 + 120);
    let r = sr.rate(120).unwrap();
    assert!(close(r * 120.0, 2.0), "{r}");
}

#[test]
fn over_one_interval_the_projection_adds_no_more_than_the_rise_seen() {
    for (dt, rise) in [(120, 13.0), (121, 40.0), (360, 31.0), (600, 5.0)] {
        let mut sr = Series::default();
        sr.note(&n("work"), T0, 30.0, None, T0);
        sr.note(&n("work"), T0 + dt, 30.0 + rise, None, T0 + dt);
        let r = sr.rate(120).unwrap();
        let p = projected(30.0 + rise, r, 120);
        assert!(p <= 30.0 + 2.0 * rise + 1e-9, "dt={dt} rise={rise}: {p}");
    }
}

#[test]
fn the_rising_history_projects_59_3_at_50_and_73_3_at_63() {
    let mut sr = Series::default();
    for (i, v) in [22.0, 32.0, 42.0, 50.0].into_iter().enumerate() {
        let at = T0 + 120 * i as i64;
        sr.note(&n("work"), at, v, None, at);
    }
    let p50 = projected(50.0, sr.rate(120).unwrap(), 120);
    assert!(close(p50, 59.3), "{p50}");
    assert!(p50 < 70.0);
    sr.note(&n("work"), T0 + 480, 63.0, None, T0 + 480);
    assert_eq!(sr.len(), MAX_POINTS, "the oldest point (22) is dropped");
    let p63 = projected(63.0, sr.rate(120).unwrap(), 120);
    assert!(close(p63, 73.3), "{p63}");
    assert!(p63 >= 70.0);
}

#[test]
fn a_slow_rise_projects_below_the_threshold() {
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 20.0, None, T0);
    sr.note(&n("work"), T0 + 120, 22.0, None, T0 + 120);
    assert!(close(projected(22.0, sr.rate(120).unwrap(), 120), 24.0));
}

#[test]
fn a_fall_with_no_reset_time_to_compare_starts_a_new_series() {
    // Only a direct read gets here with a lower figure: the step reads a
    // lower cached figure again first.
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 60.0, None, T0);
    sr.note(&n("work"), T0 + 120, 65.0, None, T0 + 120);
    sr.note(&n("work"), T0 + 240, 3.0, None, T0 + 240);
    assert_eq!(sr.len(), 1, "a new session window");
    assert_eq!(sr.rate(120), None);
}

/// A session reset time in these tests.
const R: i64 = T0 + 3 * 3600;

#[test]
fn windows_compare_by_session_reset_time() {
    assert_eq!(SAME_WINDOW_SECS, 3600);
    assert_eq!(SESSION_WINDOW_SECS, 5 * 3600);
    assert_eq!(window(Some(R), Some(R)), Window::Same);
    // A direct read prints the reset to the minute or the hour.
    assert_eq!(window(Some(R), Some(R - 59 * 60)), Window::Same);
    assert_eq!(window(Some(R), Some(R + SAME_WINDOW_SECS)), Window::Same);
    assert_eq!(
        window(Some(R), Some(R + SAME_WINDOW_SECS + 1)),
        Window::Later
    );
    assert_eq!(window(Some(R), Some(R + 5 * 3600)), Window::Later);
    assert_eq!(
        window(Some(R), Some(R - SAME_WINDOW_SECS - 1)),
        Window::Earlier
    );
    assert_eq!(window(None, Some(R)), Window::Unknown);
    assert_eq!(window(Some(R), None), Window::Unknown);
    assert_eq!(window(None, None), Window::Unknown);
}

#[test]
fn a_lower_figure_contradicts_the_last_point_unless_its_window_is_later() {
    let last = Point {
        at: T0,
        session: 66.0,
        resets: Some(R),
    };
    assert!(contradicts(&last, 64.0, Some(R)), "same window, lower");
    assert!(contradicts(&last, 64.0, None), "unknown window, lower");
    assert!(!contradicts(&last, 66.0, Some(R)), "caught up");
    assert!(!contradicts(&last, 69.0, None), "higher, unknown window");
    assert!(!contradicts(&last, 3.0, Some(R + 5 * 3600)), "a new window");
    assert!(
        contradicts(&last, 90.0, Some(R - 5 * 3600)),
        "an ended window, whatever its figure"
    );
}

#[test]
fn a_lower_figure_in_the_same_window_is_stale_and_kept_out() {
    // Crusty round 7: a status line rewrites 64% with a new time after a
    // direct 66%. Use never falls within one window.
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 63.0, Some(R), T0);
    sr.note(&n("work"), T0 + 120, 66.0, Some(R), T0 + 120);
    sr.note(&n("work"), T0 + 240, 64.0, Some(R), T0 + 240);
    assert_eq!(sr.len(), 2, "not a new window, and not a point");
    let r = sr.rate(120).unwrap();
    assert!(close(r * 120.0, 3.0), "{r}");
    // A direct read's reset to the hour is the same window.
    sr.note(&n("work"), T0 + 360, 65.0, Some(R - 1800), T0 + 360);
    assert_eq!(sr.len(), 2);
    sr.note(&n("work"), T0 + 480, 69.0, Some(R - 1800), T0 + 480);
    assert_eq!(sr.len(), 3, "a higher figure in the same window");
}

#[test]
fn a_later_session_reset_starts_a_new_series_whatever_the_figure() {
    for next in [3.0, 80.0] {
        let mut sr = Series::default();
        sr.note(&n("work"), T0, 60.0, Some(R), T0);
        sr.note(&n("work"), T0 + 120, 65.0, Some(R), T0 + 120);
        sr.note(&n("work"), T0 + 240, next, Some(R + 5 * 3600), T0 + 240);
        assert_eq!(sr.len(), 1, "{next}: a new session window");
        assert_eq!(sr.rate(120), None);
    }
}

#[test]
fn a_figure_from_an_earlier_window_is_ignored() {
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 5.0, Some(R + 5 * 3600), T0);
    sr.note(&n("work"), T0 + 120, 80.0, Some(R), T0 + 120);
    assert_eq!(sr.len(), 1);
}

#[test]
fn the_last_known_point_counts_only_while_its_window_can_be_open() {
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 66.0, Some(T0 + 600), T0);
    assert!(sr.last_known(&n("work"), T0 + 599).is_some());
    assert_eq!(
        sr.last_known(&n("work"), T0 + 600),
        None,
        "its window reset"
    );
    assert_eq!(sr.last_known(&n("spare"), T0), None, "another login's");
    // With no reset time, one session window at most.
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 66.0, None, T0);
    let end = T0 + SESSION_WINDOW_SECS;
    assert!(sr.last_known(&n("work"), end - 1).is_some());
    assert_eq!(sr.last_known(&n("work"), end), None);
}

#[test]
fn another_live_login_starts_a_new_series() {
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 50.0, None, T0);
    sr.note(&n("spare"), T0 + 120, 63.0, None, T0 + 120);
    assert_eq!(sr.len(), 1);
    assert_eq!(sr.rate(120), None, "no rate across two logins");
}

#[test]
fn clear_empties_the_series() {
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 50.0, None, T0);
    sr.note(&n("work"), T0 + 120, 63.0, None, T0 + 120);
    sr.clear();
    assert_eq!(sr.len(), 0);
    assert_eq!(sr.rate(120), None);
}

#[test]
fn a_point_not_later_than_the_last_is_ignored() {
    let mut sr = Series::default();
    sr.note(&n("work"), T0 + 120, 50.0, None, T0 + 120);
    sr.note(&n("work"), T0 + 120, 63.0, None, T0 + 120);
    sr.note(&n("work"), T0, 63.0, None, T0 + 120);
    assert_eq!(sr.len(), 1);
    assert_eq!(sr.rate(120), None);
}

#[test]
fn a_point_from_the_future_is_ignored() {
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 50.0, None, T0);
    sr.note(&n("work"), T0 + 600, 90.0, None, T0 + 120);
    assert_eq!(sr.len(), 1);
}

#[test]
fn a_non_finite_session_is_ignored() {
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 50.0, None, T0);
    sr.note(&n("work"), T0 + 120, f64::NAN, None, T0 + 120);
    sr.note(&n("work"), T0 + 240, f64::INFINITY, None, T0 + 240);
    assert_eq!(sr.len(), 1);
    assert_eq!(sr.rate(120), None);
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

#[test]
fn a_rate_never_narrows_the_margin_band() {
    // Crusty round 6: a cache frozen at 66% whose observation time keeps
    // moving gives a rate of 0 (or a diluted one). The figure is still
    // within --margin of --session-at, so it is near.
    let s = s();
    assert!(near(66.0, Some(0.0), &s), "66 >= 65 with a rate of 0");
    assert!(near(65.0, Some(0.0), &s));
    assert!(near(66.0, Some(1.0 / 600.0), &s), "a diluted rate");
    assert!(!near(64.0, Some(0.0), &s), "outside the band, no rise");
    // Outside the band, a rate still widens it.
    assert!(near(60.0, Some(10.0 / 120.0), &s), "60 + 10 = 70");
}

#[test]
fn a_frozen_series_with_a_moving_time_is_near_at_66() {
    // The points the frozen 66% leaves behind when the status line keeps
    // rewriting it with a new time: two identical points one interval apart.
    let s = s();
    let mut sr = Series::default();
    sr.note(&n("work"), T0, 66.0, None, T0);
    sr.note(&n("work"), T0 + 120, 66.0, None, T0 + 120);
    let rate = sr.rate(120);
    assert_eq!(rate, Some(0.0));
    assert!(near(66.0, rate, &s));
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
fn a_substituted_failed_or_below_threshold_reading_gives_projected() {
    for real in [Reading::Failed, m(66.0, 41.0)] {
        let got = map_reason(threshold(), Basis::Substituted, &s(), &real);
        assert_eq!(got, projected_swap(), "{real:?}");
    }
}

#[test]
fn a_substituted_reading_already_at_the_threshold_stays_threshold() {
    let got = map_reason(threshold(), Basis::Substituted, &s(), &m(95.0, 41.0));
    assert_eq!(got, threshold(), "a stale 95% needs a swap on its own");
}

#[test]
fn map_reason_never_creates_or_suppresses_a_swap_or_relabels_other_reasons() {
    let s = s();
    let real = m(63.0, 41.0);
    for basis in [Basis::Projected, Basis::Substituted] {
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

// --- with_outlook: the outlook reaches the threshold row only ---------------

fn verdict(decision: Decision) -> Verdict {
    Verdict {
        decision,
        tag: None,
        new_blocks: Vec::new(),
        next_plan_reset: None,
        unknown_plan: vec![n("busy")],
        key_held: Vec::new(),
    }
}

fn swap_for(reason: Reason) -> Decision {
    Decision::Swap {
        to: n("other"),
        reason,
    }
}

#[test]
fn a_projection_with_no_candidate_is_the_alert_not_plan_room_or_overflow() {
    // Measured 63%, projected 73%: the decision rule on the projection
    // finds no candidate that meets the margin. Whatever the plan-room or
    // overflow rules would make of 73%, they never see it.
    let s = s();
    let got = with_outlook(
        verdict(Decision::Stay),
        Decision::Alert,
        Basis::Projected,
        &s,
        &m(63.0, 41.0),
    );
    assert_eq!(got.decision, Decision::Alert);
    assert_eq!(got.tag, None, "never on overflow");
    assert_eq!(got.next_plan_reset, None);
    assert!(got.key_held.is_empty(), "no API-key ALERT on a projection");
    assert_eq!(got.unknown_plan, [n("busy")], "the measured state is kept");
}

#[test]
fn a_threshold_swap_on_a_projection_is_relabelled_projected() {
    let s = s();
    let got = with_outlook(
        verdict(Decision::Stay),
        swap_for(Reason::Threshold),
        Basis::Projected,
        &s,
        &m(63.0, 41.0),
    );
    assert_eq!(got.decision, swap_for(Reason::Projected));
    assert_eq!(got.tag, None);
}

#[test]
fn a_swap_the_measured_reading_makes_by_itself_stands() {
    // At 75% measured, the plan-room swap and the move onto overflow run on
    // the measured reading, as without the outlook.
    let s = s();
    for reason in [Reason::PlanRoom, Reason::OverflowIn, Reason::Threshold] {
        let mut real = verdict(swap_for(reason));
        if reason == Reason::OverflowIn {
            real.tag = Some(Tag::In);
        }
        for on_outlook in [Decision::Alert, threshold()] {
            let got = with_outlook(
                real.clone(),
                on_outlook,
                Basis::Substituted,
                &s,
                &m(75.0, 41.0),
            );
            assert_eq!(got, real, "{reason:?}");
        }
    }
}

#[test]
fn a_measured_alert_at_a_threshold_keeps_its_api_key_note() {
    let s = s();
    let mut real = verdict(Decision::Alert);
    real.key_held = vec![n("busy")];
    let got = with_outlook(
        real.clone(),
        Decision::Alert,
        Basis::Projected,
        &s,
        &m(10.0, 95.0),
    );
    assert_eq!(got, real);
}

#[test]
fn a_substituted_failed_probe_whose_target_beats_the_substitute_is_undamped() {
    let s = s();
    // The failed-probe rules give a threshold swap, which damping delays.
    // The target also has --margin more headroom than the substituted 70%.
    let real = verdict(threshold());
    let got = with_outlook(
        real.clone(),
        threshold(),
        Basis::Substituted,
        &s,
        &Reading::Failed,
    );
    assert_eq!(got.decision, projected_swap(), "undamped on the outlook");
    assert_eq!(got.unknown_plan, [n("busy")], "the measured state is kept");
}

#[test]
fn an_alert_on_the_substitute_never_replaces_the_failed_probe_swap() {
    // Crusty round 7: `spare` at 68% has 32 points of headroom, 2 more than
    // the substituted 70% leaves `work`, so the outlook ALERTs. The failed
    // probe means headroom 0 to the failed-probe rules, whose threshold
    // swap must stand so that damping makes it after three failures. The
    // substitution changes timing only, never the target.
    let s = s();
    let real = verdict(threshold());
    let got = with_outlook(
        real.clone(),
        Decision::Alert,
        Basis::Substituted,
        &s,
        &Reading::Failed,
    );
    assert_eq!(got, real, "the failed-probe rules' own swap");
    // With no candidate even against headroom 0, their ALERT stands.
    let real = verdict(Decision::Alert);
    for on_outlook in [Decision::Alert, threshold()] {
        let got = with_outlook(
            real.clone(),
            on_outlook.clone(),
            Basis::Substituted,
            &s,
            &Reading::Failed,
        );
        assert_eq!(got, real, "never creates a swap: {on_outlook:?}");
    }
}

#[test]
fn a_substituted_failed_probe_never_retargets_the_failed_probe_swap() {
    let s = s();
    let real = verdict(swap_for(Reason::Threshold));
    let got = with_outlook(
        real.clone(),
        swap_for(Reason::Threshold),
        Basis::Substituted,
        &s,
        &Reading::Failed,
    );
    assert_eq!(got.decision, swap_for(Reason::Projected), "real's target");
}

#[test]
fn a_swap_the_outlook_names_to_another_login_never_makes_the_failed_probe_swap_sooner() {
    // Crusty round 8: the failed-probe rules chose `other` (in the live
    // login's organisation, say); `spare` beats the substituted figure, but
    // that says nothing of `other`. Damping must apply to `other`'s swap.
    let s = s();
    let real = verdict(swap_for(Reason::Threshold));
    let got = with_outlook(
        real.clone(),
        threshold(),
        Basis::Substituted,
        &s,
        &Reading::Failed,
    );
    assert_eq!(got, real, "damped as without the outlook");
    // Nor does a stay on the outlook (`outlook_failed`'s answer when the
    // target does not beat the substituted figure, or is a crossing).
    let got = with_outlook(
        real.clone(),
        Decision::Stay,
        Basis::Substituted,
        &s,
        &Reading::Failed,
    );
    assert_eq!(got, real);
}

#[test]
fn a_stay_on_the_outlook_of_a_measured_reading_leaves_the_measured_verdict() {
    // `affinity::outlook` gives a stay where only a confirmed crossing
    // could take the session: the outlook adds neither a swap nor the
    // no-capacity ALERT.
    let s = s();
    let real = verdict(Decision::Stay);
    for basis in [Basis::Projected, Basis::Substituted] {
        let got = with_outlook(real.clone(), Decision::Stay, basis, &s, &m(63.0, 41.0));
        assert_eq!(got, real, "{basis:?}");
    }
}

#[test]
fn the_early_failed_probe_swap_is_still_stopped_by_a_home_key() {
    // The substitution only makes the failed-probe swap sooner, so the
    // home-key rule applies to it as it does to the `failed-probe` swap.
    use super::super::damp::{is_failed_probe_swap, keep_for_home_key};
    let key = Some("primaryApiKey set");
    let early = projected_swap();
    assert!(is_failed_probe_swap(&early, &Reading::Failed));
    let (d, note) = keep_for_home_key(early.clone(), None, key, &Reading::Failed);
    assert_eq!(d, Decision::Stay);
    assert!(note.is_some_and(|n| n.contains("home key")));
    // A projection on a measured reading did not come from a failed probe.
    let measured = m(63.0, 41.0);
    assert!(!is_failed_probe_swap(&early, &measured));
    assert_eq!(
        keep_for_home_key(early.clone(), None, key, &measured).0,
        early
    );
    let late = swap_for(Reason::FailedProbe);
    assert!(is_failed_probe_swap(&late, &Reading::Failed));
    assert!(!is_failed_probe_swap(&threshold(), &Reading::Failed));
}

#[test]
fn a_measured_reading_at_a_threshold_decides_by_itself() {
    // Week at 95%: the measured reading needs a swap on its own, and the
    // overflow-aware rules found no plan login with capacity. An outlook
    // whose higher session figure lets a candidate meet the margin must not
    // turn that ALERT into a threshold swap onto a login without capacity.
    let s = s();
    let real = verdict(Decision::Alert);
    for basis in [Basis::Projected, Basis::Substituted] {
        let got = with_outlook(real.clone(), threshold(), basis, &s, &m(63.0, 95.0));
        assert_eq!(got, real, "{basis:?}");
    }
}

#[test]
fn the_live_time_is_capped_at_the_start_of_the_measurement() {
    use super::super::decide::Probed;
    use super::super::fresh::live_time;
    use super::super::tests_issue_1870::rd;
    let at = T0;
    // A live re-read made 5 s after the measurement started, undated or
    // dated: never later than the start.
    assert_eq!(live_time(&rd(75.0, 20.0, Some(at + 5)), at), Some(at));
    // A direct live read of this measurement, or an older passive reading.
    assert_eq!(live_time(&rd(75.0, 20.0, None), at), Some(at));
    assert_eq!(live_time(&rd(75.0, 20.0, Some(at - 30)), at), Some(at - 30));
    assert_eq!(live_time(&Probed::failed(), at), None);
}

/// Exercise the real live step and target gate with explicitly supplied
/// evidence times. Each test thread owns its clock and callback script.
mod evidence {
    use super::super::super::cycle::Ctx;
    use super::super::super::decide::Probed;
    use super::super::super::tests_issue_1870::{exh, rd};
    use super::super::super::tests_revalidate_cause::{ctx_with, park};
    use super::super::super::tests_swap::{Env, env};
    use super::super::super::{fresh, observe, revalidate};
    use super::*;
    use crate::commands::profile::auto_switch::kit::{State, now};
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::time::Duration;

    thread_local! {
        static CLOCK: Cell<i64> = const { Cell::new(0) };
        static ANSWERS: RefCell<VecDeque<Probed>> = const { RefCell::new(VecDeque::new()) };
        static CALLS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn clock() -> i64 {
        CLOCK.get()
    }

    fn reread(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
        CALLS.with_borrow_mut(|v| v.push(name.to_string()));
        ANSWERS.with_borrow_mut(|v| v.pop_front().expect("unexpected external reread"))
    }

    fn no_read(_: &Ctx, _: &ProfileName, _: Duration) -> Probed {
        panic!("locked revalidation must not read externally")
    }

    fn fixture() -> (Env, Ctx, observe::Snap) {
        let e = env();
        let mut c = ctx_with(&e, |_, _| panic!("measurement is supplied explicitly"));
        c.settings = s();
        c.now = clock;
        c.reread = Some(reread);
        c.reread_live = Some(reread);
        CLOCK.set(now());
        ANSWERS.with_borrow_mut(VecDeque::clear);
        CALLS.with_borrow_mut(Vec::clear);
        let snap = observe::snapshot(&c, &State::new(&e.root).unwrap()).unwrap();
        (e, c, snap)
    }

    fn script(probes: impl IntoIterator<Item = Probed>) {
        ANSWERS.with_borrow_mut(|v| v.extend(probes));
    }

    fn cached(session: f64, at: i64) -> Probed {
        let mut p = rd(session, 20.0, Some(at));
        p.passive = true;
        p
    }

    fn live(c: &mut Ctx, snap: &observe::Snap, at: i64, p: Probed) -> observe::Measurement {
        CLOCK.set(at);
        // Match run_cycle's pre-measurement reset. Outlook belongs to one cycle.
        c.live_outlook = None;
        let m = observe::decide_measured(c, snap, p, vec![(n("spare"), rd(0.0, 40.0, None))], at)
            .unwrap();
        fresh::live::step(c, snap, m).unwrap()
    }

    fn establish_lag(c: &mut Ctx, snap: &observe::Snap, t: i64) {
        script([rd(65.0, 20.0, None)]);
        for dt in [0, 10, 20] {
            live(c, snap, t + dt, cached(63.0, t + dt));
        }
        CALLS.with_borrow(|v| assert_eq!(v.as_slice(), ["work"]));
    }

    #[test]
    fn older_cache_above_the_last_direct_value_cannot_clear_established_lag() {
        let (_e, mut c, snap) = fixture();
        let t = clock();
        establish_lag(&mut c, &snap, t);
        script([rd(68.0, 20.0, None), rd(71.0, 20.0, None)]);
        // 67% is greater than the accepted 65%, but predates that direct
        // evidence. It must not be accepted merely because it has moved.
        let m = live(&mut c, &snap, t + 30, cached(67.0, t + 19));
        assert_eq!(
            m.live,
            Reading::Measured {
                session: 68.0,
                week: 20.0
            }
        );
        let m = live(&mut c, &snap, t + 40, cached(69.0, t + 29));
        assert_eq!(
            m.live,
            Reading::Measured {
                session: 71.0,
                week: 20.0
            }
        );
        assert!(matches!(m.decision, Decision::Swap { .. }));
        CALLS.with_borrow(|v| assert_eq!(v.len(), 3));
    }

    #[test]
    fn failed_reread_then_changed_older_cache_preserves_lag_until_recovery() {
        let (_e, mut c, snap) = fixture();
        let t = clock();
        establish_lag(&mut c, &snap, t);
        script([Probed::failed(), rd(68.0, 20.0, None), rd(71.0, 20.0, None)]);
        let m = live(&mut c, &snap, t + 30, cached(64.0, t + 19));
        assert!(
            m.fresh_lines
                .iter()
                .any(|l| l.contains("failed; treated as unknown"))
        );
        assert_eq!(c.live_outlook.as_ref().unwrap().basis, Basis::Substituted);
        // The failed reread left the frozen count and the lag mark as they
        // were. This changed cache value is still older than the accepted
        // direct reading.
        let m = live(&mut c, &snap, t + 40, cached(66.0, t + 19));
        assert_eq!(
            m.live,
            Reading::Measured {
                session: 68.0,
                week: 20.0
            }
        );
        let m = live(&mut c, &snap, t + 50, cached(63.0, t + 19));
        assert_eq!(
            m.live,
            Reading::Measured {
                session: 71.0,
                week: 20.0
            }
        );
        CALLS.with_borrow(|v| assert_eq!(v.len(), 4));
    }

    /// `p` with its session window resetting at `resets`.
    fn in_window(mut p: Probed, resets: i64) -> Probed {
        if let Some(w) = p.windows.session.as_mut() {
            w.resets_at = Some(resets);
        }
        p
    }

    /// As `establish_lag`, with every reading in the window resetting at
    /// `resets`: cached 63% three times, then a direct 65%.
    fn establish_lag_in(c: &mut Ctx, snap: &observe::Snap, t: i64, resets: i64) {
        script([in_window(rd(65.0, 20.0, None), resets)]);
        for dt in [0, 10, 20] {
            live(c, snap, t + dt, in_window(cached(63.0, t + dt), resets));
        }
        CALLS.with_borrow(|v| assert_eq!(v.as_slice(), ["work"]));
        assert!(c.live_series.behind());
    }

    #[test]
    fn a_later_session_reset_time_clears_lag_without_a_reread() {
        let (_e, mut c, snap) = fixture();
        let t = clock();
        let resets = t + 3600;
        establish_lag_in(&mut c, &snap, t, resets);
        // A 0% in the next window: its reset is a session window later.
        let next = resets + 5 * 3600;
        let m = live(&mut c, &snap, t + 30, in_window(cached(0.0, t + 30), next));
        assert_eq!(m.decision, Decision::Stay);
        assert!(c.live_outlook.is_none());
        assert!(!c.live_series.behind(), "a new window ends the lag");
        assert_eq!(
            c.live_series.len(),
            1,
            "old window's rate must be discarded"
        );
        CALLS.with_borrow(|v| assert_eq!(v.len(), 1));
    }

    #[test]
    fn a_newer_lower_cache_figure_in_the_same_window_is_read_again() {
        // Crusty round 7: a newer observation time is not a reset. The
        // status line rewrites old figures with new times, so a lower
        // figure in the same window keeps the lag and the series.
        let (_e, mut c, snap) = fixture();
        let t = clock();
        let resets = t + 3600;
        establish_lag_in(&mut c, &snap, t, resets);
        let len = c.live_series.len();
        script([in_window(rd(68.0, 20.0, None), resets)]);
        let m = live(
            &mut c,
            &snap,
            t + 30,
            in_window(cached(64.0, t + 30), resets),
        );
        assert_eq!(
            m.live,
            Reading::Measured {
                session: 68.0,
                week: 20.0
            }
        );
        assert!(c.live_series.behind(), "still behind");
        assert_eq!(c.live_series.len(), len + 1, "the series kept its points");
        CALLS.with_borrow(|v| assert_eq!(v.len(), 2));
    }

    #[test]
    fn a_newer_lower_cache_figure_with_no_reset_time_is_read_again() {
        // With no reset time to compare, a lower cached figure is checked
        // by a direct read. One that confirms it is a new window.
        let (_e, mut c, snap) = fixture();
        let t = clock();
        establish_lag(&mut c, &snap, t);
        script([rd(0.0, 20.0, None)]);
        let m = live(&mut c, &snap, t + 30, cached(0.0, t + 30));
        assert_eq!(m.decision, Decision::Stay);
        assert!(!c.live_series.behind(), "the direct read agreed");
        assert_eq!(c.live_series.len(), 1, "a new window, by a direct read");
        CALLS.with_borrow(|v| assert_eq!(v.len(), 2));
    }

    #[test]
    fn equal_older_cache_and_older_direct_reread_preserve_accepted_history() {
        let (_e, mut c, snap) = fixture();
        let t = clock();
        establish_lag(&mut c, &snap, t);
        script([rd(1.0, 20.0, Some(t + 19)), rd(71.0, 20.0, Some(t + 40))]);
        let m = live(&mut c, &snap, t + 30, cached(65.0, t + 19));
        assert_eq!(c.live_outlook.as_ref().unwrap().basis, Basis::Substituted);
        assert!(
            m.fresh_lines
                .iter()
                .any(|l| l.contains("failed; treated as unknown"))
        );
        let m = live(&mut c, &snap, t + 40, cached(64.0, t + 19));
        assert_eq!(
            m.live,
            Reading::Measured {
                session: 71.0,
                week: 20.0
            }
        );
        CALLS.with_borrow(|v| assert_eq!(v.len(), 3));
    }

    #[test]
    fn newer_cache_catch_up_and_login_change_end_the_old_lag() {
        let (_e, mut c, mut snap) = fixture();
        let t = clock();
        establish_lag(&mut c, &snap, t);
        live(&mut c, &snap, t + 30, cached(66.0, t + 30));
        // New login evidence cannot inherit work's rate or lag flag.
        snap.live = super::super::super::identify::LiveId::Known(n("spare"));
        let m = live(&mut c, &snap, t + 40, cached(0.0, t + 40));
        assert_eq!(m.decision, Decision::Stay);
        assert_eq!(c.live_series.len(), 1);
        assert!(c.live_outlook.is_none());
        CALLS.with_borrow(|v| assert_eq!(v.len(), 1));
    }

    #[test]
    fn cache_catch_up_does_not_authorize_a_later_older_observation() {
        let (_e, mut c, snap) = fixture();
        let t = clock();
        establish_lag(&mut c, &snap, t);
        live(&mut c, &snap, t + 30, cached(66.0, t + 30));
        script([rd(71.0, 20.0, None)]);
        let m = live(&mut c, &snap, t + 40, cached(0.0, t + 19));
        assert_eq!(
            m.live,
            Reading::Measured {
                session: 71.0,
                week: 20.0
            }
        );
        CALLS.with_borrow(|v| assert_eq!(v.len(), 2));
    }

    #[test]
    fn required_target_reread_replaces_all_rejected_evidence_and_reselects() {
        // Include a retained, non-previous damping entry: previous alone
        // is not a record of every candidate's applicable recorded use.
        for record in [
            "previous-only",
            "non-previous",
            "map-newer",
            "previous-newer",
        ] {
            for answer in [Probed::failed(), exh(100.0, 20.0), rd(10.0, 20.0, None)] {
                let (e, c, snap) = fixture();
                let t = clock();
                let mut ledger = e.rs.load_ledger().unwrap().unwrap();
                ledger.previous = Some(
                    if record == "non-previous" {
                        "third"
                    } else {
                        "spare"
                    }
                    .into(),
                );
                ledger.swapped_at = Some(if record == "map-newer" {
                    t - 30
                } else {
                    t - 20
                });
                if record != "previous-only" {
                    ledger.swapped_away.insert(
                        "spare".into(),
                        if record == "previous-newer" {
                            t - 30
                        } else {
                            t - 20
                        },
                    );
                }
                e.rs.save_ledger(&ledger).unwrap();
                script([answer.clone()]);
                // Equal live/target timestamps defeat the older-than-live
                // check; only the applicable use record makes this stale.
                let m = observe::decide_measured(
                    &c,
                    &snap,
                    exh(100.0, 20.0),
                    vec![(n("spare"), cached(0.0, t - 21))],
                    t - 21,
                )
                .unwrap();
                let m = fresh::gate(&c, &snap, m, fresh::Reread::Allowed).unwrap();
                let got = &m.probed.as_ref().unwrap().1[0].1;
                assert_eq!(
                    got.reading, answer.reading,
                    "replace the reading, not just its timestamp"
                );
                assert_eq!(
                    got.windows, answer.windows,
                    "no stale capacity survives failure"
                );
                if answer.unknown {
                    assert_eq!(
                        *got,
                        Probed::failed(),
                        "replace all rejected probe metadata"
                    );
                }
                assert_eq!(
                    matches!(m.decision, Decision::Swap { .. }),
                    !answer.unknown && answer.reading != exh(100.0, 20.0).reading
                );
                CALLS.with_borrow(|v| assert_eq!(v.as_slice(), ["spare"]));
            }
        }
    }

    #[test]
    fn still_stale_reread_is_made_unknown_instead_of_reusing_rejected_capacity() {
        let (e, c, snap) = fixture();
        let t = clock();
        let mut ledger = e.rs.load_ledger().unwrap().unwrap();
        ledger.previous = Some("spare".into());
        ledger.swapped_at = Some(t);
        e.rs.save_ledger(&ledger).unwrap();
        script([rd(0.0, 0.0, None)]);
        let m = observe::decide_measured(
            &c,
            &snap,
            exh(100.0, 20.0),
            vec![(n("spare"), cached(0.0, t - 1))],
            t - 1,
        )
        .unwrap();
        // Direct evidence completed at the recorded use time, not after
        // it. The gate must reject it on the second pass without looping.
        let m = fresh::gate(&c, &snap, m, fresh::Reread::Allowed).unwrap();
        assert!(!matches!(m.decision, Decision::Swap { .. }));
        assert_eq!(m.probed.unwrap().1[0].1, Probed::failed());
        CALLS.with_borrow(|v| assert_eq!(v.as_slice(), ["spare"]));
    }

    #[test]
    fn a_reading_after_recorded_use_remains_eligible_without_rereading() {
        let (e, c, snap) = fixture();
        let t = clock();
        let mut ledger = e.rs.load_ledger().unwrap().unwrap();
        ledger.previous = Some("third".into());
        ledger.swapped_at = Some(t - 10);
        ledger.swapped_away.insert("spare".into(), t - 20);
        e.rs.save_ledger(&ledger).unwrap();
        let m = observe::decide_measured(
            &c,
            &snap,
            exh(100.0, 20.0),
            vec![(n("spare"), cached(10.0, t - 19))],
            t - 19,
        )
        .unwrap();
        let m = fresh::gate(&c, &snap, m, fresh::Reread::Allowed).unwrap();
        assert!(matches!(m.decision, Decision::Swap { to, .. } if to == n("spare")));
        CALLS.with_borrow(|v| assert!(v.is_empty()));
    }

    /// Crusty round 6: the live step runs after the measurement, so a live
    /// re-read is dated after `measured_at`, while a target read directly in
    /// that measurement is dated `measured_at`. The gate must not count the
    /// target as older than the live reading, whether the re-read is
    /// undated (dated by the step clock) or carries its own later time.
    #[test]
    fn a_live_reread_after_a_slow_measurement_keeps_the_cycles_targets_fresh() {
        let swap = Decision::Swap {
            to: n("spare"),
            reason: Reason::Threshold,
        };
        for (took, dated) in [(0, false), (1, false), (5, false), (5, true), (90, false)] {
            let (_e, mut c, snap) = fixture();
            let t = clock();
            // The live re-read reads 75%. A target re-read would fail, which
            // would leave no candidate and turn the swap into an ALERT.
            script([rd(75.0, 20.0, dated.then_some(t + took)), Probed::failed()]);
            let m = observe::decide_measured(
                &c,
                &snap,
                cached(63.0, t - 200),
                vec![(n("spare"), rd(0.0, 40.0, None))],
                t,
            )
            .unwrap();
            assert_eq!(m.decision, Decision::Stay, "63% as measured");
            // The measurement took `took` seconds.
            CLOCK.set(t + took);
            let m = fresh::live::step(&mut c, &snap, m).unwrap();
            assert_eq!(m.decision, swap, "took {took} s, dated {dated}");
            let m = fresh::gate(&c, &snap, m, fresh::Reread::Allowed).unwrap();
            assert_eq!(
                m.decision, swap,
                "took {took} s, dated {dated}: {:?}",
                m.fresh_lines
            );
            CALLS.with_borrow(|v| assert_eq!(v.as_slice(), ["work"], "took {took} s"));
            assert!(
                !m.fresh_lines.iter().any(|l| l.contains("spare")),
                "no target re-read line: {:?}",
                m.fresh_lines
            );
            ANSWERS
                .with_borrow(|v| assert_eq!(v.len(), 1, "the failing target read was never made"));
        }
    }

    /// The cap keeps the #1870 rule for a target whose passive reading was
    /// taken before the measurement: it is older than the live reading, as
    /// it is next to a direct live read, and is read again.
    #[test]
    fn a_passive_target_from_before_the_measurement_is_still_read_again() {
        let (_e, mut c, snap) = fixture();
        let t = clock();
        script([rd(75.0, 20.0, None), rd(10.0, 40.0, None)]);
        let m = observe::decide_measured(
            &c,
            &snap,
            cached(63.0, t - 200),
            vec![(n("spare"), cached(0.0, t - 10))],
            t,
        )
        .unwrap();
        CLOCK.set(t + 5);
        let m = fresh::live::step(&mut c, &snap, m).unwrap();
        let m = fresh::gate(&c, &snap, m, fresh::Reread::Allowed).unwrap();
        CALLS.with_borrow(|v| assert_eq!(v.as_slice(), ["work", "spare"]));
        assert!(matches!(m.decision, Decision::Swap { to, .. } if to == n("spare")));
        assert!(
            m.fresh_lines
                .iter()
                .any(|l| l.contains("reread: spare reading stale")),
            "{:?}",
            m.fresh_lines
        );
    }

    #[test]
    fn target_reread_retains_its_timestamp_and_rejects_carried_results() {
        for carried in [false, true] {
            let (e, c, snap) = fixture();
            let t = clock();
            let mut ledger = e.rs.load_ledger().unwrap().unwrap();
            ledger.previous = Some("spare".into());
            ledger.swapped_at = Some(t - 10);
            e.rs.save_ledger(&ledger).unwrap();
            let mut answer = rd(0.0, 0.0, Some(t - 20));
            answer.carried = carried;
            script([answer]);
            let m = observe::decide_measured(
                &c,
                &snap,
                exh(100.0, 20.0),
                vec![(n("spare"), cached(0.0, t - 20))],
                t - 20,
            )
            .unwrap();
            let m = fresh::gate(&c, &snap, m, fresh::Reread::Allowed).unwrap();
            assert!(!matches!(m.decision, Decision::Swap { .. }));
            assert_eq!(m.probed.unwrap().1[0].1, Probed::failed());
            CALLS.with_borrow(|v| assert_eq!(v.as_slice(), ["spare"]));
        }
    }

    /// A dated re-read that is itself still stale is logged with its new
    /// figures and then as failed, so the log never reads as if the re-read
    /// had been accepted.
    #[test]
    fn a_still_stale_dated_reread_is_logged_as_failed() {
        let (e, c, snap) = fixture();
        let t = clock();
        let mut ledger = e.rs.load_ledger().unwrap().unwrap();
        ledger.previous = Some("spare".into());
        ledger.swapped_at = Some(t - 10);
        e.rs.save_ledger(&ledger).unwrap();
        script([rd(0.0, 0.0, Some(t - 20))]);
        let m = observe::decide_measured(
            &c,
            &snap,
            exh(100.0, 20.0),
            vec![(n("spare"), cached(0.0, t - 20))],
            t - 20,
        )
        .unwrap();
        let m = fresh::gate(&c, &snap, m, fresh::Reread::Allowed).unwrap();
        assert!(!matches!(m.decision, Decision::Swap { .. }));
        assert_eq!(m.probed.as_ref().unwrap().1[0].1, Probed::failed());
        assert_eq!(
            m.fresh_lines,
            [
                "reread: spare reading stale s=0% w=20%; fresh s=0% w=0%",
                "reread: spare failed; treated as unknown",
            ]
        );
        CALLS.with_borrow(|v| assert_eq!(v.as_slice(), ["spare"]));
    }

    #[test]
    fn locked_validation_rejects_newly_stale_target_even_when_identity_is_unchanged() {
        for previous in [false, true] {
            let (e, mut c, snap) = fixture();
            let t = clock();
            c.reread = Some(no_read);
            c.reread_live = Some(no_read);
            let m = observe::decide_measured(
                &c,
                &snap,
                exh(100.0, 20.0),
                vec![(n("spare"), cached(0.0, t - 20))],
                t - 20,
            )
            .unwrap();
            assert!(matches!(m.decision, Decision::Swap { .. }));
            let state = State::new(&e.root).unwrap();
            let _lock = state.try_lock(Duration::ZERO).unwrap().unwrap();
            // Recorded use changed since measurement; the identity snapshot
            // did not. The fast Proceed path must still enforce freshness.
            let mut ledger = e.rs.load_ledger().unwrap().unwrap();
            ledger.previous = Some(if previous { "spare" } else { "third" }.into());
            ledger.swapped_at = Some(if previous { t - 10 } else { t - 30 });
            if !previous {
                ledger.swapped_away.insert("spare".into(), t - 10);
            }
            e.rs.save_ledger(&ledger).unwrap();
            let checked = revalidate::check(&mut c, &state, &snap, &m, 0).unwrap();
            assert!(
                matches!(&checked.verdict, revalidate::Verdict::Skip(v) if v.is_empty()),
                "stale target must fail closed under the lock"
            );
            e.assert_unchanged();
        }
    }

    #[test]
    fn locked_reselection_checks_non_previous_use_without_external_reads() {
        let (e, mut c, _) = fixture();
        super::super::super::tests_revalidate_cause::add_stored(&e, "spare2");
        let snap = observe::snapshot(&c, &State::new(&e.root).unwrap()).unwrap();
        let t = clock();
        c.reread = Some(no_read);
        c.reread_live = Some(no_read);
        let mut ledger = e.rs.load_ledger().unwrap().unwrap();
        ledger.previous = Some("third".into());
        ledger.swapped_at = Some(t - 30);
        ledger.swapped_away.insert("spare2".into(), t - 10);
        e.rs.save_ledger(&ledger).unwrap();
        let m = observe::decide_measured(
            &c,
            &snap,
            exh(100.0, 20.0),
            vec![
                (n("spare"), rd(0.0, 0.0, None)),
                (n("spare2"), cached(5.0, t - 20)),
            ],
            t - 20,
        )
        .unwrap();
        park(&e.root, "spare");
        let _lock = State::new(&e.root)
            .unwrap()
            .try_lock(Duration::ZERO)
            .unwrap()
            .unwrap();
        let checked =
            revalidate::check(&mut c, &State::new(&e.root).unwrap(), &snap, &m, 0).unwrap();
        assert!(matches!(checked.verdict, revalidate::Verdict::Skip(_)));
    }

    /// B7: an unchanged snapshot with a gated swap whose target is still
    /// fresh is retried under the lock; the same decision proceeds.
    #[test]
    fn unchanged_snapshot_with_a_fresh_target_proceeds_under_the_lock() {
        let (e, mut c, snap) = fixture();
        let t = clock();
        c.reread = Some(no_read);
        c.reread_live = Some(no_read);
        let m = observe::decide_measured(
            &c,
            &snap,
            exh(100.0, 20.0),
            vec![(n("spare"), rd(0.0, 20.0, Some(t - 20)))],
            t - 20,
        )
        .unwrap();
        assert!(matches!(&m.decision, Decision::Swap { to, .. } if *to == n("spare")));
        let state = State::new(&e.root).unwrap();
        let _lock = state.try_lock(Duration::ZERO).unwrap().unwrap();
        let checked = revalidate::check(&mut c, &state, &snap, &m, 0).unwrap();
        assert!(
            matches!(checked.verdict, revalidate::Verdict::Proceed),
            "the same retried decision proceeds"
        );
        e.assert_unchanged();
    }

    /// B7: an unchanged snapshot whose swap target ages past
    /// [`fresh::MAX_READING_AGE_SECS`] under the lock is decided once more on
    /// the stored probes; another fresh target is taken, with the
    /// `stale reading` line.
    #[test]
    fn unchanged_snapshot_stale_target_with_a_fresh_alternative_is_reconsidered() {
        let (e, mut c, _) = fixture();
        super::super::super::tests_revalidate_cause::add_stored(&e, "spare2");
        let snap = observe::snapshot(&c, &State::new(&e.root).unwrap()).unwrap();
        let t = clock();
        c.reread = Some(no_read);
        c.reread_live = Some(no_read);
        let mut spent = exh(100.0, 20.0);
        spent.observed_at = Some(t - 100);
        let m = observe::decide_measured(
            &c,
            &snap,
            spent,
            vec![
                (n("spare"), rd(0.0, 20.0, Some(t - 50))),
                (n("spare2"), rd(5.0, 20.0, Some(t - 10))),
            ],
            t,
        )
        .unwrap();
        assert!(matches!(&m.decision, Decision::Swap { to, .. } if *to == n("spare")));
        // Only `spare` is older than the age limit at the later clock.
        CLOCK.set(t - 50 + fresh::MAX_READING_AGE_SECS + 1);
        let state = State::new(&e.root).unwrap();
        let _lock = state.try_lock(Duration::ZERO).unwrap().unwrap();
        let checked = revalidate::check(&mut c, &state, &snap, &m, 0).unwrap();
        match checked.verdict {
            revalidate::Verdict::Reconsidered { line, m, .. } => {
                assert_eq!(line, revalidate::reconsidered(&n("spare"), "stale reading"));
                assert!(
                    matches!(&m.decision, Decision::Swap { to, .. } if *to == n("spare2")),
                    "{:?}",
                    m.decision
                );
            }
            _ => panic!("expected a reconsidered swap"),
        }
        e.assert_unchanged();
    }

    /// B7: an unchanged snapshot whose only target ages past the limit under
    /// the lock ends the cycle with the bare skip line and no labels.
    #[test]
    fn unchanged_snapshot_stale_target_without_an_alternative_skips_with_the_bare_line() {
        let (e, mut c, snap) = fixture();
        let t = clock();
        c.reread = Some(no_read);
        c.reread_live = Some(no_read);
        let mut spent = exh(100.0, 20.0);
        spent.observed_at = Some(t - 100);
        let m = observe::decide_measured(
            &c,
            &snap,
            spent,
            vec![(n("spare"), rd(0.0, 20.0, Some(t - 50)))],
            t,
        )
        .unwrap();
        assert!(matches!(&m.decision, Decision::Swap { to, .. } if *to == n("spare")));
        CLOCK.set(t - 50 + fresh::MAX_READING_AGE_SECS + 1);
        let state = State::new(&e.root).unwrap();
        let _lock = state.try_lock(Duration::ZERO).unwrap().unwrap();
        let checked = revalidate::check(&mut c, &state, &snap, &m, 0).unwrap();
        match checked.verdict {
            revalidate::Verdict::Skip(v) => {
                assert!(v.is_empty(), "{v:?}");
                assert_eq!(
                    revalidate::changed_line(&v),
                    super::super::super::log::msg::CHANGED
                );
            }
            _ => panic!("expected a bare skip"),
        }
        e.assert_unchanged();
    }

    /// B7: a measurement that kept no probes cannot be retried; with an
    /// unchanged snapshot it proceeds rather than skipping.
    #[test]
    fn unchanged_snapshot_without_probes_proceeds_under_the_lock() {
        let (e, mut c, snap) = fixture();
        let t = clock();
        c.reread = Some(no_read);
        c.reread_live = Some(no_read);
        let mut m = observe::decide_measured(
            &c,
            &snap,
            exh(100.0, 20.0),
            vec![(n("spare"), rd(0.0, 20.0, Some(t - 20)))],
            t - 20,
        )
        .unwrap();
        assert!(matches!(m.decision, Decision::Swap { .. }));
        m.probed = None;
        let state = State::new(&e.root).unwrap();
        let _lock = state.try_lock(Duration::ZERO).unwrap().unwrap();
        let checked = revalidate::check(&mut c, &state, &snap, &m, 0).unwrap();
        assert!(matches!(checked.verdict, revalidate::Verdict::Proceed));
    }

    /// Risk 4.1: a target the gate re-read (its stale probe replaced by the
    /// fresh answer in `m.probed`) passes locked revalidation without any
    /// further external read.
    #[test]
    fn a_reread_target_proceeds_through_locked_revalidation() {
        let (e, mut c, snap) = fixture();
        let t = clock();
        let mut ledger = e.rs.load_ledger().unwrap().unwrap();
        ledger.previous = Some("spare".into());
        ledger.swapped_at = Some(t - 20);
        e.rs.save_ledger(&ledger).unwrap();
        script([rd(10.0, 20.0, None)]);
        let m = observe::decide_measured(
            &c,
            &snap,
            exh(100.0, 20.0),
            vec![(n("spare"), cached(0.0, t - 21))],
            t - 21,
        )
        .unwrap();
        let m = fresh::gate(&c, &snap, m, fresh::Reread::Allowed).unwrap();
        assert!(matches!(&m.decision, Decision::Swap { to, .. } if *to == n("spare")));
        CALLS.with_borrow(|v| assert_eq!(v.as_slice(), ["spare"]));
        c.reread = Some(no_read);
        c.reread_live = Some(no_read);
        let state = State::new(&e.root).unwrap();
        let _lock = state.try_lock(Duration::ZERO).unwrap().unwrap();
        let checked = revalidate::check(&mut c, &state, &snap, &m, 0).unwrap();
        assert!(
            matches!(checked.verdict, revalidate::Verdict::Proceed),
            "a re-read target must not be skipped under the lock"
        );
    }
}

/// Production reader chain, rather than a callback-routing assertion. The
/// server classifies synthetic credentials but never retains or prints headers.
/// Shutdown and request completion use blocking I/O and joins, with no timers.
#[cfg(unix)]
mod production_reader {
    use super::super::super::cycle::Ctx;
    use super::super::super::decide::{Decision, Reading};
    use super::super::super::tests_issue_1870::rd;
    use super::super::super::tests_revalidate_cause::ctx_with;
    use super::super::super::tests_swap::{Env, env, n, private};
    use super::super::super::{fresh, observe};
    use crate::commands::profile::auto_switch::kit::{State, now};
    use crate::test_support::with_env;
    use serde_json::json;
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Request {
        LiveUsage,
        StoredUsage,
        Refresh,
        Other,
    }

    struct Server {
        address: std::net::SocketAddr,
        requests: Arc<Mutex<Vec<Request>>>,
        worker: Option<JoinHandle<()>>,
    }

    impl Server {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let log = Arc::clone(&requests);
            let worker = std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    let mut input = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    input.read_line(&mut line).unwrap();
                    if line.contains(" /__shutdown ") {
                        break;
                    }
                    let refresh = line.starts_with("POST /v1/oauth/token ");
                    let usage = line.starts_with("GET /api/oauth/usage ");
                    let mut live = false;
                    let mut stored = false;
                    let mut length = 0;
                    loop {
                        line.clear();
                        input.read_line(&mut line).unwrap();
                        if line == "\r\n" || line.is_empty() {
                            break;
                        }
                        if let Some((key, value)) = line.split_once(':') {
                            if key.eq_ignore_ascii_case("authorization") {
                                live = value.trim() == "Bearer synthetic-live-2211";
                                stored = value.trim() == "Bearer synthetic-stored-2211";
                            }
                            if key.eq_ignore_ascii_case("content-length") {
                                length = value.trim().parse().unwrap();
                            }
                        }
                    }
                    // Consume refresh input without recording or printing it.
                    std::io::copy(&mut input.take(length), &mut std::io::sink()).unwrap();
                    let request = match (refresh, usage, live, stored) {
                        (true, _, _, _) => Request::Refresh,
                        (_, true, true, _) => Request::LiveUsage,
                        (_, true, _, true) => Request::StoredUsage,
                        _ => Request::Other,
                    };
                    log.lock().unwrap().push(request);
                    let body = if refresh {
                        json!({"access_token":"synthetic-stored-2211", "refresh_token":"synthetic-renewed-2211", "expires_in":3600})
                    } else {
                        json!({"five_hour":{"utilization":if live {80} else {5}, "resets_at":"2099-01-01T00:00:00Z"},
                            "seven_day":{"utilization":20, "resets_at":"2099-01-02T00:00:00Z"},
                            "extra_usage":{"is_enabled":false}})
                    }.to_string();
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
                    stream.flush().unwrap();
                }
            });
            Self {
                address,
                requests,
                worker: Some(worker),
            }
        }

        fn seen(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let mut stream = TcpStream::connect(self.address).unwrap();
            stream
                .write_all(b"GET /__shutdown HTTP/1.1\r\n\r\n")
                .unwrap();
            self.worker.take().unwrap().join().unwrap();
        }
    }

    fn credentials(path: &Path, live: bool, expired: bool) {
        // Preserve the fixture's moving identity for real cycle/snapshot code.
        let mut body: serde_json::Value =
            serde_json::from_str(&super::super::tests_fixture_identity::body(path)).unwrap();
        body["claudeAiOauth"] = json!({
            "accessToken": if live {"synthetic-live-2211"} else {"synthetic-stored-2211"},
            "refreshToken":"synthetic-refresh-2211",
            "expiresAt": if expired {1_i64} else {i64::MAX}
        });
        private(path, &body.to_string());
    }

    fn isolated(e: &Env, server: &Server, run: impl FnOnce(&mut Ctx)) {
        let mut c = ctx_with(e, |_, _| panic!("measurement supplied explicitly"));
        c.settings = super::s();
        c.reread_live = Some(fresh::reread_live_as_configured);
        c.reread = Some(fresh::reread_as_configured);
        let decoy = e._tmp.path().join("decoy-home");
        credentials(&decoy.join(".claude/.credentials.json"), false, false);
        let base = format!("http://{}", server.address);
        let token = format!("{base}/v1/oauth/token");
        // HOME deliberately differs from Ctx::home. A fallback to process
        // HOME would use the wrong synthetic credential and read 5%.
        with_env(
            &[
                ("HOME", Some(decoy.as_os_str())),
                ("AMPLIHACK_CLAUDE_PROFILE_ROOT", Some(e.root.as_os_str())),
                ("AMPLIHACK_PROFILE_USAGE_API_BASE", Some(base.as_ref())),
                ("AMPLIHACK_PROFILE_USAGE_TOKEN_URL", Some(token.as_ref())),
                ("AMPLIHACK_PROFILE_USAGE_PROBE", Some("direct".as_ref())),
                ("CLAUDE_CONFIG_DIR", None),
                ("ANTHROPIC_API_KEY", None),
                ("ANTHROPIC_AUTH_TOKEN", None),
                ("CLAUDE_CODE_OAUTH_TOKEN", None),
            ],
            || run(&mut c),
        );
    }

    #[test]
    fn live_reader_uses_supplied_home_and_target_reader_uses_stored_credentials() {
        let e = env();
        let server = Server::start();
        credentials(&e.live(), true, false);
        credentials(&e.slot("work"), false, false);
        // A passive value must not replace either production reread.
        private(
            &e.root.join(".auto-switch.d/usage.json"),
            &json!({"version":1,"accounts":{"work":{
                "five_hour":{"used_percentage":7,"resets_at":now()+3600},
                "seven_day":{"used_percentage":9,"resets_at":now()+86400},
                "observed_at":now(),"source":"statusline"
            }}})
            .to_string(),
        );
        isolated(&e, &server, |c| {
            let live = fresh::reread_live_as_configured(c, &n("work"), Duration::from_secs(5));
            assert_eq!(
                live.reading,
                Reading::Measured {
                    session: 80.0,
                    week: 20.0
                }
            );
            assert!(!live.passive);
            let stored = fresh::reread_as_configured(c, &n("work"), Duration::from_secs(5));
            assert_eq!(
                stored.reading,
                Reading::Measured {
                    session: 5.0,
                    week: 20.0
                }
            );
        });
        assert_eq!(server.seen(), [Request::LiveUsage, Request::StoredUsage]);
    }

    #[test]
    fn stale_live_step_succeeds_without_any_stored_live_credentials() {
        let e = env();
        let server = Server::start();
        credentials(&e.live(), true, false);
        assert!(!e.slot("work").exists());
        isolated(&e, &server, |c| {
            let snap = observe::snapshot(c, &State::new(&e.root).unwrap()).unwrap();
            let at = now();
            let mut old = rd(63.0, 20.0, Some(at - 121));
            old.passive = true;
            let m = observe::decide_measured(
                c,
                &snap,
                old,
                vec![(n("spare"), rd(0.0, 40.0, None))],
                at,
            )
            .unwrap();
            let m = fresh::live::step(c, &snap, m).unwrap();
            assert_eq!(
                m.live,
                Reading::Measured {
                    session: 80.0,
                    week: 20.0
                }
            );
            assert!(matches!(m.decision, Decision::Swap { to, .. } if to == n("spare")));
            assert!(
                m.fresh_lines
                    .iter()
                    .any(|l| l == "stale live reading; re-read")
            );
        });
        assert_eq!(server.seen(), [Request::LiveUsage]);
    }

    #[test]
    fn expired_live_reader_never_refreshes_or_changes_credentials() {
        let _refresh = crate::test_support::refresh_count_lock();
        let e = env();
        let server = Server::start();
        credentials(&e.live(), true, true);
        credentials(&e.slot("work"), false, true);
        let before_live = fs::read(e.live()).unwrap();
        let before_stored = fs::read(e.slot("work")).unwrap();
        let live_mtime = fs::metadata(e.live()).unwrap().modified().unwrap();
        isolated(&e, &server, |c| {
            let p = fresh::reread_live_as_configured(c, &n("work"), Duration::from_secs(5));
            assert_eq!(p.reading, Reading::Failed);
            assert!(
                server.seen().is_empty(),
                "expired live token sends no request"
            );
            assert!(
                fs::read(e.live()).unwrap() == before_live,
                "live credential bytes changed"
            );
            assert_eq!(
                fs::metadata(e.live()).unwrap().modified().unwrap(),
                live_mtime,
                "live credential rewritten (mtime changed)"
            );
            assert!(
                fs::read(e.slot("work")).unwrap() == before_stored,
                "stored credential bytes changed"
            );
            // Positive control: identical expiry on a stored target does
            // refresh, proving the fixture exercises the expiry branch.
            let p = fresh::reread_as_configured(c, &n("work"), Duration::from_secs(5));
            assert_eq!(
                p.reading,
                Reading::Measured {
                    session: 5.0,
                    week: 20.0
                }
            );
        });
        assert_eq!(server.seen(), [Request::Refresh, Request::StoredUsage]);
        assert!(
            fs::read(e.live()).unwrap() == before_live,
            "live credential bytes changed"
        );
        assert!(
            fs::read(e.slot("work")).unwrap() != before_stored,
            "positive control did not refresh"
        );
    }
}
