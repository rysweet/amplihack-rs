//! Issue #2910 (F1 to F4, D1 to D4, D15, S3, S6, S11): the one pure rule that
//! chooses the login to move to. Wired from `target.rs` (`rotate::decide::target`).
//!
//! The rule is pure: it reads no clock and does no I/O. Every test builds its
//! readings in memory. Nothing here starts `claude`, reads a login or touches
//! a profile root.
//!
//! The API these tests exercise (provided in
//! `target.rs`; `decide` keeps its signature and is built on it):
//!
//! ```text
//! room_now(session: f64, week: f64) -> Option<u8>
//! enum Unfit { SpendLimit, NoReading, Full, AboveCeiling }
//! fit(m: &Measured, week_max: u8) -> Result<u8 /* room now */, Unfit>
//! enum LiveRoom { NoRoom, Failed, Room(u8) }
//! live_room(live: &Reading, blocked: bool) -> LiveRoom
//! relief_floor(live: LiveRoom, margin: u8) -> u16
//! upgrade_floor(live_room: u8, upgrade_margin: u8) -> u16
//! ranked<'a>(s: &Settings, candidates: &'a [Measured], floor: u16)
//!     -> Vec<(&'a ProfileName, u8 /* room now */)>
//! held_move(hold: u64, swapped_at: Option<i64>, now: i64) -> bool
//! swap_reason(s: &Settings, to: &Reading) -> Reason   // Threshold or PlanRoom
//! enum NoMove { NoTarget, LessRoom { most, room, needs },
//!               Held { to, session, since }, Unconfirmed }
//! no_move(s, live: &Reading, candidates: &[Measured], swapped_at, now) -> NoMove
//! ```
//!
//! `decide` returns `Reason::Threshold` for every relief swap (E2); the label
//! `plan-room` is applied afterwards by `swap_reason`.

use super::*;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::decide::{
    Decision, Measured, Reading, Reason, Windows, decide,
};
use crate::commands::profile::rotate::settings::Settings;

// --- Fixtures ----------------------------------------------------------------

const NOW: i64 = 1_795_000_000;

fn settings(session_at: u8, week_at: u8, margin: u8, upgrade_margin: u8, hold: u64) -> Settings {
    Settings {
        session_at,
        week_at,
        margin,
        upgrade_margin,
        hold,
        interval: 60,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
        target_week_max: 95,
    }
}

/// The documented defaults: 85, 90, margin 5, upgrade margin 30, hold 3600.
fn defaults() -> Settings {
    settings(85, 90, 5, 30, 3600)
}

/// The reporter's flags on 22 November: 85, 90, margin 3, upgrade margin 30,
/// hold 3600, ceiling 95.
fn nov22() -> Settings {
    settings(85, 90, 3, 30, 3600)
}

/// The thresholds the hosts run, `--session-at 95 --week-at 98`, with the
/// reporter's other flags and the default ceiling of 95.
fn deployed() -> Settings {
    settings(95, 98, 3, 30, 3600)
}

fn name(n: &str) -> ProfileName {
    ProfileName::parse(n).unwrap()
}

fn m(who: &str, session: f64, week: f64) -> Measured {
    Measured {
        name: name(who),
        reading: Reading::Measured { session, week },
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
    }
}

/// A login a probe reported as out of room: a window at 100% or more.
fn out(who: &str, session: f64, week: f64) -> Measured {
    Measured {
        reading: Reading::Exhausted {
            session: Some(session),
            week: Some(week),
        },
        ..m(who, session, week)
    }
}

/// A probe that read nothing (failed, timed out or still running).
fn unread(who: &str) -> Measured {
    Measured {
        reading: Reading::Failed,
        unknown: true,
        ..m(who, 0.0, 0.0)
    }
}

fn live(session: f64, week: f64) -> Reading {
    Reading::Measured { session, week }
}

fn live_out(session: f64, week: f64) -> Reading {
    Reading::Exhausted {
        session: Some(session),
        week: Some(week),
    }
}

fn names<'a>(r: &[(&'a ProfileName, u8)]) -> Vec<&'a str> {
    r.iter().map(|(n, _)| n.as_str()).collect()
}

fn swapped(d: &Decision) -> Option<(&str, Reason)> {
    match d {
        Decision::Swap { to, reason } => Some((to.as_str(), *reason)),
        _ => None,
    }
}

/// The candidates of the reporter's cycle at 05:56:57Z on 22 November, from the
/// rotate log. A reading at 100% on either window is out of room; "unknown,
/// still running" is an unread probe. The live login is `rrowan` (100/67).
fn nov22_candidates() -> Vec<Measured> {
    vec![
        out("login-aa", 0.0, 100.0),
        out("login-ab", 0.0, 100.0),
        out("login-ac", 0.0, 100.0),
        out("login-ad", 100.0, 74.0),
        m("login-ae", 96.0, 82.0),
        unread("login-af"),
        out("login-ag", 100.0, 12.0),
        out("login-ah", 0.0, 100.0),
        out("login-ai", 0.0, 100.0),
        m("login-aj", 90.0, 10.0),
        unread("login-ak"),
        m("login-al", 0.0, 95.0),
        unread("login-an"),
        out("login-ao", 100.0, 50.0),
        out("login-ap", 0.0, 100.0),
        out("login-aq", 100.0, 99.0),
        m("login-ar", 88.0, 20.0),
        out("login-at", 100.0, 22.0),
        out("login-au", 0.0, 100.0),
        m("ssorrel", 93.0, 22.0),
        m("login-bd", 90.0, 71.0),
        m("login-bh", 88.0, 71.0),
    ]
}

// --- room_now (S3) -------------------------------------------------------------

#[test]
fn room_now_is_the_points_left_in_the_worse_window_rounded_down() {
    assert_eq!(room_now(40.0, 30.0), Some(60));
    assert_eq!(room_now(0.0, 95.0), Some(5));
    assert_eq!(room_now(88.0, 71.0), Some(12));
    assert_eq!(room_now(10.0, 94.5), Some(5), "5.5 points left rounds down");
    assert_eq!(room_now(99.9, 0.0), Some(0));
    assert_eq!(room_now(0.0, 0.0), Some(100));
    assert_eq!(room_now(-0.0, 0.0), Some(100), "negative zero is zero");
}

#[test]
fn room_now_is_none_unless_both_windows_are_from_zero_up_to_below_100() {
    for (s, w) in [
        (100.0, 0.0),
        (0.0, 100.0),
        (100.0, 100.0),
        (1e9, 0.0),
        (f64::NAN, 0.0),
        (0.0, f64::NAN),
        (-1.0, 0.0),
        (0.0, -0.0001),
        (f64::INFINITY, 0.0),
        (0.0, f64::NEG_INFINITY),
    ] {
        assert_eq!(room_now(s, w), None, "room_now({s}, {w})");
    }
}

// --- fit (F1) ------------------------------------------------------------------

#[test]
fn a_week_of_exactly_the_ceiling_is_eligible_and_above_it_is_not() {
    assert_eq!(fit(&m("a", 10.0, 95.0), 95), Ok(5));
    assert_eq!(fit(&m("a", 10.0, 94.9), 95), Ok(5));
    assert_eq!(fit(&m("a", 10.0, 95.4), 95), Err(Unfit::AboveCeiling));
    assert_eq!(fit(&m("a", 10.0, 96.0), 95), Err(Unfit::AboveCeiling));
    assert_eq!(fit(&m("a", 10.0, 98.0), 95), Err(Unfit::AboveCeiling));
}

#[test]
fn the_ceiling_is_the_setting_and_not_a_constant() {
    assert_eq!(fit(&m("a", 10.0, 80.0), 80), Ok(20));
    assert_eq!(fit(&m("a", 10.0, 80.1), 80), Err(Unfit::AboveCeiling));
    assert_eq!(fit(&m("a", 10.0, 99.0), 100), Ok(1));
    assert_eq!(fit(&m("a", 10.0, 1.0), 1), Ok(90));
}

#[test]
fn fit_refuses_in_the_documented_order() {
    // A spend limit reads Failed, and still says what it is.
    let mut spend = unread("a");
    spend.spend_limit = true;
    assert_eq!(fit(&spend, 95), Err(Unfit::SpendLimit));
    // The flag wins over a reading that has room.
    let mut flagged = m("a", 1.0, 1.0);
    flagged.spend_limit = true;
    assert_eq!(fit(&flagged, 95), Err(Unfit::SpendLimit));
    // No reading comes before full.
    assert_eq!(fit(&unread("a"), 95), Err(Unfit::NoReading));
    // Full comes before the ceiling: 100% session with a week of 98.
    assert_eq!(fit(&out("a", 100.0, 98.0), 95), Err(Unfit::Full));
    assert_eq!(fit(&m("a", 100.0, 98.0), 95), Err(Unfit::Full));
    assert_eq!(fit(&m("a", 10.0, 100.0), 95), Err(Unfit::Full));
}

/// S3: a reading no probe can produce never ranks. An over-limit figure is
/// full; one that is not a number, is negative or is infinite is no reading.
#[test]
fn fit_never_lets_a_hostile_figure_rank() {
    for (s, w) in [
        (f64::NAN, 10.0),
        (10.0, f64::NAN),
        (-1.0, 10.0),
        (10.0, -0.0001),
        (f64::NEG_INFINITY, 10.0),
        (10.0, f64::NEG_INFINITY),
        (f64::INFINITY, 10.0),
        (10.0, f64::INFINITY),
    ] {
        assert_eq!(fit(&m("a", s, w), 95), Err(Unfit::NoReading), "{s}/{w}");
    }
    assert_eq!(fit(&m("a", 1e9, 10.0), 95), Err(Unfit::Full));
    assert_eq!(fit(&m("a", 10.0, 1e9), 95), Err(Unfit::Full));
}

// --- live_room, relief_floor, upgrade_floor (F2, D1, D2) -------------------------

#[test]
fn the_live_login_has_no_room_at_a_full_window_or_with_a_block() {
    assert_eq!(live_room(&live_out(100.0, 67.0), false), LiveRoom::NoRoom);
    assert_eq!(live_room(&live(100.0, 67.0), false), LiveRoom::NoRoom);
    assert_eq!(live_room(&live(10.0, 100.0), false), LiveRoom::NoRoom);
    // E11: a recorded block means no room, whatever the probe shows.
    assert_eq!(live_room(&live(10.0, 10.0), true), LiveRoom::NoRoom);
    assert_eq!(live_room(&Reading::Failed, true), LiveRoom::NoRoom);
}

#[test]
fn the_live_login_with_room_reports_its_room_and_a_failed_probe_is_failed() {
    assert_eq!(live_room(&live(86.0, 65.0), false), LiveRoom::Room(14));
    assert_eq!(live_room(&live(0.0, 95.0), false), LiveRoom::Room(5));
    assert_eq!(live_room(&Reading::Failed, false), LiveRoom::Failed);
}

#[test]
fn relief_floor_is_nothing_with_no_room_the_margin_when_failed_else_room_plus_margin() {
    assert_eq!(relief_floor(LiveRoom::NoRoom, 3), 0, "D2: no margin");
    assert_eq!(relief_floor(LiveRoom::NoRoom, 0), 0);
    // F2: a failed probe takes the live room as 0 and requires the margin.
    assert_eq!(relief_floor(LiveRoom::Failed, 3), 3);
    assert_eq!(relief_floor(LiveRoom::Failed, 0), 0);
    // D1: with room, the margin on top of the live room.
    assert_eq!(relief_floor(LiveRoom::Room(14), 3), 17);
    assert_eq!(relief_floor(LiveRoom::Room(5), 5), 10);
    // D1: the floor of 1 stops two logins with equal room trading places.
    assert_eq!(relief_floor(LiveRoom::Room(14), 0), 15);
    // Past 100 nothing can qualify, and nothing overflows.
    assert_eq!(relief_floor(LiveRoom::Room(99), 100), 199);
}

#[test]
fn upgrade_floor_needs_a_gain_above_zero_and_the_upgrade_margin() {
    assert_eq!(upgrade_floor(40, 30), 70);
    assert_eq!(
        upgrade_floor(40, 0),
        41,
        "a gain above 0 even with margin 0"
    );
    assert_eq!(upgrade_floor(40, 1), 41);
    assert_eq!(upgrade_floor(0, 30), 30);
}

// --- ranked (F3) ---------------------------------------------------------------

#[test]
fn tier_one_ranks_before_a_larger_room_in_tier_two() {
    // login-al: session 0 (tier 1), room 5. login-ar: session 88 (tier 2), room 12.
    let c = [m("login-ar", 88.0, 20.0), m("login-al", 0.0, 95.0)];
    let r = ranked(&nov22(), &c, 0);
    assert_eq!(names(&r), ["login-al", "login-ar"]);
    assert_eq!(r[0].1, 5);
    assert_eq!(r[1].1, 12);
}

#[test]
fn within_a_tier_the_larger_room_wins_then_the_lower_week_then_the_name() {
    let c = [
        m("d", 10.0, 50.0), // room 50, week 50
        m("c", 0.0, 40.0),  // room 60, week 40
        m("b", 40.0, 40.0), // room 60, week 40, name b
        m("a", 40.0, 41.0), // room 59
        m("e", 10.0, 40.0), // room 60, week 40, name e
    ];
    // Room 60: c, b, e (equal week, so by name); then 59; then 50.
    assert_eq!(
        names(&ranked(&defaults(), &c, 0)),
        ["b", "c", "e", "a", "d"]
    );
}

#[test]
fn a_lower_week_beats_a_higher_one_at_equal_room() {
    // Both room 12; both tier 2.
    let c = [m("login-bh", 88.0, 71.0), m("login-ar", 88.0, 20.0)];
    assert_eq!(names(&ranked(&nov22(), &c, 0)), ["login-ar", "login-bh"]);
}

#[test]
fn names_break_the_last_tie_in_ascending_byte_order() {
    // '1' (0x31) sorts before '2' (0x32), so "b10" comes before "b2".
    let c = [
        m("b2", 10.0, 10.0),
        m("b10", 10.0, 10.0),
        m("a", 10.0, 10.0),
    ];
    assert_eq!(names(&ranked(&defaults(), &c, 0)), ["a", "b10", "b2"]);
}

#[test]
fn ranked_leaves_out_a_login_below_the_floor_and_every_unfit_one() {
    let mut spend = m("spend", 1.0, 1.0);
    spend.spend_limit = true;
    let c = [
        m("fits", 50.0, 50.0),  // room 50
        m("low", 90.0, 50.0),   // room 10
        m("edge", 10.0, 95.0),  // room 5, at the ceiling
        m("above", 10.0, 96.0), // above the ceiling
        out("full", 100.0, 10.0),
        unread("unread"),
        spend,
        m("nan", f64::NAN, 10.0),
        m("neg", -5.0, 10.0),
        m("huge", 1e9, 10.0),
    ];
    // Tier one (session below 85) first: fits, then edge; low is tier two.
    assert_eq!(names(&ranked(&defaults(), &c, 0)), ["fits", "edge", "low"]);
    assert_eq!(names(&ranked(&defaults(), &c, 11)), ["fits"]);
    assert_eq!(names(&ranked(&defaults(), &c, 5)), ["fits", "edge", "low"]);
    assert!(ranked(&defaults(), &c, 51).is_empty());
}

#[test]
fn ranked_uses_the_week_ceiling_of_the_settings() {
    let c = [m("a", 10.0, 60.0), m("b", 10.0, 40.0)];
    let low = Settings {
        target_week_max: 50,
        ..defaults()
    };
    assert_eq!(names(&ranked(&low, &c, 0)), ["b"]);
}

// --- held_move (D3, S11) --------------------------------------------------------

#[test]
fn a_move_is_held_for_hold_seconds_after_the_last_swap() {
    assert!(held_move(3600, Some(NOW - 600), NOW));
    assert!(held_move(3600, Some(NOW - 3599), NOW));
    assert!(
        !held_move(3600, Some(NOW - 3600), NOW),
        "the hold has run out"
    );
    assert!(!held_move(3600, Some(NOW - 7200), NOW));
    assert!(!held_move(3600, None, NOW), "no swap time holds nothing");
}

#[test]
fn hold_zero_disables_it() {
    assert!(!held_move(0, Some(NOW - 1), NOW));
    assert!(!held_move(0, Some(NOW), NOW));
}

/// S11: a swap time in the future (a clock change) is not evidence of a
/// recent swap for this rule, so it never holds a move.
#[test]
fn a_swap_time_in_the_future_never_holds() {
    assert!(!held_move(3600, Some(NOW + 100), NOW));
    assert!(!held_move(3600, Some(i64::MAX), NOW));
    assert!(!held_move(3600, Some(NOW), NOW - 1));
}

#[test]
fn held_move_does_not_overflow_at_the_extremes() {
    // The span between the extremes must not wrap or panic; the call only has
    // to return.
    let _ = held_move(u64::MAX, Some(i64::MIN), i64::MAX);
    // A hold longer than any clock span holds a recent swap.
    assert!(held_move(u64::MAX, Some(NOW - 1), NOW));
}

// --- swap_reason (D15) ------------------------------------------------------------

#[test]
fn a_target_below_both_thresholds_is_a_threshold_swap() {
    let s = defaults();
    assert_eq!(swap_reason(&s, &live(10.0, 20.0)), Reason::Threshold);
    assert_eq!(swap_reason(&s, &live(84.9, 89.9)), Reason::Threshold);
}

#[test]
fn a_target_with_room_that_is_past_a_threshold_is_a_plan_room_swap() {
    let s = defaults();
    assert_eq!(swap_reason(&s, &live(85.0, 20.0)), Reason::PlanRoom);
    assert_eq!(swap_reason(&s, &live(87.0, 54.0)), Reason::PlanRoom);
    assert_eq!(swap_reason(&s, &live(10.0, 90.0)), Reason::PlanRoom);
    assert_eq!(
        swap_reason(&s, &live(0.0, 95.0)),
        Reason::PlanRoom,
        "login-al"
    );
}

// --- S1, S2, S3 of #1911, absorbed here (D5) ----------------------------------------

/// S1: the live login is spent on its week; the one login that leaves it more
/// room is chosen, and it is past `--session-at`.
#[test]
fn s1_leaves_a_session_fresh_week_high_login_for_the_one_with_more_room() {
    let c = [
        m("login-al", 87.0, 54.0), // room 13
        m("login-af", 93.0, 54.0), // room 7
        m("login-ap", 97.0, 54.0), // room 3
        m("login-ao", 98.0, 55.0), // room 2
    ];
    let d = decide(&defaults(), &live(28.0, 95.0), &c, None, NOW);
    assert_eq!(swapped(&d), Some(("login-al", Reason::Threshold)), "{d:?}");
}

/// S2: the login with the same room as the live one is never the target.
#[test]
fn s2_never_chooses_the_login_that_leaves_no_more_room() {
    let c = [
        m("login-ac", 92.0, 13.0), // room 8, the same as the live login's
        m("login-ao", 86.0, 53.0), // room 14
        m("login-ap", 86.0, 53.0), // room 14
    ];
    let d = decide(&defaults(), &live(92.0, 54.0), &c, None, NOW);
    assert_eq!(swapped(&d), Some(("login-ao", Reason::Threshold)), "{d:?}");
}

/// S3: a login that just fails the margin is never chosen over ones that pass,
/// and a login at 100% is never chosen at all.
#[test]
fn s3_never_chooses_a_login_below_the_floor_and_breaks_ties_by_week_then_name() {
    let c = [
        m("login-ad", 92.0, 13.0), // room 8, below the floor of 4 + 5
        m("login-ak", 87.0, 68.0), // room 13
        m("login-an", 87.0, 67.0), // room 13
        m("login-ao", 87.0, 67.0), // room 13
        out("login-ab", 0.0, 100.0),
    ];
    let d = decide(&defaults(), &live(96.0, 70.0), &c, None, NOW);
    assert_eq!(swapped(&d), Some(("login-an", Reason::Threshold)), "{d:?}");
}

// --- 22 November (AC2, AC9) ---------------------------------------------------------

/// 05:56:57Z. The live login is at 100% of its session. Every plan login that
/// is below 100% on both windows is a target, with no margin (D2), and the one
/// whose session is fresh ranks first.
#[test]
fn the_22_nov_0556_cycle_swaps_to_login_al() {
    let d = decide(
        &nov22(),
        &live_out(100.0, 67.0),
        &nov22_candidates(),
        None,
        NOW,
    );
    assert_eq!(swapped(&d), Some(("login-al", Reason::Threshold)), "{d:?}");
}

#[test]
fn without_login_al_the_22_nov_cycle_takes_the_login_with_the_most_room() {
    let c: Vec<Measured> = nov22_candidates()
        .into_iter()
        .filter(|c| c.name.as_str() != "login-al")
        .collect();
    let d = decide(&nov22(), &live_out(100.0, 67.0), &c, None, NOW);
    // login-ar and login-bh both have room 12; login-ar's week is lower.
    assert_eq!(swapped(&d), Some(("login-ar", Reason::Threshold)), "{d:?}");
}

#[test]
fn with_no_room_on_the_live_login_the_hold_does_not_stop_the_move() {
    // The rotator swapped ten seconds ago and the live login is full again.
    let d = decide(
        &nov22(),
        &live_out(100.0, 67.0),
        &nov22_candidates(),
        Some(NOW - 10),
        NOW,
    );
    assert_eq!(swapped(&d), Some(("login-al", Reason::Threshold)), "{d:?}");
}

/// 05:48:27Z. The live login (86/65) has 14 points of room and is past
/// `--session-at`. No candidate leaves it 3 more, so it stays, and the cycle
/// says so instead of raising the no-capacity ALERT (D1, D10).
#[test]
fn the_22_nov_0548_cycle_stays_because_no_login_leaves_17_points() {
    let c = nov22_candidates();
    let l = live(86.0, 65.0);
    assert_eq!(decide(&nov22(), &l, &c, None, NOW), Decision::Alert);
    match no_move(&nov22(), &l, &c, None, NOW) {
        NoMove::LessRoom { most, room, needs } => {
            assert_eq!((most, room, needs), (Some(12), Some(14), 17));
        }
        other => panic!("expected LessRoom, got {other:?}"),
    }
}

/// One cycle after the 05:57Z swap: the live login is login-al (0/95, room 5),
/// and the best login that leaves it 3 more is past `--session-at`. Inside
/// `--hold` the rotator stays; once the hold has run out it moves (D3).
#[test]
fn after_the_swap_a_move_to_a_login_past_session_at_waits_for_the_hold() {
    let mut c = nov22_candidates();
    c.retain(|c| c.name.as_str() != "login-al");
    c.push(out("rrowan", 100.0, 67.0));
    let l = live(0.0, 95.0);

    let inside = Some(NOW - 600);
    assert_eq!(decide(&nov22(), &l, &c, inside, NOW), Decision::Alert);
    match no_move(&nov22(), &l, &c, inside, NOW) {
        NoMove::Held { to, session, since } => {
            assert_eq!(to.as_str(), "login-ar");
            assert_eq!(session as i64, 88);
            assert_eq!(since, 600);
        }
        other => panic!("expected Held, got {other:?}"),
    }

    let after = Some(NOW - 7200);
    let d = decide(&nov22(), &l, &c, after, NOW);
    assert_eq!(swapped(&d), Some(("login-ar", Reason::Threshold)), "{d:?}");
    // A ledger with no swap time holds nothing.
    let d = decide(&nov22(), &l, &c, None, NOW);
    assert_eq!(swapped(&d), Some(("login-ar", Reason::Threshold)), "{d:?}");
}

/// `decide` and `no_move` each apply the hold; this keeps them in step. Over a
/// grid of live readings, candidate sets, settings and swap times, when
/// `decide` does not move, `no_move` names a hold exactly when taking the hold
/// away makes `decide` move, and to the login it names.
#[test]
fn decide_and_no_move_agree_on_the_hold() {
    let lives = [
        live(86.0, 65.0),
        live(0.0, 95.0),
        live(96.0, 40.0),
        live(100.0, 67.0),
        live_out(100.0, 67.0),
        Reading::Failed,
    ];
    let sets = [
        nov22_candidates(),
        vec![m("a", 88.0, 20.0), m("b", 10.0, 90.0)],
        vec![m("a", 90.0, 10.0), m("b", 91.0, 10.0)],
        vec![m("a", 100.0, 10.0)],
        vec![],
    ];
    let mut held_seen = 0;
    for s in [nov22(), defaults(), deployed()] {
        for l in &lives {
            for c in &sets {
                for at in [None, Some(NOW - 600), Some(NOW - 7200), Some(NOW + 600)] {
                    if decide(&s, l, c, at, NOW) != Decision::Alert {
                        continue;
                    }
                    let free = decide(&s, l, c, None, NOW);
                    match no_move(&s, l, c, at, NOW) {
                        NoMove::Held { to, .. } => {
                            held_seen += 1;
                            assert_eq!(
                                swapped(&free),
                                Some((to.as_str(), Reason::Threshold)),
                                "held, so no hold means a move to {to}: {l:?} {at:?}"
                            );
                        }
                        other => assert_eq!(
                            free,
                            Decision::Alert,
                            "{other:?} is not the hold, so no hold changes nothing: {l:?} {at:?}"
                        ),
                    }
                }
            }
        }
    }
    assert!(held_seen > 0, "the grid reaches the hold at least once");
}

#[test]
fn the_hold_never_stops_a_move_to_a_login_below_session_at() {
    // login-al is live and spent on its week; a fresh login is available.
    let c = [m("fresh", 10.0, 20.0), m("login-ar", 88.0, 20.0)];
    let d = decide(&nov22(), &live(0.0, 95.0), &c, Some(NOW - 10), NOW);
    assert_eq!(swapped(&d), Some(("fresh", Reason::Threshold)), "{d:?}");
}

#[test]
fn a_failed_live_probe_is_not_held_by_the_move_hold() {
    // Damping holds failed-probe swaps itself; this rule must not add to it.
    let c = [m("login-ar", 88.0, 20.0)];
    let d = decide(&nov22(), &Reading::Failed, &c, Some(NOW - 10), NOW);
    assert_eq!(swapped(&d), Some(("login-ar", Reason::Threshold)), "{d:?}");
}

#[test]
fn hold_zero_lets_the_move_go_at_once() {
    let mut c = nov22_candidates();
    c.retain(|c| c.name.as_str() != "login-al");
    let s = Settings { hold: 0, ..nov22() };
    let d = decide(&s, &live(0.0, 95.0), &c, Some(NOW - 1), NOW);
    assert_eq!(swapped(&d), Some(("login-ar", Reason::Threshold)), "{d:?}");
}

// --- 22 November at the deployed thresholds, 95/98 -----------------------------------

/// Every 22 November login with room and a session below 95% is in the first
/// tier, so the ranking is by room now: login-ar and login-bh (12, login-ar's week
/// is lower), login-aj and login-bd (10), ssorrel (7), login-al (5). login-ae
/// (96%) is in the second tier.
#[test]
fn at_the_deployed_settings_the_22_nov_logins_rank_by_room_now() {
    let c = nov22_candidates();
    let r = ranked(&deployed(), &c, 0);
    assert_eq!(
        names(&r),
        [
            "login-ar", "login-bh", "login-aj", "login-bd", "ssorrel", "login-al", "login-ae"
        ]
    );
}

/// 05:56:57Z at 95/98: the live login is full, every login below 100% on
/// both windows qualifies, and login-ar ranks first. Its windows are below both
/// deployed thresholds, so the swap is a threshold swap.
#[test]
fn the_22_nov_0556_cycle_at_the_deployed_settings_swaps_to_login_ar() {
    let s = deployed();
    let d = decide(&s, &live_out(100.0, 67.0), &nov22_candidates(), None, NOW);
    assert_eq!(swapped(&d), Some(("login-ar", Reason::Threshold)), "{d:?}");
    assert_eq!(swap_reason(&s, &live(88.0, 20.0)), Reason::Threshold);
    // The hold never stops a move off a login with no room.
    let d = decide(
        &s,
        &live_out(100.0, 67.0),
        &nov22_candidates(),
        Some(NOW - 10),
        NOW,
    );
    assert_eq!(swapped(&d), Some(("login-ar", Reason::Threshold)), "{d:?}");
}

/// 05:48:27Z at 95/98: the live login (86/65, room 14) is below both
/// thresholds, so nothing needs to move. An upgrade would need room 44 and
/// no login has it: a plain stay, not the advisory ALERT that carries the
/// stay note.
#[test]
fn the_22_nov_0548_cycle_at_the_deployed_settings_is_a_plain_stay() {
    let d = decide(
        &deployed(),
        &live(86.0, 65.0),
        &nov22_candidates(),
        None,
        NOW,
    );
    assert_eq!(d, Decision::Stay);
}

/// At 95/98 the live login reaches `--session-at` with room 5. A move needs
/// room 8, and login-ar (room 12) is below `--session-at`, so the hold does
/// not apply and it moves at once.
#[test]
fn at_the_deployed_settings_a_live_login_at_95_percent_moves_at_once() {
    let l = live(95.0, 66.0);
    let d = decide(&deployed(), &l, &nov22_candidates(), Some(NOW - 10), NOW);
    assert_eq!(swapped(&d), Some(("login-ar", Reason::Threshold)), "{d:?}");
    // One point below the threshold nothing needs to move.
    let d = decide(
        &deployed(),
        &live(94.0, 66.0),
        &nov22_candidates(),
        None,
        NOW,
    );
    assert_eq!(d, Decision::Stay);
}

/// Logins between 95% and 99% of their session at 95/98: a live login at
/// 98% (room 2) may move to one at 95% (room 5, a gain of the margin 3), but
/// that target is at `--session-at`, so the move waits for the hold after the
/// last swap. One at 96% (room 4) is short of the margin and never a target.
#[test]
fn at_the_deployed_settings_a_move_between_logins_past_session_at_waits_for_the_hold() {
    let s = deployed();
    let l = live(98.0, 40.0);
    let c = [m("edge", 95.0, 20.0), m("short", 96.0, 20.0)];
    let inside = Some(NOW - 600);
    assert_eq!(decide(&s, &l, &c, inside, NOW), Decision::Alert);
    match no_move(&s, &l, &c, inside, NOW) {
        NoMove::Held { to, since, .. } => {
            assert_eq!(to.as_str(), "edge");
            assert_eq!(since, 600);
        }
        other => panic!("expected Held, got {other:?}"),
    }
    let d = decide(&s, &l, &c, Some(NOW - 7200), NOW);
    assert_eq!(swapped(&d), Some(("edge", Reason::Threshold)), "{d:?}");
    // At `--session-at`, the cycle labels it a plan-room swap (D15).
    assert_eq!(swap_reason(&s, &live(95.0, 20.0)), Reason::PlanRoom);
    let only_short = [m("short", 96.0, 20.0)];
    assert_eq!(decide(&s, &l, &only_short, None, NOW), Decision::Alert);
}

// --- AC3: a login at 98% of its week is never chosen -----------------------------------

#[test]
fn ac3_a_week_of_98_is_never_chosen_when_the_live_login_has_no_room() {
    let c = [m("only", 10.0, 98.0)];
    assert_eq!(
        decide(&nov22(), &live_out(100.0, 20.0), &c, None, NOW),
        Decision::Alert
    );
}

#[test]
fn ac3_the_ceiling_is_inclusive_at_95_and_not_at_95_point_4() {
    let at = [m("edge", 10.0, 95.0)];
    let d = decide(&nov22(), &live_out(100.0, 20.0), &at, None, NOW);
    assert_eq!(swapped(&d), Some(("edge", Reason::Threshold)), "{d:?}");
    let above = [m("edge", 10.0, 95.4)];
    assert_eq!(
        decide(&nov22(), &live_out(100.0, 20.0), &above, None, NOW),
        Decision::Alert
    );
}

#[test]
fn ac3_a_week_of_98_is_never_an_upgrade_even_with_upgrade_margin_zero() {
    let s = Settings {
        upgrade_margin: 0,
        hold: 0,
        ..defaults()
    };
    let c = [m("x", 0.0, 98.0)];
    assert_eq!(decide(&s, &live(60.0, 60.0), &c, None, NOW), Decision::Stay);
    // The positive control: a login that is better is taken.
    let c = [m("y", 0.0, 50.0)];
    let d = decide(&s, &live(60.0, 60.0), &c, None, NOW);
    assert_eq!(swapped(&d), Some(("y", Reason::Upgrade)), "{d:?}");
}

#[test]
fn ac3_a_week_of_98_is_never_chosen_after_a_failed_live_probe() {
    assert_eq!(
        decide(&nov22(), &Reading::Failed, &[m("x", 0.0, 98.0)], None, NOW),
        Decision::Alert
    );
    let d = decide(&nov22(), &Reading::Failed, &[m("y", 0.0, 95.0)], None, NOW);
    assert_eq!(swapped(&d), Some(("y", Reason::Threshold)), "{d:?}");
}

#[test]
fn ac3_a_week_of_98_loses_to_any_login_under_the_ceiling() {
    let c = [m("high", 0.0, 98.0), m("tight", 0.0, 95.0)];
    let d = decide(&nov22(), &live_out(100.0, 20.0), &c, None, NOW);
    assert_eq!(swapped(&d), Some(("tight", Reason::Threshold)), "{d:?}");
}

// --- Upgrade (F2) -----------------------------------------------------------------------

#[test]
fn an_upgrade_needs_the_upgrade_margin_and_stays_inside_the_hold() {
    let s = defaults();
    // Live room 40; a login with room 80 is 40 better.
    let c = [m("better", 0.0, 20.0)];
    let l = live(60.0, 60.0);
    let d = decide(&s, &l, &c, None, NOW);
    assert_eq!(swapped(&d), Some(("better", Reason::Upgrade)), "{d:?}");
    assert_eq!(decide(&s, &l, &c, Some(NOW - 60), NOW), Decision::Stay);
    // Room 60 is only 20 better than 40, short of 30.
    let c = [m("slightly", 0.0, 40.0)];
    assert_eq!(decide(&s, &l, &c, None, NOW), Decision::Stay);
}

#[test]
fn with_default_settings_a_login_past_session_at_is_never_an_upgrade() {
    // Live 84/89 has room 11; 86/0 has room 14: a gain of 3, short of 30.
    let c = [m("past", 86.0, 0.0)];
    assert_eq!(
        decide(&defaults(), &live(84.0, 89.0), &c, None, NOW),
        Decision::Stay
    );
}

// --- no_move ------------------------------------------------------------------------------

#[test]
fn no_move_names_the_case_with_no_eligible_login() {
    let c = [out("a", 100.0, 10.0), unread("b"), m("c", 5.0, 98.0)];
    assert_eq!(
        no_move(&nov22(), &live_out(100.0, 67.0), &c, None, NOW),
        NoMove::NoTarget
    );
    assert_eq!(
        no_move(&nov22(), &live_out(100.0, 67.0), &[], None, NOW),
        NoMove::NoTarget
    );
}

#[test]
fn no_move_counts_since_from_the_swap_and_never_goes_negative() {
    let c = [m("login-ar", 88.0, 20.0)];
    let l = live(0.0, 95.0);
    match no_move(&nov22(), &l, &c, Some(NOW - 5), NOW) {
        NoMove::Held { since, .. } => assert_eq!(since, 5),
        other => panic!("expected Held, got {other:?}"),
    }
    // A swap time in the future is not a hold (S11), so the move goes ahead.
    let d = decide(&nov22(), &l, &c, Some(NOW + 500), NOW);
    assert_eq!(swapped(&d), Some(("login-ar", Reason::Threshold)), "{d:?}");
}
