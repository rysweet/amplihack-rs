//! Issue #1804: the plan-room swap and explicit consent for the API-key
//! profile, in the rotator's pure decision. No I/O. See "Rotate plan-room
//! swap" and "Rotate decision order" in docs/reference/profile-command.md.
//!
//! Contract (`rotate::plan_room`): `has_room(&Reading) -> bool` is a
//! `Measured` reading with session and week both below 100; `Failed` and
//! `Exhausted` never have room. `room()` and `has_room` keep the 100% rule:
//! auto-switch calls them, and the `no_room` ledger record is read by it.
//!
//! Issue #2910 replaced `plan_room::choose` with the one target rule
//! (`decide::target`, tested in `tests_issue_2910_rule`): `Reason::PlanRoom`,
//! rendered `plan-room`, now labels a move onto a login with room that is at
//! or above `--session-at` or `--week-at`. The ranking tests below are the
//! `choose` tests, ported.
//!
//! Contract (`overflow::decide_overflow`, live on a plan login), D1:
//! 1. `decide`; a swap onto a plan login (with capacity or with room only,
//!    ranked by the one rule), or a stay, stands.
//! 2. Only when `decide` gave `Alert`, the live probe did not fail, and no
//!    plan login, live included, has room and the gate allows it: the first
//!    usable marked per-token profile by tier (credit tiers 1 to 9, then the
//!    marked API key at 10), `OverflowIn`.
//! 3. `Alert`.
//!
//! Live on a per-token profile: the best eligible plan login (the one
//! ranking), `OverflowOut`; then, off the API key with no plan login, a
//! usable credit profile (`OverflowOut`); off a blocked profile, the next
//! usable per-token one only when no plan login has room.

use super::decide::target::{NoMove, no_move, ranked};
use super::decide::{Decision, Measured, Probed, Reading, Reason, Win, Windows, decide};
use super::ledger::OverflowBlock;
use super::overflow::{OverflowInput, Tag, Verdict, decide_overflow};
use super::plan_room::{has_room, has_room_under, room_under};
use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::OverflowEntry;
use std::collections::BTreeMap;

const NOW: i64 = 1_793_389_376; // 2026-10-30T19:42:56Z
const HOUR: i64 = 3600;
const FAR: Option<i64> = Some(NOW + 4 * HOUR);
const NEAR: Option<i64> = Some(NOW + 600);

fn settings(session_at: u8) -> Settings {
    Settings {
        session_at,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 120,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
        target_week_max: 95,
    }
}

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn measured(session: f64, week: f64, reset: Option<i64>) -> Probed {
    Probed {
        reading: Reading::Measured { session, week },
        windows: Windows {
            session: Some(Win {
                used: session,
                resets_at: reset,
            }),
            week: Some(Win {
                used: week,
                resets_at: reset,
            }),
            no_session_limit: false,
        },
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at: None,
        carried: false,
        passive: false,
    }
}

fn exhausted(reset: Option<i64>) -> Probed {
    Probed {
        reading: Reading::EXHAUSTED,
        windows: Windows {
            session: Some(Win {
                used: 100.0,
                resets_at: reset,
            }),
            week: Some(Win {
                used: 100.0,
                resets_at: reset,
            }),
            no_session_limit: false,
        },
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at: None,
        carried: false,
        passive: false,
    }
}

fn m(name: &str, p: &Probed) -> Measured {
    Measured::new(n(name), p)
}

fn swap(to: &str, reason: Reason) -> Decision {
    Decision::Swap { to: n(to), reason }
}

/// One cycle: the live login, the candidates and the overflow entries.
struct Case {
    s: Settings,
    live_name: String,
    live: Probed,
    candidates: Vec<Measured>,
    overflow: Vec<OverflowEntry>,
    blocks: BTreeMap<String, OverflowBlock>,
    unmeasured: Vec<ProfileName>,
    refused: Vec<ProfileName>,
}

impl Case {
    fn new(live_name: &str, live: Probed) -> Self {
        Self {
            s: settings(85),
            live_name: live_name.into(),
            live,
            candidates: Vec::new(),
            overflow: Vec::new(),
            blocks: BTreeMap::new(),
            unmeasured: Vec::new(),
            refused: Vec::new(),
        }
    }

    fn at(mut self, session_at: u8) -> Self {
        self.s = settings(session_at);
        self
    }

    fn cand(mut self, name: &str, p: Probed) -> Self {
        self.candidates.push(m(name, &p));
        self
    }

    /// `api-key` marked with `overflow add`: in the overflow order at the
    /// API-key tier, and a candidate with its synthetic reading.
    fn marked_key(mut self) -> Self {
        self.overflow.push(OverflowEntry::platform(n("api-key")));
        self.candidates.push(m("api-key", &Probed::platform()));
        self
    }

    /// `credits`, a tier-1 credit profile, as a candidate.
    fn credits(mut self, p: Probed) -> Self {
        self.overflow.push(OverflowEntry::new(n("credits"), 1));
        self.candidates.push(m("credits", &p));
        self
    }

    /// A plan login skipped as busy: not measured this cycle.
    fn busy(mut self, name: &str) -> Self {
        self.unmeasured.push(n(name));
        self
    }

    /// A profile skipped as `directory refused` (issue #1698).
    fn refused(mut self, name: &str) -> Self {
        self.refused.push(n(name));
        self
    }

    fn spend_blocked(mut self, name: &str) -> Self {
        self.blocks.insert(
            name.into(),
            OverflowBlock {
                at: NOW - 60,
                until: NOW + 10 * 86_400,
                reason: "spend_limit".into(),
            },
        );
        self
    }

    fn run(&self) -> Verdict {
        let live_name = n(&self.live_name);
        decide_overflow(&OverflowInput {
            settings: &self.s,
            live_name: Some(&live_name),
            live: &self.live,
            candidates: &self.candidates,
            overflow: &self.overflow,
            blocks: &self.blocks,
            platform_block: None,
            unmeasured_plan: &self.unmeasured,
            refused: &self.refused,
            swapped_at: Some(NOW - 60),
            now: NOW,
        })
    }
}

/// The 19:42:56Z cycle from issue #1804: live iivanovich at 87/61 and
/// seven plan candidates. Every reset is four hours away.
fn replay() -> Case {
    Case::new("iivanovich", measured(87.0, 61.0, FAR))
        .cand("aanderson", measured(25.0, 95.0, FAR))
        .cand("bbuck", measured(50.0, 97.0, FAR))
        .cand("ccapstone", measured(11.0, 92.0, FAR))
        .cand("ddrumpf", measured(0.0, 92.0, FAR))
        .cand("eelsworth", measured(98.0, 17.0, FAR))
        .cand("ggrant", exhausted(FAR))
        .cand("ryan-mistt-personal", measured(0.0, 96.0, FAR))
}

// --- Reason ------------------------------------------------------------------------

#[test]
fn the_plan_room_reason_is_logged_as_plan_room() {
    assert_eq!(Reason::PlanRoom.as_str(), "plan-room");
}

// --- has_room ----------------------------------------------------------------------

#[test]
fn room_means_a_measured_reading_below_100_in_both_windows() {
    let r = |s, w| Reading::Measured {
        session: s,
        week: w,
    };
    assert!(has_room(&r(0.0, 0.0)));
    assert!(has_room(&r(99.9, 99.9)));
    assert!(has_room(&r(98.0, 17.0)), "past --session-at still has room");
    assert!(!has_room(&r(100.0, 10.0)));
    assert!(!has_room(&r(10.0, 100.0)));
    assert!(
        !has_room(&r(f64::NAN, 10.0)),
        "an unreadable value is no room"
    );
    assert!(!has_room(&Reading::EXHAUSTED));
    assert!(!has_room(&Reading::Failed), "a failed probe never has room");
}

// --- The ranking (issue #2910 replaced `choose`) ------------------------------------
//
// Issue #2910 moved the plan-room choice into the one target rule
// (`decide::target`). These tests are the `choose` tests, ported, with the
// rulings of #2910 where the expectation changed:
//
// - D1: a live login that still has room moves only for `max(--margin, 1)`
//   more room now, so the replay (live 87/61, room 13) stays where `choose`
//   moved it to a login with room 8.
// - D2: a live login with no room takes any eligible login, so a login at or
//   above `--session-at` is a target again (reversing #2857 for that case);
//   with room on the live login the margin and the hold keep #2857's guard.
// - F1: the week bound is `--target-week-max` (95 by default), inclusive.
// - F3: tier, then room now, then week, then name; session is not a key
//   beyond the tier.

fn plan_of(c: &Case) -> Vec<Measured> {
    c.candidates.clone()
}

fn ranked_names(s: &Settings, plan: &[Measured]) -> Vec<String> {
    ranked(s, plan, 0)
        .into_iter()
        .map(|(n, _)| n.to_string())
        .collect()
}

fn full() -> Reading {
    Reading::Exhausted {
        session: Some(100.0),
        week: Some(61.0),
    }
}

/// D1: the replay's live login has room 13; nothing has 18.
#[test]
fn the_replay_stays_on_a_live_login_with_more_room_than_any_other() {
    let c = replay();
    let d = decide(&c.s, &c.live.reading, &plan_of(&c), None, NOW);
    assert_eq!(d, Decision::Alert);
    assert_eq!(
        no_move(&c.s, &c.live.reading, &plan_of(&c), None, NOW),
        NoMove::LessRoom {
            most: 8,
            room: 13,
            needs: 18
        }
    );
}

/// D2 and F3: with the live login full, ccapstone and ddrumpf both have
/// room 8 and week 92, so the name decides; aanderson (25/95, room 5) is at
/// the ceiling and eligible but has less room; bbuck and the 96% login are
/// above the ceiling.
#[test]
fn the_replay_with_the_live_login_full_takes_the_most_room_now() {
    let c = replay();
    let d = decide(&c.s, &full(), &plan_of(&c), None, NOW);
    assert_eq!(d, swap("ccapstone", Reason::Threshold));
    assert_eq!(
        ranked_names(&c.s, &plan_of(&c)),
        ["ccapstone", "ddrumpf", "aanderson", "eelsworth"]
    );
    let at_70 = replay().at(70);
    assert_eq!(
        decide(&at_70.s, &full(), &plan_of(&at_70), None, NOW),
        swap("ccapstone", Reason::Threshold)
    );
}

#[test]
fn the_ranking_breaks_a_full_tie_by_name() {
    let s = settings(85);
    let plan = vec![
        m("zed", &measured(10.0, 50.0, FAR)),
        m("abe", &measured(10.0, 50.0, FAR)),
    ];
    assert_eq!(ranked_names(&s, &plan), ["abe", "zed"]);
}

/// D2 reverses #2857 for a live login with no room: every login past
/// `--session-at` is still a target, the one with the most room first. With
/// room on the live login, the margin keeps the move from happening.
#[test]
fn logins_past_session_at_are_targets_only_when_the_live_login_has_no_room() {
    let s = settings(85);
    let plan = vec![
        m("spare", &measured(95.0, 40.0, FAR)),
        m("extra", &measured(88.0, 60.0, FAR)),
        m("gone", &exhausted(FAR)),
    ];
    assert_eq!(
        decide(&s, &full(), &plan, None, NOW),
        swap("extra", Reason::Threshold)
    );
    let live = Reading::Measured {
        session: 86.0,
        week: 50.0,
    };
    assert_eq!(decide(&s, &live, &plan, None, NOW), Decision::Alert);
}

/// The tier boundary is strict: a login exactly at `--session-at` is in the
/// second tier, one point under is in the first.
#[test]
fn a_login_exactly_at_session_at_ranks_in_the_second_tier() {
    let s = settings(85);
    let plan = vec![
        m("at", &measured(85.0, 0.0, FAR)),
        m("under", &measured(84.9, 80.0, FAR)),
    ];
    assert_eq!(ranked_names(&s, &plan), ["under", "at"]);
}

/// The 19 Nov 2026 swaps: four logins in a row were at 98-100% session. A
/// live login among them has room, and no other has the margin more, so the
/// rotator does not move between them.
#[test]
fn logins_at_98_to_100_percent_session_do_not_trade_places() {
    let s = settings(85);
    let plan = vec![
        m("bbuck", &measured(99.0, 40.0, FAR)),
        m("ccapstone", &measured(99.9, 20.0, FAR)),
        m("ddrumpf", &measured(100.0, 10.0, FAR)),
    ];
    let live = Reading::Measured {
        session: 98.0,
        week: 60.0,
    };
    assert_eq!(decide(&s, &live, &plan, None, NOW), Decision::Alert);
}

/// `--session-at` decides the tier across its range, and a NaN session is
/// never ranked.
#[test]
fn the_tier_follows_session_at_and_a_nan_never_ranks() {
    let plan = vec![
        m("high", &measured(99.0, 0.0, FAR)),
        m("low", &measured(50.0, 40.0, FAR)),
    ];
    assert_eq!(ranked_names(&settings(100), &plan), ["low", "high"]);
    assert_eq!(ranked_names(&settings(51), &plan), ["low", "high"]);
    assert_eq!(ranked_names(&settings(50), &plan), ["low", "high"]);
    let nan = vec![m("spare", &measured(f64::NAN, 40.0, FAR))];
    for at in [1, 50, 100] {
        assert!(
            ranked_names(&settings(at), &nan).is_empty(),
            "session_at {at}"
        );
    }
}

/// F1: the week bound is `--target-week-max`, inclusive, not `--week-at`.
#[test]
fn the_week_bound_is_the_target_week_max() {
    let s = settings(85);
    assert_eq!(
        ranked_names(&s, &[m("spare", &measured(10.0, 95.0, FAR))]),
        ["spare"]
    );
    assert!(ranked_names(&s, &[m("spare", &measured(10.0, 95.5, FAR))]).is_empty());
    assert!(ranked_names(&s, &[m("spare", &measured(10.0, 100.0, FAR))]).is_empty());
}

#[test]
fn the_ranking_leaves_out_failed_exhausted_and_full_logins() {
    let s = settings(85);
    let plan = vec![
        m("failed", &Probed::failed()),
        m("gone", &exhausted(FAR)),
        m("full", &measured(100.0, 1.0, FAR)),
    ];
    assert!(ranked_names(&s, &plan).is_empty());
}

/// D1: two logins with the same room do not trade places: a move needs
/// `max(--margin, 1)` more room now.
#[test]
fn a_login_with_the_same_room_as_the_live_one_is_not_a_target() {
    let s = settings(85);
    let live = Reading::Measured {
        session: 0.0,
        week: 92.0,
    };
    let plan = vec![m("ccapstone", &measured(11.0, 92.0, FAR))];
    assert_eq!(decide(&s, &live, &plan, None, NOW), Decision::Alert);
    let live = Reading::Measured {
        session: 10.0,
        week: 95.0,
    };
    let plan = vec![m("spare", &measured(92.0, 20.0, FAR))];
    assert_eq!(decide(&s, &live, &plan, None, NOW), Decision::Alert);
}

// --- decide_overflow: T1 and T2 ------------------------------------------------------

// Issue #2910 changed these three expectations and kept the fixture (ruling
// D1): the replay's live login (87/61) has room 13, and no plan login has the
// 18 a move needs, so it stays (the ALERT is the stay note). The API key is
// still never chosen while a plan login has room.

#[test]
fn t1_the_replay_with_no_mark_stays_on_the_login_with_the_most_room() {
    let v = replay().run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.tag, None, "not an overflow move");
}

#[test]
fn t1_at_session_at_70_stays_too() {
    let v = replay().at(70).run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
}

#[test]
fn t2_the_replay_with_the_api_key_marked_and_the_gate_open_still_stays() {
    let v = replay().marked_key().run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(
        v.tag, None,
        "the API key is never chosen while a plan login has room"
    );
}

// --- decide_overflow: the plan-room swap -------------------------------------------

#[test]
fn the_flapping_guard_keeps_ddrumpf_live() {
    let v = Case::new("ddrumpf", measured(0.0, 92.0, FAR))
        .cand("ccapstone", measured(11.0, 92.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
}

/// Issue #2910 changed this expectation (ruling D1): ccapstone and ddrumpf
/// both have room 8, so neither leaves the other; the margin, with a floor
/// of 1, is the flapping guard now.
#[test]
fn the_margin_keeps_ccapstone_from_moving_to_a_login_with_the_same_room() {
    let v = Case::new("ccapstone", measured(11.0, 92.0, FAR))
        .cand("ddrumpf", measured(0.0, 92.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
}

/// Issue #2857 changed this test's expectation and kept it: with every login
/// past `--session-at` (85) the rotator no longer swaps onto the one with the
/// lowest week. The login it stays on has room, so no credit is spent either:
/// the cycle is the ALERT (advisory while the live login reads below 100%).
#[test]
fn no_plan_room_swap_when_every_login_is_past_session_at() {
    let v = Case::new("work", measured(90.0, 50.0, FAR))
        .cand("spare", measured(95.0, 40.0, FAR))
        .cand("extra", measured(88.0, 60.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.tag, None);
}

/// Issue #2857 changed this test's expectation and kept it. No login has
/// session room (--session-at 85); the first cycle used to swap on the week
/// ranking. Now neither cycle swaps, in either direction, so there is nothing
/// to flap.
#[test]
fn d5_with_no_login_under_session_at_neither_cycle_swaps_in_either_direction() {
    let first = Case::new("work", measured(90.0, 80.0, FAR))
        .cand("spare", measured(95.0, 60.0, FAR))
        .run();
    assert_eq!(first.decision, Decision::Alert, "{first:?}");
    // The next cycle, over the same readings, from the other login.
    let second = Case::new("spare", measured(95.0, 60.0, FAR))
        .cand("work", measured(90.0, 80.0, FAR))
        .run();
    assert_eq!(second.decision, Decision::Alert, "no swap back: {second:?}");
    assert_eq!(second.tag, None);
}

/// Issue #2910 changed the first expectation (ruling D1): `work` (90/80) has
/// room 10 and spare (80/95) has room 5, so the first cycle stays. From
/// spare, `work` has the margin more, but it is past `--session-at` and the
/// last swap is a minute old, so the hold (D3) keeps the second cycle too.
#[test]
fn d5_neither_cycle_moves_between_a_login_with_more_room_and_one_with_less() {
    let first = Case::new("work", measured(90.0, 80.0, FAR))
        .cand("spare", measured(80.0, 95.0, FAR))
        .run();
    assert_eq!(first.decision, Decision::Alert, "{first:?}");
    let second = Case::new("spare", measured(80.0, 95.0, FAR))
        .cand("work", measured(90.0, 80.0, FAR))
        .run();
    assert_eq!(second.decision, Decision::Alert, "no flap back: {second:?}");
}

/// Issue #2857 changed this test's expectation and kept it. An exhausted live
/// login used to give way to a login at 99% session. That login is far past
/// `--session-at`, so the rotator no longer swaps onto it; it has room, so no
/// credit is spent either.
#[test]
fn d5_an_exhausted_live_login_does_not_swap_onto_a_login_at_99_percent() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", measured(99.0, 99.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.tag, None);
}

/// Issue #2910 changed this expectation (ruling D1): the margin applies to
/// every move from a live login that still has room. Spare's room (13) is
/// short of the live login's (10) plus --margin 5, so the rotator stays.
#[test]
fn d5_the_margin_applies_to_a_move_from_a_live_login_with_room() {
    let v = Case::new("work", measured(90.0, 61.0, FAR))
        .cand("spare", measured(80.0, 87.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
}

#[test]
fn a_live_login_past_session_at_with_the_lowest_week_is_not_swapped_away_from() {
    let v = Case::new("work", measured(90.0, 30.0, FAR))
        .cand("spare", measured(95.0, 40.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
}

#[test]
fn a_threshold_swap_that_meets_the_margin_is_not_replaced() {
    let v = Case::new("work", measured(90.0, 40.0, FAR))
        .cand("spare", measured(10.0, 20.0, FAR))
        .cand("other", measured(0.0, 5.0, FAR))
        .run();
    assert_eq!(v.decision, swap("other", Reason::Threshold), "{v:?}");
}

/// The issue's cycle with the live login further along. Issue #2910 changed
/// this expectation (ruling F3): ccapstone and ddrumpf tie on tier, room now
/// (8) and week (92), so the name decides; session is not a key beyond the
/// tier. `decide_overflow` gives the threshold swap; the cycle labels it
/// `plan-room` (D15), since the target's week is past --week-at.
#[test]
fn a_threshold_swap_without_capacity_follows_the_one_ranking() {
    let v = Case::new("iivanovich", measured(98.0, 61.0, FAR))
        .cand("ccapstone", measured(11.0, 92.0, FAR))
        .cand("ddrumpf", measured(0.0, 92.0, FAR))
        .cand("eelsworth", measured(98.0, 17.0, FAR))
        .run();
    assert_eq!(v.decision, swap("ccapstone", Reason::Threshold), "{v:?}");
}

#[test]
fn plan_room_never_fires_below_both_thresholds() {
    // No threshold reached and no upgrade: a stay, however much week room
    // another login has.
    let v = Case::new("work", measured(50.0, 80.0, FAR))
        .cand("spare", measured(60.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Stay, "{v:?}");
}

/// Issue #2910 changed this expectation (ruling D2, reversing #2857 for this
/// case): an exhausted live login has no room, so it moves to any eligible
/// login, spare at 97% session included.
#[test]
fn an_exhausted_live_login_moves_onto_a_login_at_97_percent_session() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", measured(97.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold), "{v:?}");
}

/// Issue #2910 (F1): a login above the week ceiling (95) is never a target,
/// whatever its session; it still has room, so no credit is spent either.
/// At the ceiling it is a target.
#[test]
fn an_exhausted_live_login_never_moves_onto_a_login_above_the_week_ceiling() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", measured(50.0, 97.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", measured(50.0, 95.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold), "{v:?}");
}

#[test]
fn plan_room_is_skipped_when_the_live_probe_failed() {
    let v = Case::new("work", Probed::failed())
        .cand("spare", measured(97.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
}

#[test]
fn a_failed_live_probe_never_moves_onto_the_api_key() {
    let v = Case::new("work", Probed::failed())
        .cand("spare", exhausted(FAR))
        .marked_key()
        .run();
    assert_ne!(v.decision, swap("api-key", Reason::OverflowIn), "{v:?}");
    assert_eq!(v.tag, None);
}

#[test]
fn plan_room_comes_before_a_usable_credit_profile() {
    // D1: credit profiles are billed per token too, so a plan login with
    // room wins over them. Issue #2910: the live login has no room, so any
    // eligible login qualifies; the fixture's week moved from 97 to 94, under
    // the week ceiling (F1). The cycle labels the move `plan-room` (D15).
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(60.0, 94.0, FAR))
        .credits(measured(10.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold), "{v:?}");
    assert_eq!(v.tag, None, "not an overflow move");
}

/// Issue #2910 changed this expectation (ruling D2, reversing #2857 for this
/// case): the live login is at 100%, so it has no room and moves to the
/// login at 99% session, which has 1 point; no credit is spent.
#[test]
fn a_full_live_login_moves_to_a_plan_login_at_99_percent_before_credits() {
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(99.0, 50.0, FAR))
        .credits(measured(10.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold), "{v:?}");
    assert_eq!(v.tag, None, "no credit spend while a plan login has room");
}

/// Issue #2910 changed these two expectations (ruling D1): the replay's live
/// login keeps its room 13, and no credit is spent while a plan login has
/// room.
#[test]
fn d1_the_replay_with_a_tier_1_credit_profile_stays() {
    let v = replay().credits(measured(5.0, 5.0, FAR)).run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.tag, None, "no credit spend while a plan login has room");
}

#[test]
fn d1_the_replay_with_credits_and_the_api_key_marked_stays() {
    let v = replay().credits(measured(5.0, 5.0, FAR)).marked_key().run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.tag, None);
}

#[test]
fn credits_wait_while_the_live_login_still_has_room() {
    // The live login is past --session-at but below 100% and wins pass 2:
    // no plan-room swap, and no credit spend either, since it has room.
    let v = Case::new("work", measured(87.0, 61.0, FAR))
        .cand("spare", exhausted(FAR))
        .credits(measured(10.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.tag, None);
}

#[test]
fn credits_wait_while_a_failed_live_probe_leaves_the_state_unknown() {
    let v = Case::new("work", Probed::failed())
        .cand("spare", exhausted(FAR))
        .credits(measured(10.0, 10.0, FAR))
        .run();
    assert_ne!(v.decision, swap("credits", Reason::OverflowIn), "{v:?}");
    assert_ne!(v.tag, Some(Tag::In));
}

#[test]
fn credits_are_used_when_no_plan_login_has_room_and_the_gate_allows() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", measured(100.0, 30.0, FAR))
        .credits(measured(10.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, swap("credits", Reason::OverflowIn), "{v:?}");
    assert_eq!(v.tag, Some(Tag::In));
}

#[test]
fn credits_still_need_the_gate() {
    let v = Case::new("work", exhausted(NEAR))
        .cand("spare", exhausted(NEAR))
        .credits(measured(10.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.tag, None);
}

#[test]
fn credit_tiers_are_tried_in_tier_order_before_the_api_key() {
    let mut c = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .marked_key();
    c.overflow.push(OverflowEntry::new(n("late"), 3));
    c.candidates.push(m("late", &measured(1.0, 1.0, FAR)));
    c.overflow.push(OverflowEntry::new(n("early"), 2));
    c.candidates.push(m("early", &measured(50.0, 50.0, FAR)));
    let v = c.run();
    assert_eq!(v.decision, swap("early", Reason::OverflowIn), "{v:?}");
}

#[test]
fn plan_room_comes_before_the_api_key() {
    // Issue #2910: the fixture's week moved from 97 to 94, under the week
    // ceiling (F1); the live login has no room, so spare qualifies.
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(60.0, 94.0, FAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold), "{v:?}");
    assert_eq!(v.tag, None);
}

/// Issue #2910 changed this expectation (ruling D2): the full live login
/// moves to the plan login at 99% session, and the API key is not used.
#[test]
fn a_full_live_login_moves_to_a_plan_login_at_99_percent_before_the_api_key() {
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(99.0, 50.0, FAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold), "{v:?}");
    assert_eq!(v.tag, None);
}

// --- decide_overflow: the API key -------------------------------------------------

#[test]
fn the_api_key_is_used_when_every_plan_login_is_at_100_and_it_is_marked() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .cand("extra", measured(100.0, 100.0, FAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, swap("api-key", Reason::OverflowIn), "{v:?}");
    assert_eq!(v.tag, Some(Tag::In));
}

#[test]
fn the_api_key_is_never_used_unmarked_even_when_every_plan_login_is_at_100() {
    // Unmarked: not in the overflow order and, after the snapshot filter,
    // not a candidate.
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.tag, None);
}

#[test]
fn the_api_key_is_not_used_while_the_live_login_still_has_room() {
    // The issue's own case: live past --session-at but below 100%, nothing
    // else with room. Plan-room keeps it (the live login wins pass 2), and
    // the API key is refused because the live login has room.
    let v = Case::new("work", measured(87.0, 61.0, FAR))
        .cand("spare", exhausted(FAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.tag, None);
}

#[test]
fn a_failed_plan_probe_keeps_the_api_key_out_and_names_the_login() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", Probed::failed())
        .marked_key()
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.key_held, [n("spare")]);
}

#[test]
fn a_busy_plan_login_keeps_the_api_key_out_and_names_the_login() {
    // Crusty round 13: `work` at 100%, `ccapstone` with a session on it at
    // s=11% w=92%. It is skipped, so nothing here knows it has room.
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .busy("ccapstone")
        .marked_key()
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.tag, None);
    assert_eq!(v.key_held, [n("ccapstone")]);
}

#[test]
fn every_plan_login_known_lets_the_api_key_in_and_raises_no_key_held() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, swap("api-key", Reason::OverflowIn), "{v:?}");
    assert!(v.key_held.is_empty() && v.unknown_plan.is_empty(), "{v:?}");
}

#[test]
fn an_unknown_plan_login_does_not_name_itself_when_the_gate_stopped_the_key() {
    // The wait said no, so the unknown login is not why the key was unused.
    let v = Case::new("work", exhausted(NEAR))
        .cand("spare", exhausted(NEAR))
        .busy("ccapstone")
        .marked_key()
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert!(v.key_held.is_empty(), "{v:?}");
}

#[test]
fn credit_tiers_do_not_wait_for_a_busy_login_or_a_read_state_and_report_the_count() {
    // `Failed` without `unknown`: the probe read a state such as
    // `not_logged_in`. A timeout holds credits too; see the #1698 tests below.
    let read_a_state = Probed {
        unknown: false,
        ..Probed::failed()
    };
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", read_a_state)
        .busy("ccapstone")
        .credits(measured(10.0, 10.0, FAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, swap("credits", Reason::OverflowIn), "{v:?}");
    assert_eq!(v.unknown_plan, [n("ccapstone"), n("spare")]);
    assert!(v.key_held.is_empty(), "{v:?}");
}

#[test]
fn blocked_credits_do_not_move_to_the_api_key_past_a_busy_plan_login() {
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", exhausted(FAR))
        .busy("ccapstone")
        .marked_key()
        .spend_blocked("credits");
    c.overflow.push(OverflowEntry::new(n("credits"), 1));
    let v = c.run();
    assert_ne!(v.decision, swap("api-key", Reason::OverflowIn), "{v:?}");
}

#[test]
fn the_api_key_still_needs_the_gate() {
    let v = Case::new("work", exhausted(NEAR))
        .cand("spare", exhausted(NEAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
}

#[test]
fn credits_still_come_before_the_api_key() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .credits(measured(10.0, 10.0, FAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, swap("credits", Reason::OverflowIn), "{v:?}");
}

// --- Off the API key and off blocked credits -----------------------------------------

#[test]
fn on_the_api_key_a_plan_login_with_room_is_a_way_off() {
    // Issue #2857: "room" for a move is now under --session-at (85); work is
    // at 60% session and short of capacity (its week is past --week-at).
    let mut c = Case::new("api-key", Probed::platform())
        .cand("work", measured(60.0, 95.0, FAR))
        .cand("spare", exhausted(FAR));
    c.overflow.push(OverflowEntry::platform(n("api-key")));
    let v = c.run();
    assert_eq!(v.decision, swap("work", Reason::OverflowOut), "{v:?}");
    assert_eq!(v.tag, Some(Tag::Out));
}

/// Issue #2910 changed this expectation and kept the fixture (ruling D2,
/// reversing #2857 for an overflow live login): every eligible plan login is
/// a way off paid spend, a login at 95% session included.
#[test]
fn on_the_api_key_a_plan_login_past_session_at_is_a_way_off() {
    let mut c = Case::new("api-key", Probed::platform())
        .cand("work", measured(95.0, 40.0, FAR))
        .cand("spare", exhausted(FAR));
    c.overflow.push(OverflowEntry::platform(n("api-key")));
    let v = c.run();
    assert_eq!(v.decision, swap("work", Reason::OverflowOut), "{v:?}");
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn on_the_api_key_plan_room_comes_before_a_usable_credit_profile() {
    let mut c = Case::new("api-key", Probed::platform())
        .cand("work", measured(60.0, 95.0, FAR))
        .credits(measured(10.0, 10.0, FAR));
    c.overflow.push(OverflowEntry::platform(n("api-key")));
    let v = c.run();
    assert_eq!(v.decision, swap("work", Reason::OverflowOut), "{v:?}");
    assert_eq!(v.tag, Some(Tag::Out));
}

/// Issue #2910 changed this expectation and kept the fixture (ruling D2): a
/// plan login with room, at 95% session, costs nothing, so it comes before
/// the credit profile.
#[test]
fn on_the_api_key_a_plan_login_past_session_at_comes_before_a_usable_credit_profile() {
    let mut c = Case::new("api-key", Probed::platform())
        .cand("work", measured(95.0, 40.0, FAR))
        .credits(measured(10.0, 10.0, FAR));
    c.overflow.push(OverflowEntry::platform(n("api-key")));
    let v = c.run();
    assert_eq!(v.decision, swap("work", Reason::OverflowOut), "{v:?}");
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn on_the_api_key_with_no_plan_room_a_usable_credit_profile_is_the_way_off() {
    let mut c = Case::new("api-key", Probed::platform())
        .cand("work", exhausted(FAR))
        .credits(measured(10.0, 10.0, FAR));
    c.overflow.push(OverflowEntry::platform(n("api-key")));
    let v = c.run();
    assert_eq!(v.decision, swap("credits", Reason::OverflowOut), "{v:?}");
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn on_a_credit_profile_a_plan_login_with_room_is_a_way_off() {
    // 60/95 at --session-at 85: no capacity (the week is past --week-at), but
    // room under the session threshold. Credits cost money, so the rotator
    // returns to the plan login. Issue #2857: the fixture was 92/40, which is
    // past --session-at; see the next test.
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", measured(60.0, 95.0, FAR))
        .cand("spare", exhausted(FAR));
    c.overflow.push(OverflowEntry::new(n("credits"), 1));
    let v = c.run();
    assert_eq!(v.decision, swap("work", Reason::OverflowOut), "{v:?}");
    assert_eq!(v.tag, Some(Tag::Out));
}

/// Issue #2910 changed this expectation and kept the fixture (92/40 at
/// `--session-at 85`; ruling D2): credits cost money, and a plan login with
/// room is a way off them whatever its session.
#[test]
fn on_a_credit_profile_a_plan_login_past_session_at_is_a_way_off() {
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", measured(92.0, 40.0, FAR))
        .cand("spare", exhausted(FAR));
    c.overflow.push(OverflowEntry::new(n("credits"), 1));
    let v = c.run();
    assert_eq!(v.decision, swap("work", Reason::OverflowOut), "{v:?}");
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn on_a_credit_profile_with_no_plan_room_it_stays() {
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", exhausted(FAR))
        .cand("spare", Probed::failed());
    c.overflow.push(OverflowEntry::new(n("credits"), 1));
    let v = c.run();
    assert_eq!(v.decision, Decision::Stay, "{v:?}");
    assert_eq!(v.tag, Some(Tag::On));
}

#[test]
fn on_a_credit_profile_a_marked_api_key_is_never_a_way_off_while_unblocked() {
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", exhausted(FAR))
        .marked_key();
    c.overflow.push(OverflowEntry::new(n("credits"), 1));
    let v = c.run();
    assert_ne!(v.decision, swap("api-key", Reason::OverflowIn), "{v:?}");
}

#[test]
fn on_the_api_key_with_no_plan_room_it_stays() {
    let mut c = Case::new("api-key", Probed::platform())
        .cand("work", exhausted(FAR))
        .cand("spare", Probed::failed());
    c.overflow.push(OverflowEntry::platform(n("api-key")));
    let v = c.run();
    assert_eq!(v.decision, Decision::Stay, "{v:?}");
    assert_eq!(v.tag, Some(Tag::On));
}

#[test]
fn blocked_credits_move_to_a_plan_login_with_room_not_to_the_api_key() {
    // Issue #2857: the plan login must be under --session-at (85); work is
    // at 60% session with no capacity (its week is past --week-at).
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", measured(60.0, 95.0, FAR))
        .marked_key()
        .spend_blocked("credits");
    c.overflow.push(OverflowEntry::new(n("credits"), 1));
    let got = c.run();
    assert_ne!(got.decision, swap("api-key", Reason::OverflowIn), "{got:?}");
    assert_eq!(got.decision, swap("work", Reason::OverflowOut), "{got:?}");
}

/// Issue #2910 changed this expectation and kept the fixture (ruling D2,
/// reversing #2857's consequence here): the plan login at 95% session has
/// room, so it is the way off the spend-blocked credit profile, before the
/// marked API key.
#[test]
fn blocked_credits_with_the_only_plan_login_past_session_at_move_to_it() {
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", measured(95.0, 40.0, FAR))
        .marked_key()
        .spend_blocked("credits");
    c.overflow.push(OverflowEntry::new(n("credits"), 1));
    let got = c.run();
    assert_eq!(got.decision, swap("work", Reason::OverflowOut), "{got:?}");
    assert_eq!(got.tag, Some(Tag::Out));
}

#[test]
fn blocked_credits_move_to_the_api_key_when_no_plan_login_has_room() {
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", exhausted(FAR))
        .marked_key()
        .spend_blocked("credits");
    c.overflow.push(OverflowEntry::new(n("credits"), 1));
    let v = c.run();
    assert_eq!(v.decision, swap("api-key", Reason::OverflowIn), "{v:?}");
}

// --- PlatformTierFixed ---------------------------------------------------------------

#[test]
fn platform_tier_fixed_names_the_profile_and_says_why() {
    use crate::commands::profile::ProfileError;
    let e = ProfileError::PlatformTierFixed("console".into());
    assert_eq!(
        e.to_string(),
        "profile console is an API-key profile; the API-key tier is fixed at 10, \
         so --tier cannot be set"
    );
    assert!(e.hint().contains("without --tier"), "{}", e.hint());
}

// --- Issue #1698 with issue #1804: both holds, after the merge --------------

fn all_out() -> Case {
    Case::new("work", exhausted(FAR)).cand("spare", exhausted(FAR))
}

#[test]
fn an_unknown_plan_probe_holds_credits_and_the_api_key() {
    let v = all_out()
        .cand("timedout", Probed::failed())
        .credits(measured(0.0, 0.0, FAR))
        .marked_key()
        .run();
    assert_eq!(v.tag, None, "{v:?}");
    assert_eq!(v.unknown_plan, [n("timedout")]);
}

#[test]
fn a_refused_plan_profile_holds_credits_and_counts_as_unknown() {
    let v = all_out()
        .refused("locked")
        .credits(measured(0.0, 0.0, FAR))
        .run();
    assert_eq!(v.tag, None, "{v:?}");
    assert_eq!(v.unknown_plan, [n("locked")]);
}

#[test]
fn a_refused_plan_profile_alone_keeping_the_key_back_is_named_for_the_alert() {
    let v = all_out().refused("locked").marked_key().run();
    assert_eq!(v.tag, None, "{v:?}");
    assert_eq!(v.key_held, [n("locked")]);
}

#[test]
fn a_refused_overflow_profile_holds_nothing() {
    let v = all_out()
        .credits(measured(0.0, 0.0, FAR))
        .refused("credits")
        .run();
    assert_eq!(v.decision, swap("credits", Reason::OverflowIn), "{v:?}");
}

#[test]
fn neither_hold_stops_the_plan_room_swap_which_costs_nothing() {
    for c in [
        Case::new("work", exhausted(FAR)).cand("timedout", Probed::failed()),
        Case::new("work", exhausted(FAR)).refused("locked"),
    ] {
        // Issue #2910: the fixture's week moved from 97 to 94, under the
        // week ceiling (F1); the cycle labels the move `plan-room` (D15).
        let v = c.cand("spare", measured(50.0, 94.0, FAR)).run();
        assert_eq!(v.decision, swap("spare", Reason::Threshold), "{v:?}");
    }
}

// --- Issue #2857: one comparison, shared -------------------------------------------
//
// `room_under` is what the rotator records as `live_room` on its cycle (the
// issue #2857 rule). It adds `session < --session-at` to `room` and takes
// nothing away: `room` and `has_room` keep their meaning for auto-switch.

#[test]
fn room_under_is_room_and_a_session_strictly_below_session_at() {
    assert!(room_under(84.9, 40.0, 85));
    assert!(
        !room_under(85.0, 40.0, 85),
        "a login at the threshold is at it, not under it"
    );
    assert!(!room_under(85.1, 40.0, 85));
    assert!(!room_under(98.0, 40.0, 85), "the 19 November logins");
    assert!(room_under(0.0, 99.9, 85));
    assert!(!room_under(0.0, 100.0, 85), "a week at 100 is no room");
    assert!(
        !room_under(f64::NAN, 40.0, 85) && !room_under(10.0, f64::NAN, 85),
        "an unreadable value is never room"
    );
}

/// At `--session-at 100` the bound is the 100% rule `room` already is.
#[test]
fn room_under_at_a_session_at_of_100_is_room() {
    for (session, week) in [(0.0, 0.0), (99.9, 99.9), (100.0, 0.0), (0.0, 100.0)] {
        assert_eq!(
            room_under(session, week, 100),
            super::plan_room::room(session, week),
            "{session}/{week}"
        );
    }
}

#[test]
fn has_room_under_needs_a_measured_reading() {
    assert!(has_room_under(&measured(10.0, 40.0, FAR).reading, 85));
    assert!(!has_room_under(&measured(90.0, 40.0, FAR).reading, 85));
    assert!(!has_room_under(&Probed::failed().reading, 85));
    assert!(!has_room_under(&exhausted(FAR).reading, 85));
}

/// SEC-1, ported (issue #2910): the ranking only narrows. Over a grid of
/// readings and thresholds, a lone plan login is ranked exactly when the one
/// eligibility test passes, and whatever is ranked has the room `has_room`
/// asks for. Ruling (D2): with the live login full a login at or above
/// `--session-at` is ranked, so the old equality with `has_room_under` no
/// longer holds; the bound is the week ceiling instead.
#[test]
fn a_lone_login_is_ranked_exactly_when_it_is_eligible_and_never_without_room() {
    let values = [
        0.0,
        50.0,
        84.9,
        85.0,
        90.0,
        95.0,
        98.0,
        99.9,
        100.0,
        f64::NAN,
    ];
    for session_at in [1u8, 50, 85, 100] {
        let s = settings(session_at);
        for session in values {
            for week in values {
                let probed = measured(session, week, FAR);
                let lone = [m("spare", &probed)];
                let picked = !ranked(&s, &lone, 0).is_empty();
                let eligible = session < 100.0 && week <= 95.0 && session >= 0.0;
                assert_eq!(
                    picked, eligible,
                    "session_at {session_at}, {session}/{week}"
                );
                if picked {
                    assert!(has_room(&probed.reading), "{session}/{week}");
                }
            }
        }
    }
}

/// The incident's own ledger line. Ruling (D1, D2): while the live login has
/// room no other login here has the margin more, so there is no swap; once
/// it has none, the login with the most room is the target.
#[test]
fn a_plan_whose_logins_all_have_room_but_none_under_session_at() {
    let s = settings(85);
    let plan = vec![
        m("aanderson", &measured(98.0, 60.0, FAR)),
        m("bbuck", &measured(99.0, 40.0, FAR)),
    ];
    assert!(plan.iter().all(|p| has_room(&p.reading)), "all have room");
    let live = Reading::Measured {
        session: 97.0,
        week: 60.0,
    };
    assert_eq!(decide(&s, &live, &plan, None, NOW), Decision::Alert);
    assert_eq!(
        decide(&s, &full(), &plan, None, NOW),
        swap("aanderson", Reason::Threshold)
    );
}

#[path = "tests_issue_1804_msg.rs"]
mod tests_issue_1804_msg;
