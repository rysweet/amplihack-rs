//! Issue #1804: the plan-room swap and explicit consent for the API-key
//! profile, in the rotator's pure decision. No I/O. See "Rotate plan-room
//! swap" and "Rotate decision order" in docs/reference/profile-command.md.
//!
//! Contract (`rotate::plan_room`):
//! - `has_room(&Reading) -> bool`: a `Measured` reading with session and
//!   week both below 100. `Failed` and `Exhausted` never have room.
//! - `choose(&Settings, live: Option<&Measured>, plan: &[Measured]) ->
//!   Option<ProfileName>`: one pass over the live login and the plan logins,
//!   taking those with session < `session_at` and week < 100. Issue #2857:
//!   the rotator does not plan-room swap onto a login already at or above its
//!   session threshold, so the second pass that took any login with room
//!   (session < 100) is gone. Ranked by week, then session, then name.
//!   `None` when nothing qualifies or when the winner is the live login (the
//!   flapping guard). `room()` and `has_room` keep the 100% rule: auto-switch
//!   calls them, and the `no_room` ledger record is read by it.
//! - `Reason::PlanRoom`, rendered `plan-room`.
//!
//! Contract (`overflow::decide_overflow`, live on a plan login), D1:
//! 1. `decide`; a swap onto a plan login with capacity, or a stay, stands.
//! 2. When step 1 gave `Alert` or a swap onto a login without capacity, and
//!    the live probe did not fail: plan room (`plan_room::choose`, the live
//!    login ranked with the others).
//! 3. Only when no plan login, live included, has room and the gate allows
//!    it: the first usable marked per-token profile by tier (credit tiers
//!    1 to 9, then the marked API key at 10), `OverflowIn`.
//! 4. `Alert`.
//!
//! Live on a per-token profile: a plan login with capacity, then plan room,
//! both `OverflowOut`; then, off the API key with no plan room, a usable
//! credit profile (`OverflowOut`); off a blocked profile, the next usable
//! per-token one only when no plan login has room.

use super::decide::{Decision, Measured, Probed, Reading, Reason, Win, Windows};
use super::ledger::OverflowBlock;
use super::overflow::{OverflowInput, Tag, Verdict, decide_overflow};
use super::plan_room::{choose, has_room, has_room_under, room_under};
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

// --- choose ------------------------------------------------------------------------

fn plan_of(c: &Case) -> Vec<Measured> {
    c.candidates.clone()
}

#[test]
fn choose_in_the_replay_picks_ddrumpf_over_eelsworth_and_ccapstone() {
    let c = replay();
    let live = m("iivanovich", &c.live);
    assert_eq!(
        choose(&c.s, Some(&live), &plan_of(&c)),
        Some(n("ddrumpf")),
        "eelsworth (98%) is past --session-at; ddrumpf ties ccapstone on week and wins on session"
    );
}

#[test]
fn choose_in_the_replay_at_session_at_70_still_picks_ddrumpf() {
    let c = replay().at(70);
    let live = m("iivanovich", &c.live);
    assert_eq!(choose(&c.s, Some(&live), &plan_of(&c)), Some(n("ddrumpf")));
}

#[test]
fn choose_breaks_a_full_tie_by_name() {
    let s = settings(85);
    let plan = vec![
        m("zed", &measured(10.0, 50.0, FAR)),
        m("abe", &measured(10.0, 50.0, FAR)),
    ];
    assert_eq!(choose(&s, None, &plan), Some(n("abe")));
}

/// Issue #2857 changed this test's expectation and kept it. It used to say that
/// with every login past `--session-at` the lowest week wins (the second pass).
/// The rotator no longer swaps onto a login at or above its own threshold, so
/// nothing qualifies.
#[test]
fn choose_takes_no_login_past_session_at_even_when_none_is_under_it() {
    let s = settings(85);
    let plan = vec![
        m("spare", &measured(95.0, 40.0, FAR)),
        m("extra", &measured(88.0, 60.0, FAR)),
        m("gone", &exhausted(FAR)),
    ];
    assert_eq!(
        choose(&s, None, &plan),
        None,
        "every login past --session-at: no plan-room swap"
    );
}

/// The threshold is a strict bound, as the first pass had it: a login exactly
/// at `--session-at` is at its threshold and is not a target; one point under
/// is.
#[test]
fn choose_treats_a_login_exactly_at_session_at_as_at_its_threshold() {
    let s = settings(85);
    assert_eq!(
        choose(&s, None, &[m("edge", &measured(85.0, 40.0, FAR))]),
        None
    );
    assert_eq!(
        choose(&s, None, &[m("edge", &measured(84.9, 40.0, FAR))]),
        Some(n("edge"))
    );
}

/// The 19 Nov 2026 swaps: ttanner, bbuck, ssackett and uunze were at 98-100%
/// session, so a rotator at `--session-at 85` makes none of them a target,
/// whichever of them is live.
#[test]
fn choose_makes_none_of_the_logins_at_98_to_100_percent_session_a_target() {
    let s = settings(85);
    let plan = vec![
        m("ttanner", &measured(98.0, 60.0, FAR)),
        m("bbuck", &measured(99.0, 40.0, FAR)),
        m("ssackett", &measured(99.9, 20.0, FAR)),
        m("uunze", &measured(100.0, 10.0, FAR)),
    ];
    assert_eq!(choose(&s, None, &plan), None);
    let live = m("ttanner", &measured(98.0, 60.0, FAR));
    assert_eq!(choose(&s, Some(&live), &plan[1..]), None);
}

/// `--session-at` can be set anywhere in 1..=100. At 100 the bound is the 100%
/// rule `room` already is, so a login at 99.9% is a target; at 1 only a login
/// under 1% is. An unreadable (NaN) session is never a target.
#[test]
fn choose_follows_session_at_across_its_range_and_never_takes_a_nan() {
    let plan = vec![m("spare", &measured(99.9, 40.0, FAR))];
    assert_eq!(choose(&settings(100), None, &plan), Some(n("spare")));
    assert_eq!(choose(&settings(99), None, &plan), None);
    let low = vec![m("spare", &measured(0.5, 40.0, FAR))];
    assert_eq!(choose(&settings(1), None, &low), Some(n("spare")));
    let over = vec![m("spare", &measured(1.0, 40.0, FAR))];
    assert_eq!(choose(&settings(1), None, &over), None);
    let nan = vec![m("spare", &measured(f64::NAN, 40.0, FAR))];
    for at in [1, 50, 100] {
        assert_eq!(choose(&settings(at), None, &nan), None, "session_at {at}");
    }
}

/// The week bound is unchanged: `< 100`, not `--week-at`. A login under
/// `--session-at` with its week at 99.9% is still a target.
#[test]
fn choose_keeps_the_week_bound_at_100_not_week_at() {
    let s = settings(85);
    assert_eq!(
        choose(&s, None, &[m("spare", &measured(10.0, 99.9, FAR))]),
        Some(n("spare"))
    );
    assert_eq!(
        choose(&s, None, &[m("spare", &measured(10.0, 100.0, FAR))]),
        None
    );
}

#[test]
fn choose_ignores_failed_exhausted_and_full_logins() {
    let s = settings(85);
    let plan = vec![
        m("failed", &Probed::failed()),
        m("gone", &exhausted(FAR)),
        m("full", &measured(100.0, 1.0, FAR)),
    ];
    assert_eq!(choose(&s, None, &plan), None);
}

#[test]
fn choose_returns_none_when_the_live_login_ranks_first() {
    let s = settings(85);
    let live = m("ddrumpf", &measured(0.0, 92.0, FAR));
    let plan = vec![m("ccapstone", &measured(11.0, 92.0, FAR))];
    assert_eq!(choose(&s, Some(&live), &plan), None);
}

#[test]
fn choose_counts_the_live_login_so_it_ranks_first_and_nothing_swaps() {
    let s = settings(85);
    // The live login is under --session-at and wins; spare, with a lower
    // week but past --session-at, is not a target at all (issue #2857).
    let live = m("work", &measured(10.0, 95.0, FAR));
    let plan = vec![m("spare", &measured(92.0, 20.0, FAR))];
    assert_eq!(choose(&s, Some(&live), &plan), None);
}

// --- decide_overflow: T1 and T2 ------------------------------------------------------

#[test]
fn t1_the_replay_with_no_mark_swaps_to_ddrumpf_for_plan_room() {
    let v = replay().run();
    assert_eq!(v.decision, swap("ddrumpf", Reason::PlanRoom), "{v:?}");
    assert_eq!(v.tag, None, "not an overflow move");
}

#[test]
fn t1_at_session_at_70_swaps_to_ddrumpf_too() {
    let v = replay().at(70).run();
    assert_eq!(v.decision, swap("ddrumpf", Reason::PlanRoom), "{v:?}");
}

#[test]
fn t2_the_replay_with_the_api_key_marked_and_the_gate_open_still_swaps_to_ddrumpf() {
    let v = replay().marked_key().run();
    assert_eq!(v.decision, swap("ddrumpf", Reason::PlanRoom), "{v:?}");
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

#[test]
fn the_flapping_guard_still_lets_ccapstone_move_to_ddrumpf_once() {
    let v = Case::new("ccapstone", measured(11.0, 92.0, FAR))
        .cand("ddrumpf", measured(0.0, 92.0, FAR))
        .run();
    assert_eq!(v.decision, swap("ddrumpf", Reason::PlanRoom), "{v:?}");
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

/// The flap guard still does its work where a swap is allowed: one swap onto a
/// login under `--session-at` (80, with no capacity: its week is past
/// `--week-at`), and the next cycle from there makes none.
#[test]
fn d5_a_swap_onto_a_login_under_session_at_happens_once_and_does_not_flap_back() {
    let first = Case::new("work", measured(90.0, 80.0, FAR))
        .cand("spare", measured(80.0, 95.0, FAR))
        .run();
    assert_eq!(first.decision, swap("spare", Reason::PlanRoom), "{first:?}");
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

#[test]
fn d5_margin_plays_no_part_in_plan_room() {
    // Issue #2857 moved spare under --session-at (80, was 95): a plan-room
    // target must be. Its headroom (13) is short of the live login's (10) by
    // less than --margin 5, so no threshold swap applies, yet it is the only
    // login under --session-at and plan room takes it.
    let v = Case::new("work", measured(90.0, 61.0, FAR))
        .cand("spare", measured(80.0, 87.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::PlanRoom), "{v:?}");
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

/// The issue's cycle with the live login further along. The most headroom is
/// a tie that the name would give to ccapstone; the plan-room rank is not.
#[test]
fn a_threshold_swap_without_capacity_follows_the_plan_room_rank() {
    let v = Case::new("iivanovich", measured(98.0, 61.0, FAR))
        .cand("ccapstone", measured(11.0, 92.0, FAR))
        .cand("ddrumpf", measured(0.0, 92.0, FAR))
        .cand("eelsworth", measured(98.0, 17.0, FAR))
        .run();
    assert_eq!(v.decision, swap("ddrumpf", Reason::PlanRoom), "{v:?}");
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

/// Issue #2857 changed this test's expectation and kept it: spare at 97%
/// session is past `--session-at` (85), so an exhausted live login no longer
/// moves onto it.
#[test]
fn plan_room_does_not_fire_for_an_exhausted_live_login_onto_a_login_at_97_percent() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", measured(97.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
}

/// The old intent, on a login the new rule allows: under `--session-at`, with
/// a gain over the live login short of `--margin`, so it is a plan-room move and
/// not a threshold swap.
#[test]
fn plan_room_fires_for_an_exhausted_live_login_onto_a_login_under_session_at() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", measured(50.0, 97.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::PlanRoom), "{v:?}");
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
    // room wins over them. Issue #2857: that login must be under
    // --session-at (85); its 3 points of headroom over the live login are
    // short of --margin 5, so the move is a plan-room one.
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(60.0, 97.0, FAR))
        .credits(measured(10.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::PlanRoom), "{v:?}");
    assert_eq!(v.tag, None, "not an overflow move");
}

/// Issue #2857: the same cycle with the other login at 99% session. It has
/// room but is far past `--session-at`, so there is no plan-room swap, and no
/// credit is spent because a login with room still exists: the ALERT.
#[test]
fn a_plan_login_at_99_percent_is_neither_swapped_to_nor_replaced_by_credits() {
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(99.0, 50.0, FAR))
        .credits(measured(10.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    assert_eq!(v.tag, None, "no credit spend while a plan login has room");
}

#[test]
fn d1_the_replay_with_a_tier_1_credit_profile_still_swaps_to_ddrumpf() {
    let v = replay().credits(measured(5.0, 5.0, FAR)).run();
    assert_eq!(v.decision, swap("ddrumpf", Reason::PlanRoom), "{v:?}");
    assert_eq!(v.tag, None, "no credit spend while a plan login has room");
}

#[test]
fn d1_the_replay_with_credits_and_the_api_key_marked_still_swaps_to_ddrumpf() {
    let v = replay().credits(measured(5.0, 5.0, FAR)).marked_key().run();
    assert_eq!(v.decision, swap("ddrumpf", Reason::PlanRoom), "{v:?}");
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
    // Issue #2857: the plan login must be under --session-at (85).
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(60.0, 97.0, FAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, swap("spare", Reason::PlanRoom), "{v:?}");
    assert_eq!(v.tag, None);
}

/// Issue #2857: with the other plan login at 99% session the API key is still
/// not used, because that login has room. The cycle is the ALERT.
#[test]
fn a_plan_login_at_99_percent_keeps_the_api_key_out_without_being_swapped_to() {
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(99.0, 50.0, FAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
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

/// Issue #2857 changed this expectation and kept the fixture: a plan login at
/// 95% session is past `--session-at`, so it is not a way off the API key, and
/// with no credit profile the rotator stays where it is.
#[test]
fn on_the_api_key_a_plan_login_past_session_at_is_not_a_way_off() {
    let mut c = Case::new("api-key", Probed::platform())
        .cand("work", measured(95.0, 40.0, FAR))
        .cand("spare", exhausted(FAR));
    c.overflow.push(OverflowEntry::platform(n("api-key")));
    let v = c.run();
    assert_eq!(v.decision, Decision::Stay, "{v:?}");
    assert_eq!(v.tag, Some(Tag::On));
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

/// Issue #2857 changed this expectation and kept the fixture: the plan login
/// at 95% session is past `--session-at`, so the way off the API key is the
/// credit profile, as it is when no plan login has room.
#[test]
fn on_the_api_key_a_plan_login_past_session_at_gives_way_to_a_usable_credit_profile() {
    let mut c = Case::new("api-key", Probed::platform())
        .cand("work", measured(95.0, 40.0, FAR))
        .credits(measured(10.0, 10.0, FAR));
    c.overflow.push(OverflowEntry::platform(n("api-key")));
    let v = c.run();
    assert_eq!(v.decision, swap("credits", Reason::OverflowOut), "{v:?}");
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

/// Issue #2857 changed this expectation and kept the fixture (92/40 at
/// `--session-at 85`): no capacity and no longer a move target either. The
/// rotator stays on the credit profile it is on; leaving it for a login at 92%
/// session is the swap onto a login near its limit this issue stops.
#[test]
fn on_a_credit_profile_a_plan_login_past_session_at_is_not_a_way_off() {
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", measured(92.0, 40.0, FAR))
        .cand("spare", exhausted(FAR));
    c.overflow.push(OverflowEntry::new(n("credits"), 1));
    let v = c.run();
    assert_eq!(v.decision, Decision::Stay, "{v:?}");
    assert_eq!(v.tag, Some(Tag::On));
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

/// Issue #2857's consequence for a spend-blocked credit profile, named in the
/// change's description: the plan login at 95% session is past `--session-at`,
/// so it is not a way off, and the marked API key is the next per-token
/// profile by tier. This is the fallback to "overflow" that the second pass
/// of `choose` used to hide; the key is still only ever used when the owner
/// marked it and every plan login is known.
#[test]
fn blocked_credits_with_the_only_plan_login_past_session_at_move_to_the_marked_api_key() {
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", measured(95.0, 40.0, FAR))
        .marked_key()
        .spend_blocked("credits");
    c.overflow.push(OverflowEntry::new(n("credits"), 1));
    let got = c.run();
    assert_eq!(got.decision, swap("api-key", Reason::OverflowIn), "{got:?}");
    assert_eq!(got.tag, Some(Tag::In));
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
        let v = c.cand("spare", measured(50.0, 97.0, FAR)).run();
        assert_eq!(v.decision, swap("spare", Reason::PlanRoom), "{v:?}");
    }
}

// --- Issue #2857: one comparison, shared -------------------------------------------
//
// `room_under` is what the chooser filters by and what the rotator records as
// `live_room` on its cycle, so a swap cannot land on a login the ledger then
// calls full. It adds `session < --session-at` to `room` and takes nothing
// away: `room` and `has_room` keep their meaning for auto-switch.

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

/// SEC-1: the chooser only narrows. Over a grid of readings and thresholds, a
/// lone plan login is chosen exactly when `has_room_under` says so, and
/// whatever is chosen also has the room `has_room` asks for. A second pass
/// that took any login with room would fail the first equality for every
/// login between `--session-at` and 100%.
#[test]
fn choose_picks_a_lone_login_exactly_when_it_has_room_under_and_never_otherwise() {
    let values = [0.0, 50.0, 84.9, 85.0, 90.0, 98.0, 99.9, 100.0, f64::NAN];
    for session_at in [1u8, 50, 85, 100] {
        let s = settings(session_at);
        for session in values {
            for week in values {
                let probed = measured(session, week, FAR);
                let picked = choose(&s, None, &[m("spare", &probed)]);
                assert_eq!(
                    picked.is_some(),
                    has_room_under(&probed.reading, session_at),
                    "session_at {session_at}, {session}/{week}"
                );
                if picked.is_some() {
                    assert!(has_room(&probed.reading), "{session}/{week}");
                }
            }
        }
    }
}

/// The incident's own ledger line, and the cost the design accepts: where every
/// plan login is at or above `--session-at`, even with room, there is no
/// plan-room swap, so the decision is what it is without one.
#[test]
fn a_plan_whose_logins_all_have_room_but_none_under_session_at_gives_no_swap() {
    let s = settings(85);
    let plan = vec![
        m("ttanner", &measured(98.0, 60.0, FAR)),
        m("bbuck", &measured(99.0, 40.0, FAR)),
    ];
    assert!(plan.iter().all(|p| has_room(&p.reading)), "all have room");
    assert_eq!(choose(&s, None, &plan), None);
}

#[path = "tests_issue_1804_msg.rs"]
mod tests_issue_1804_msg;
