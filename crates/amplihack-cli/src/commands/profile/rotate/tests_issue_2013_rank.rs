//! Issue #2013 (D3, D4, D16, D17): one rule for which logins can be chosen
//! and in what order, used by the threshold and upgrade swaps
//! (`decide::decide`) and by the plan-room swap (`plan_room::choose`).
//!
//! Contract (`rotate/rank.rs`, pure, no side effects):
//! - `rank::eligible(&Settings, &Reading) -> bool`: a measured reading with
//!   `session < 100` and `week < week_at`. `session_at` is not part of it
//!   (7 November addendum): it never drops a login with weekly room. Failed
//!   and exhausted readings are never eligible. Freshness is applied before the rule, by
//!   the caller (`tests_issue_2013_fresh`).
//! - `rank::order(&Measured, &Measured) -> Ordering`: week use ascending,
//!   then session use ascending, then name.
//! - `rank::best_eligible(&Settings, &[Measured]) -> Option<&Measured>`.
//! - `rank::best_with_margin(&Settings, live: &Reading, &[Measured]) ->
//!   Option<&Measured>`: the first candidate with room (both windows below
//!   100) in rank order whose weekly headroom (`100 - week`) beats the live
//!   login's by at least `margin`. A failed or exhausted live reading has
//!   weekly headroom 0.
//! - `decide::best` is removed; `decide::decide` uses `best_eligible`,
//!   then `best_with_margin`, then `Decision::Alert` when a threshold is
//!   reached. A threshold swap to an eligible login needs no margin. A live
//!   login past `session_at` only, itself eligible, stays unless the best
//!   eligible login ranks ahead of it (`rank::ranks_ahead`).
//! - `plan_room::choose` pass 1 takes eligible logins only; pass 2 ranks the
//!   logins with room (the live login among them) and moves only on the
//!   `margin` rule.

use super::decide::{Decision, Reading, Reason, decide};
use super::plan_room::choose;
use super::rank::{best_eligible, best_with_margin, eligible, order};
use super::settings::Settings;
use super::tests_issue_2013_common::{m, settings};
use super::tests_swap::n;
use std::cmp::Ordering;

const T: i64 = 1_794_200_000;

fn measured(session: f64, week: f64) -> Reading {
    Reading::Measured { session, week }
}

fn swap_to(name: &str, reason: Reason) -> Decision {
    Decision::Swap {
        to: n(name),
        reason,
    }
}

// --- eligible ---------------------------------------------------------------

#[test]
fn eligible_needs_session_room_and_week_below_week_at() {
    let s = settings();
    assert!(eligible(&s, &measured(84.0, 89.0)));
    // Issue #2013, 7 November addendum: session_at drops no login.
    assert!(eligible(&s, &measured(85.0, 10.0)), "session at session_at");
    assert!(eligible(&s, &measured(99.0, 10.0)), "session below 100");
    assert!(!eligible(&s, &measured(100.0, 10.0)), "no session room");
    assert!(!eligible(&s, &measured(10.0, 90.0)), "week at week_at");
    assert!(!eligible(&s, &measured(10.0, 97.0)));
}

#[test]
fn failed_exhausted_and_unreadable_readings_are_never_eligible() {
    let s = settings();
    assert!(!eligible(&s, &Reading::Failed));
    assert!(!eligible(
        &s,
        &Reading::Exhausted {
            session: Some(100.0),
            week: Some(5.0)
        }
    ));
    assert!(!eligible(&s, &measured(f64::NAN, 10.0)));
    assert!(!eligible(&s, &measured(10.0, f64::NAN)));
}

#[test]
fn eligibility_follows_the_settings_not_fixed_numbers() {
    let s = Settings {
        session_at: 5,
        week_at: 50,
        ..settings()
    };
    assert!(
        eligible(&s, &measured(10.0, 11.0)),
        "session_at drops no login"
    );
    assert!(!eligible(&s, &measured(4.0, 50.0)), "week 50 >= 50");
    assert!(eligible(&s, &measured(4.0, 49.0)));
}

// --- order ------------------------------------------------------------------

#[test]
fn order_ranks_week_first_then_session_then_name() {
    // aanderson has less headroom overall (20) but less week use.
    let a = m("aanderson", 80.0, 10.0);
    let b = m("bbuck", 0.0, 15.0);
    assert_eq!(order(&a, &b), Ordering::Less);
    let i = m("iivanovich", 20.0, 52.0);
    let l = m("llarame", 30.0, 52.0);
    assert_eq!(order(&i, &l), Ordering::Less, "a week tie goes to session");
    let x = m("aaa", 20.0, 52.0);
    assert_eq!(order(&x, &i), Ordering::Less, "a full tie goes to the name");
    assert_eq!(order(&i, &i.clone()), Ordering::Equal);
}

// --- best_eligible and best_with_margin --------------------------------------

#[test]
fn best_eligible_takes_the_lowest_week_not_the_most_overall_headroom() {
    let s = settings();
    // a: headroom 60, week 20. b: headroom 70, week 30. The old rule (most
    // headroom in the worse window) chose b.
    let c = [m("bravo", 5.0, 30.0), m("alpha", 40.0, 20.0)];
    assert_eq!(
        best_eligible(&s, &c).map(|x| x.name.as_str()),
        Some("alpha")
    );
}

#[test]
fn best_eligible_is_none_when_nothing_is_eligible() {
    let s = settings();
    let c = [m("full", 10.0, 97.0), m("spent", 100.0, 10.0)];
    assert!(best_eligible(&s, &c).is_none());
    assert!(best_eligible(&s, &[]).is_none());
}

#[test]
fn best_with_margin_needs_margin_points_of_weekly_headroom_over_the_live_login() {
    let s = settings();
    let live = measured(10.0, 98.0);
    // Weekly headroom 3 against the live login's 2: a gain of 1 < 5.
    let near = [m("ffarmer", 10.0, 97.0), m("oole", 41.0, 97.0)];
    assert!(best_with_margin(&s, &live, &near).is_none());
    // Session near 100, week 11: weekly headroom 89.
    let room = [m("ffarmer", 10.0, 97.0), m("eelsworth", 96.0, 11.0)];
    assert_eq!(
        best_with_margin(&s, &live, &room).map(|x| x.name.as_str()),
        Some("eelsworth")
    );
}

#[test]
fn best_with_margin_never_takes_a_login_without_room() {
    let s = settings();
    let live = Reading::Failed;
    let c = [m("spent", 100.0, 10.0), m("week", 10.0, 100.0)];
    assert!(best_with_margin(&s, &live, &c).is_none());
}

#[test]
fn a_failed_or_exhausted_live_reading_has_weekly_headroom_zero() {
    let s = settings();
    let c = [m("ok", 95.0, 96.0)];
    for live in [
        Reading::Failed,
        Reading::Exhausted {
            session: Some(100.0),
            week: Some(10.0),
        },
    ] {
        assert_eq!(
            best_with_margin(&s, &live, &c).map(|x| x.name.as_str()),
            Some("ok"),
            "{live:?}"
        );
    }
}

// --- D16: the fixture from the issue ------------------------------------------

/// Live at w=98%; candidates at w=97%, 11% and 52%, all at s=10%.
fn d16() -> (Reading, Vec<super::decide::Measured>) {
    (
        measured(10.0, 98.0),
        vec![
            m("ninetyseven", 10.0, 97.0),
            m("eleven", 10.0, 11.0),
            m("fiftytwo", 10.0, 52.0),
        ],
    )
}

#[test]
fn d16_the_threshold_swap_goes_to_the_11_percent_login() {
    let (live, c) = d16();
    assert_eq!(
        decide(&settings(), &live, &c, None, T),
        swap_to("eleven", Reason::Threshold)
    );
}

#[test]
fn d16_plan_room_chooses_the_11_percent_login() {
    let (_, c) = d16();
    let live = m("live", 10.0, 98.0);
    assert_eq!(choose(&settings(), Some(&live), &c), Some(n("eleven")));
}

#[test]
fn d16_with_session_at_5_the_11_percent_login_is_still_chosen() {
    let s = Settings {
        session_at: 5,
        ..settings()
    };
    let (live, c) = d16();
    // 7 November addendum: every session (10%) is past session_at, which
    // drops none of them.
    assert_eq!(
        decide(&s, &live, &c, None, T),
        swap_to("eleven", Reason::Threshold)
    );
}

#[test]
fn d16_with_week_at_10_nothing_is_eligible_and_the_margin_rule_applies() {
    let s = Settings {
        week_at: 10,
        ..settings()
    };
    let (live, c) = d16();
    assert!(best_eligible(&s, &c).is_none(), "every week is >= 10");
    // eleven still has room, and 89 points of weekly headroom beat the live
    // login's 2 by more than the margin.
    assert_eq!(
        decide(&s, &live, &c, None, T),
        swap_to("eleven", Reason::Threshold)
    );
}

#[test]
fn d4_every_candidate_near_its_weekly_limit_means_no_threshold_swap() {
    let s = settings();
    let live = measured(10.0, 98.0);
    let c = [m("ffarmer", 10.0, 97.0), m("oole", 41.0, 97.0)];
    assert_eq!(decide(&s, &live, &c, None, T), Decision::Alert);
}

#[test]
fn d4_plan_room_stays_rather_than_move_between_logins_at_97_and_98_percent() {
    let s = settings();
    let live = m("nnorth", 17.0, 98.0);
    let c = [m("ffarmer", 10.0, 97.0), m("oole", 41.0, 97.0)];
    // Before #2013, pass 1 filtered on session only and chose ffarmer.
    assert_eq!(choose(&s, Some(&live), &c), None);
}

#[test]
fn plan_room_pass_1_filters_on_week_at() {
    let s = settings();
    let live = m("live", 90.0, 95.0);
    let c = [m("weekfull", 10.0, 92.0), m("ok", 40.0, 60.0)];
    assert_eq!(choose(&s, Some(&live), &c), Some(n("ok")));
}

#[test]
fn plan_room_pass_2_still_stays_when_the_live_login_ranks_first() {
    let s = settings();
    // ddrumpf live at s=0% w=92%; ccapstone at s=11% w=92%.
    let live = m("ddrumpf", 0.0, 92.0);
    let c = [m("ccapstone", 11.0, 92.0)];
    assert_eq!(choose(&s, Some(&live), &c), None);
    // And ccapstone would not move back.
    let live = m("ccapstone", 11.0, 92.0);
    let c = [m("ddrumpf", 0.0, 92.0)];
    assert_eq!(choose(&s, Some(&live), &c), None);
}

// --- D17 and the upgrade path ---------------------------------------------------

#[test]
fn d17_a_threshold_swap_to_an_eligible_login_needs_no_margin() {
    let s = settings();
    // Live at week_at with weekly headroom 9; the candidate's is 12, a
    // gain of 3 < margin, but it is eligible.
    let live = measured(10.0, 91.0);
    let c = [m("spare", 10.0, 88.0)];
    assert_eq!(
        decide(&s, &live, &c, None, T),
        swap_to("spare", Reason::Threshold)
    );
    // Live past session_at only: the same holds for a candidate that ranks
    // ahead of it on week, by any amount.
    let live = measured(86.0, 40.0);
    let c = [m("spare", 10.0, 39.0)];
    assert_eq!(
        decide(&s, &live, &c, None, T),
        swap_to("spare", Reason::Threshold)
    );
}

/// The 7 November addendum: a live login past session_at only never moves
/// to a login with less weekly room, whatever that login's session use.
#[test]
fn a_live_login_past_session_at_only_never_moves_to_less_weekly_room() {
    let s = settings();
    let live = measured(86.0, 40.0);
    let c = [m("spare", 10.0, 82.0)];
    assert_eq!(decide(&s, &live, &c, None, T), Decision::Stay);
    // Equal figures never displace each other either.
    let c = [m("twin", 86.0, 40.0)];
    assert_eq!(decide(&s, &live, &c, None, T), Decision::Stay);
    // With no other login eligible, it is the ALERT, as before.
    let c = [m("full", 10.0, 95.0)];
    assert_eq!(decide(&s, &live, &c, None, T), Decision::Alert);
}

#[test]
fn the_upgrade_never_trades_weekly_room() {
    let s = settings();
    // live headroom 30 (session 70); cand headroom 85 but week 15 > 10.
    let live = measured(70.0, 10.0);
    let c = [m("spare", 0.0, 15.0)];
    assert_eq!(decide(&s, &live, &c, None, T), Decision::Stay);
}

#[test]
fn the_upgrade_takes_the_first_eligible_login_in_rank_order_that_gains_enough() {
    let s = settings();
    // Live headroom 50. aanderson ranks first (week 10) but has headroom
    // 20; bbuck (week 15) has headroom 85, a gain of 35 >= 30.
    let live = measured(50.0, 50.0);
    let c = [m("aanderson", 80.0, 10.0), m("bbuck", 0.0, 15.0)];
    assert_eq!(
        decide(&s, &live, &c, None, T),
        swap_to("bbuck", Reason::Upgrade)
    );
}

#[test]
fn the_upgrade_never_takes_a_login_that_is_not_eligible() {
    let s = settings();
    // `wide` has the lowest session use, but its week is at week_at, so it
    // is not eligible and no upgrade goes to it.
    let live = measured(60.0, 80.0);
    let c = [m("wide", 0.0, 90.0)];
    assert!(!eligible(&s, &c[0].reading));
    assert_eq!(decide(&s, &live, &c, None, T), Decision::Stay);
}

#[test]
fn the_upgrade_still_respects_hold() {
    let s = settings();
    let live = measured(50.0, 50.0);
    let c = [m("bbuck", 0.0, 15.0)];
    assert_eq!(decide(&s, &live, &c, Some(T - 10), T), Decision::Stay);
}
