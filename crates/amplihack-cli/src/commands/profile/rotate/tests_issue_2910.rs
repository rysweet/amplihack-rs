//! Issue #2910: the rotator swaps only to a login with room now. Pure
//! decisions over stand-in readings; no I/O.
//!
//! Contract:
//! - The week bound for a target is `--week-at`, strictly, in every pass:
//!   `plan_room::choose`, `decide::decide` (threshold and upgrade swaps) and
//!   the overflow substitution all use `overflow::plan_usable`.
//! - `Probed::no_room()`: a reading that fails `plan_usable` and carries no
//!   spend limit, substituted when a target's fresh reading shows no room,
//!   so the existing re-decision picks the next candidate.
//! - `fresh::must_confirm(first_visit: bool, can_read: bool) -> bool`: the
//!   chosen target is re-read once per cycle, unless no read is possible.
//!   The caller passes `first_visit` false for a reading this cycle took
//!   directly, which is already the confirmation rule 3 asks for.
//! - `Reading::headroom()` order is kept for ranking among logins with room.

use super::super::decide::{Decision, Measured, Probed, Reading, Reason, Windows, decide};
use super::super::fresh::must_confirm;
use super::super::overflow::plan_usable;
use super::super::settings::Settings;
use super::{choose, room_under};
use crate::commands::profile::name::ProfileName;

const NOW: i64 = 1_793_389_376;

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
    }
}

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn r(session: f64, week: f64) -> Reading {
    Reading::Measured { session, week }
}

fn m(name: &str, reading: Reading) -> Measured {
    Measured {
        name: n(name),
        reading,
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
    }
}

fn swapped_to(d: &Decision) -> Option<&str> {
    match d {
        Decision::Swap { to, .. } => Some(to.as_str()),
        _ => None,
    }
}

#[test]
fn a_login_at_98_percent_of_its_week_is_never_chosen_by_plan_room() {
    let s = settings();
    assert_eq!(choose(&s, None, &[m("spare", r(5.0, 98.0))]), None);
    let live = m("work", r(50.0, 60.0));
    assert_eq!(choose(&s, Some(&live), &[m("spare", r(5.0, 98.0))]), None);
}

#[test]
fn the_week_bound_is_week_at_and_strict() {
    let s = settings();
    assert_eq!(
        choose(&s, None, &[m("spare", r(10.0, 89.9))]),
        Some(n("spare"))
    );
    assert_eq!(choose(&s, None, &[m("spare", r(10.0, 90.0))]), None);
    let loose = Settings {
        week_at: 99,
        ..settings()
    };
    assert_eq!(
        choose(&loose, None, &[m("spare", r(10.0, 98.0))]),
        Some(n("spare")),
        "a configured --week-at moves the bound"
    );
}

#[test]
fn plan_room_picks_the_login_with_room_over_one_past_the_week_bound() {
    let s = settings();
    let plan = [m("full_week", r(1.0, 98.0)), m("roomy", r(40.0, 70.0))];
    assert_eq!(choose(&s, None, &plan), Some(n("roomy")));
}

#[test]
fn a_threshold_swap_never_lands_on_a_login_past_the_week_bound() {
    let s = settings();
    let live = Reading::EXHAUSTED;
    let only = [m("spare", r(5.0, 92.0))];
    assert_eq!(decide(&s, &live, &only, None, NOW), Decision::Alert);
    let two = [m("spare", r(5.0, 92.0)), m("fine", r(30.0, 60.0))];
    assert_eq!(
        swapped_to(&decide(&s, &live, &two, None, NOW)),
        Some("fine")
    );
}

#[test]
fn an_upgrade_swap_never_lands_on_a_login_past_the_week_bound() {
    let s = settings();
    let live = r(80.0, 80.0);
    let only = [m("spare", r(0.0, 92.0))];
    assert_eq!(decide(&s, &live, &only, None, NOW), Decision::Stay);
}

#[test]
fn ranking_still_prefers_the_most_headroom_among_logins_with_room() {
    let s = settings();
    let live = Reading::EXHAUSTED;
    let c = [m("a", r(10.0, 80.0)), m("b", r(10.0, 40.0))];
    match decide(&s, &live, &c, None, NOW) {
        Decision::Swap { to, reason } => {
            assert_eq!(to.as_str(), "b");
            assert_eq!(reason, Reason::Threshold);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn plan_usable_fails_closed_on_failed_exhausted_and_unmeasured_readings() {
    let s = settings();
    assert!(plan_usable(&s, &r(10.0, 10.0)));
    assert!(!plan_usable(&s, &r(10.0, 98.0)));
    assert!(!plan_usable(&s, &r(90.0, 10.0)));
    assert!(!plan_usable(&s, &Reading::Failed));
    assert!(!plan_usable(&s, &Reading::EXHAUSTED));
    assert!(!plan_usable(&s, &r(f64::NAN, 10.0)));
}

#[test]
fn room_under_keeps_its_meaning_for_the_recorded_live_room() {
    // #2911 reads `last_cycle.live_room`; it must not gain the week bound.
    assert!(room_under(10.0, 99.9, 85));
    assert!(!room_under(10.0, 100.0, 85));
    assert!(!room_under(85.0, 10.0, 85));
}

#[test]
fn a_no_room_probe_is_never_usable_and_leaves_no_trace_of_a_spend_limit() {
    let p = Probed::no_room();
    assert!(!plan_usable(&settings(), &p.reading));
    assert!(!p.spend_limit);
    assert_eq!(p.windows, Windows::default());
}

#[test]
fn the_chosen_target_is_confirmed_once_per_cycle_and_only_when_a_read_is_possible() {
    assert!(must_confirm(true, true), "first visit, can read");
    assert!(!must_confirm(false, true), "already confirmed this cycle");
    assert!(
        !must_confirm(true, false),
        "no read possible: the swap is not refused for it"
    );
    assert!(!must_confirm(false, false));
}
