//! `rotate::decide`: the pure decision rule (D1-D5, issue #1592).
//!
//! Contract:
//! - `Reading::{Measured { session: f64, week: f64 }, Exhausted, Failed}`,
//!   built from a probe with `Reading::from_report(&UsageReport)`.
//! - `Reading::headroom() -> Option<u8>`: `None` for a failed probe,
//!   `Some(0)` when exhausted, else `100 - max(session, week)` rounded down
//!   and clamped to 0..=100.
//! - `Measured { name: ProfileName, reading: Reading }`.
//! - `decide(&Settings, live: &Reading, candidates: &[Measured], swapped_at:
//!   Option<i64>, now: i64) -> Decision`, where `Decision` is `Stay`,
//!   `Swap { to, reason: Reason::{Threshold, Upgrade} }` or `Alert`.

use super::decide::{Decision, Measured, Reading, Reason, decide};
use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::usage_parse::{UsageReport, UsageState, UsageWindow};

const NOW: i64 = 1_760_000_000;

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
    }
}

fn r(session: f64, week: f64) -> Reading {
    Reading::Measured { session, week }
}

fn c(name: &str, reading: Reading) -> Measured {
    Measured {
        name: ProfileName::parse(name).unwrap(),
        reading,
    }
}

fn swap(to: &str, reason: Reason) -> Decision {
    Decision::Swap {
        to: ProfileName::parse(to).unwrap(),
        reason,
    }
}

fn window(label: &str, used: f64) -> UsageWindow {
    UsageWindow {
        label: label.into(),
        used_percentage: used,
        resets_text: None,
        resets_at: None,
    }
}

fn report(state: UsageState, session: Option<f64>, week: Option<f64>) -> UsageReport {
    UsageReport {
        state,
        session: session.map(|u| window("session", u)),
        week_all: week.map(|u| window("all models", u)),
        week_models: Vec::new(),
    }
}

// --- Readings and headroom -------------------------------------------------

#[test]
fn headroom_is_the_worse_window_rounded_down() {
    assert_eq!(r(20.0, 30.0).headroom(), Some(70));
    assert_eq!(r(85.5, 10.0).headroom(), Some(14));
    assert_eq!(r(0.0, 0.0).headroom(), Some(100));
    assert_eq!(r(120.0, 0.0).headroom(), Some(0), "clamped");
    assert_eq!(Reading::Exhausted.headroom(), Some(0));
    assert_eq!(Reading::Failed.headroom(), None);
}

#[test]
fn readings_from_probe_reports() {
    let ok = Reading::from_report(&report(UsageState::Ok, Some(20.0), Some(30.0)));
    assert_eq!(ok, r(20.0, 30.0));
    let no_session = Reading::from_report(&report(UsageState::Ok, None, Some(30.0)));
    assert_eq!(no_session, r(0.0, 30.0), "a missing session window is 0%");
    let no_week = Reading::from_report(&report(UsageState::Ok, Some(20.0), None));
    assert_eq!(no_week, Reading::Failed, "a missing week window fails");
    for s in [UsageState::SessionExhausted, UsageState::WeeklyExhausted] {
        assert_eq!(
            Reading::from_report(&report(s, Some(100.0), Some(50.0))),
            Reading::Exhausted
        );
    }
    for s in [
        UsageState::SpendLimit,
        UsageState::NotLoggedIn,
        UsageState::Timeout,
        UsageState::Unparsed,
        UsageState::Error,
    ] {
        assert_eq!(
            Reading::from_report(&report(s, Some(1.0), Some(1.0))),
            Reading::Failed,
            "{s:?}"
        );
    }
}

// --- Threshold swaps -------------------------------------------------------

#[test]
fn session_threshold_is_inclusive() {
    let cands = [c("spare", r(0.0, 0.0))];
    assert_eq!(
        decide(&settings(), &r(84.0, 0.0), &cands, None, NOW),
        swap("spare", Reason::Upgrade),
        "below the threshold only an upgrade can happen"
    );
    assert_eq!(
        decide(&settings(), &r(85.0, 0.0), &cands, None, NOW),
        swap("spare", Reason::Threshold)
    );
}

#[test]
fn week_threshold_is_inclusive() {
    let cands = [c("spare", r(0.0, 70.0))];
    assert_eq!(
        decide(&settings(), &r(0.0, 89.0), &cands, None, NOW),
        Decision::Stay,
        "headroom 11 vs 30: below the upgrade margin"
    );
    assert_eq!(
        decide(&settings(), &r(0.0, 90.0), &cands, None, NOW),
        swap("spare", Reason::Threshold)
    );
}

#[test]
fn margin_of_five_is_inclusive_and_four_is_not_enough() {
    // live headroom 10.
    let live = r(90.0, 0.0);
    let five = [c("spare", r(85.0, 0.0))];
    assert_eq!(
        decide(&settings(), &live, &five, None, NOW),
        swap("spare", Reason::Threshold)
    );
    let four = [c("spare", r(86.0, 0.0))];
    assert_eq!(
        decide(&settings(), &live, &four, None, NOW),
        Decision::Alert
    );
}

#[test]
fn a_failed_live_probe_needs_a_swap_with_live_headroom_zero() {
    assert_eq!(
        decide(
            &settings(),
            &Reading::Failed,
            &[c("spare", r(95.0, 0.0))],
            None,
            NOW
        ),
        swap("spare", Reason::Threshold),
        "headroom 5 meets the margin against 0"
    );
    assert_eq!(
        decide(
            &settings(),
            &Reading::Failed,
            &[c("spare", r(96.0, 0.0))],
            None,
            NOW
        ),
        Decision::Alert
    );
}

#[test]
fn an_exhausted_live_login_needs_a_swap() {
    assert_eq!(
        decide(
            &settings(),
            &Reading::Exhausted,
            &[c("spare", r(50.0, 50.0))],
            None,
            NOW
        ),
        swap("spare", Reason::Threshold)
    );
}

#[test]
fn no_candidates_at_a_threshold_is_an_alert() {
    assert_eq!(
        decide(&settings(), &r(99.0, 0.0), &[], None, NOW),
        Decision::Alert
    );
}

#[test]
fn failed_and_exhausted_candidates_are_never_chosen() {
    let cands = [
        c("a-failed", Reading::Failed),
        c("b-exhausted", Reading::Exhausted),
    ];
    assert_eq!(
        decide(&settings(), &r(90.0, 0.0), &cands, None, NOW),
        Decision::Alert
    );
}

#[test]
fn the_best_candidate_wins_and_ties_go_to_the_first_name() {
    let cands = [
        c("zed", r(10.0, 10.0)),
        c("mid", r(40.0, 0.0)),
        c("alpha", r(10.0, 0.0)),
    ];
    assert_eq!(
        decide(&settings(), &r(90.0, 0.0), &cands, None, NOW),
        swap("alpha", Reason::Threshold),
        "alpha and zed both have 90; alpha sorts first"
    );
}

#[test]
fn threshold_swaps_are_never_held() {
    let cands = [c("spare", r(0.0, 0.0))];
    assert_eq!(
        decide(&settings(), &r(90.0, 0.0), &cands, Some(NOW - 1), NOW),
        swap("spare", Reason::Threshold)
    );
}

// --- Upgrades and the hold -------------------------------------------------

#[test]
fn upgrade_margin_of_thirty_is_inclusive_and_twenty_five_is_not_enough() {
    // live headroom 50.
    let live = r(50.0, 0.0);
    assert_eq!(
        decide(&settings(), &live, &[c("spare", r(20.0, 0.0))], None, NOW),
        swap("spare", Reason::Upgrade)
    );
    assert_eq!(
        decide(&settings(), &live, &[c("spare", r(25.0, 0.0))], None, NOW),
        Decision::Stay
    );
}

#[test]
fn an_upgrade_margin_of_zero_never_swaps_to_equal_headroom() {
    // With `--hold 0`, an equal-headroom upgrade would swap back every cycle.
    let s = Settings {
        upgrade_margin: 0,
        hold: 0,
        ..settings()
    };
    let live = r(50.0, 0.0);
    assert_eq!(
        decide(&s, &live, &[c("spare", r(50.0, 0.0))], None, NOW),
        Decision::Stay
    );
    assert_eq!(
        decide(&s, &live, &[c("spare", r(49.0, 0.0))], None, NOW),
        swap("spare", Reason::Upgrade)
    );
}

#[test]
fn no_upgrade_during_the_hold() {
    let cands = [c("spare", r(0.0, 0.0))];
    let live = r(50.0, 0.0);
    assert_eq!(
        decide(&settings(), &live, &cands, Some(NOW - 3599), NOW),
        Decision::Stay
    );
    assert_eq!(
        decide(&settings(), &live, &cands, Some(NOW - 3600), NOW),
        swap("spare", Reason::Upgrade),
        "the hold has ended at exactly `hold` seconds"
    );
}

#[test]
fn hold_zero_disables_the_hold() {
    let s = Settings {
        hold: 0,
        ..settings()
    };
    assert_eq!(
        decide(
            &s,
            &r(50.0, 0.0),
            &[c("spare", r(0.0, 0.0))],
            Some(NOW),
            NOW
        ),
        swap("spare", Reason::Upgrade)
    );
}

#[test]
fn a_swap_time_in_the_future_still_holds() {
    let cands = [c("spare", r(0.0, 0.0))];
    assert_eq!(
        decide(&settings(), &r(50.0, 0.0), &cands, Some(NOW + 10), NOW),
        Decision::Stay
    );
}

#[test]
fn custom_thresholds_and_margins_apply() {
    let s = Settings {
        session_at: 50,
        margin: 0,
        ..settings()
    };
    assert_eq!(
        decide(&s, &r(50.0, 0.0), &[c("spare", r(50.0, 0.0))], None, NOW),
        swap("spare", Reason::Threshold),
        "margin 0: equal headroom is enough"
    );
}

#[test]
fn a_dry_run_reports_the_outcome_it_previewed() {
    use super::cycle::CycleOutcome;
    use super::observe::would;
    assert_eq!(would(&Decision::Stay), CycleOutcome::Stayed);
    assert_eq!(
        would(&swap("spare", Reason::Threshold)),
        CycleOutcome::Swapped
    );
    assert_eq!(would(&Decision::Alert), CycleOutcome::Alerted);
}
