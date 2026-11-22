//! Issue #1804: the plan-room swap and explicit consent for the API-key
//! profile, in the rotator's pure decision. No I/O. See "Rotate plan-room
//! swap" and "Rotate decision order" in docs/reference/profile-command.md.
//!
//! Contract (`rotate::plan_room`):
//! - `has_room(&Reading) -> bool`: a `Measured` reading with session and
//!   week both below 100. `Failed` and `Exhausted` never have room.
//! - `choose(&Settings, live: Option<&Measured>, plan: &[Measured]) ->
//!   Option<ProfileName>`: one pass over the live login and the plan logins,
//!   taking those `overflow::plan_usable` accepts. Issue #2857:
//!   the rotator does not plan-room swap onto a login already at or above its
//!   session threshold, so the second pass that took any login with room
//!   (session < 100) is gone. Issue #2910: nor onto one at or above
//!   `--week-at`, where this file used to bound the week at 100. Ranked by
//!   week, then session, then name. `None` when nothing qualifies or when the
//!   winner is the live login (the flapping guard). `room()` and `has_room`
//!   keep the 100% rule: auto-switch calls them, and the `no_room` ledger
//!   record is read by it.
//! - `Reason::PlanRoom`, rendered `plan-room`.
//!
//! Issue #2910 moved much of this file's evidence. Its replay logins sit at
//! 92% to 97% of their week, which the plan-room pass used to accept and now
//! does not: `--week-at` defaults to 90. The replay therefore runs at
//! `--week-at 95`, the figure #2910 itself names, so it keeps pinning
//! #1804's ranking rule at the cycle's own readings, and
//! [`at_the_default_week_at_the_1804_replay_has_no_target_at_all`] pins what
//! the default now does to the same cycle. Where a login's week was only a
//! device to reach the plan-room pass (a login with session room but no
//! capacity), the device has moved under the bound instead, because #2910
//! made `plan_usable` the admission rule for both passes.
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

use super::decide::{Decision, Measured, Probed, Reading, Reason, Win, Windows, needs_swap};
use super::ledger::OverflowBlock;
use super::overflow::{OverflowInput, Tag, Verdict, decide_overflow, plan_usable};
use super::plan_room::{choose, has_room, has_room_under, room_under};
use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::OverflowEntry;
use std::collections::BTreeMap;

const NOW: i64 = 1_793_389_376; // 2026-10-30T19:42:56Z
const HOUR: i64 = 3600;
const FAR: Option<i64> = Some(NOW + 4 * HOUR);
const NEAR: Option<i64> = Some(NOW + 600);

/// The `--week-at` the #1804 replay runs at. Its logins sit at 92% of their
/// week, which issue #2910 keeps out of every pass at the default of 90; 95
/// is the figure #2910's own text names, and at it the replay still pins
/// what #1804 settled.
const REPLAY_WEEK_AT: u8 = 95;

fn settings(session_at: u8) -> Settings {
    settings_at(session_at, 90)
}

fn settings_at(session_at: u8, week_at: u8) -> Settings {
    Settings {
        session_at,
        week_at,
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

    /// Sets `--session-at`, leaving `--week-at` as it is: issue #2910 made
    /// the week a bound on every pass, so a case that has chosen one must
    /// keep it when its session threshold moves.
    fn at(mut self, session_at: u8) -> Self {
        self.s.session_at = session_at;
        self
    }

    fn week_at(mut self, week_at: u8) -> Self {
        self.s.week_at = week_at;
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
///
/// Run at [`REPLAY_WEEK_AT`], not at the default 90. Five of its seven
/// candidates sit at 92% to 97% of their week, which issue #2910 keeps out
/// of every pass; at 95 the cycle still has ccapstone and ddrumpf to choose
/// between, which is what #1804 was about. See
/// [`at_the_default_week_at_the_1804_replay_has_no_target_at_all`].
fn replay() -> Case {
    Case::new("iivanovich", measured(87.0, 61.0, FAR))
        .week_at(REPLAY_WEEK_AT)
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

/// The 19 Nov 2026 swaps: four logins in a row were at 98-100% session, so a
/// rotator at `--session-at 85` makes none of them a target, whichever of
/// them is live.
#[test]
fn choose_makes_none_of_the_logins_at_98_to_100_percent_session_a_target() {
    let s = settings(85);
    let plan = vec![
        m("aanderson", &measured(98.0, 60.0, FAR)),
        m("bbuck", &measured(99.0, 40.0, FAR)),
        m("ccapstone", &measured(99.9, 20.0, FAR)),
        m("ddrumpf", &measured(100.0, 10.0, FAR)),
    ];
    assert_eq!(choose(&s, None, &plan), None);
    let live = m("aanderson", &measured(98.0, 60.0, FAR));
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

/// Issue #2910 reverses #1804's week bound of 100. On 21 November 2026 at
/// 07:20:31Z the rotator moved candide onto a login at 98% of its week
/// (`plan-room`), and its next request met the spend limit. The bound is
/// now `--week-at` (90 by default), strictly, as the swap rules' is: a
/// login with its week at 99.9% is never a target.
#[test]
fn choose_bounds_the_week_at_week_at_not_100() {
    let s = settings(85);
    assert_eq!(
        choose(&s, None, &[m("spare", &measured(10.0, 89.9, FAR))]),
        Some(n("spare"))
    );
    for week in [90.0, 99.9, 100.0] {
        assert_eq!(
            choose(&s, None, &[m("spare", &measured(10.0, week, FAR))]),
            None,
            "week {week}"
        );
    }
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

/// The flapping guard, at the replay's own readings. Run at
/// [`REPLAY_WEEK_AT`]: at the default 90 neither login is a target, so the
/// `None` would say nothing about the guard. Issue #2910 left the guard
/// where it is but put it out of a cycle's reach, which
/// [`a_login_needs_a_swap_exactly_when_it_is_not_a_target`] records; this
/// is now the only test that exercises it.
#[test]
fn choose_returns_none_when_the_live_login_ranks_first() {
    let s = settings_at(85, REPLAY_WEEK_AT);
    let live = m("ddrumpf", &measured(0.0, 92.0, FAR));
    let plan = vec![m("ccapstone", &measured(11.0, 92.0, FAR))];
    assert_eq!(choose(&s, Some(&live), &plan), None);
}

#[test]
fn choose_counts_the_live_login_so_it_ranks_first_and_nothing_swaps() {
    let s = settings(85);
    // The live login is under --session-at and wins; spare, with a lower
    // week but past --session-at, is not a target at all (issue #2857).
    // Issue #2910 moved the live login's week from 95 under --week-at 90,
    // because a login over that bound is no longer in the pool to rank.
    let live = m("work", &measured(10.0, 85.0, FAR));
    let plan = vec![m("spare", &measured(92.0, 20.0, FAR))];
    assert_eq!(choose(&s, Some(&live), &plan), None);
}

// --- decide_overflow: T1 and T2 ------------------------------------------------------

/// What issue #2910 costs issue #1804, stated rather than left implicit.
///
/// `--week-at` defaults to 90. Five of the replay's seven candidates are
/// between 92% and 97% of their week, the sixth is at 98% of its session
/// and the seventh is exhausted, so at the default no login in that cycle
/// is one the rotator may move onto and it alerts. #1804's remedy for that
/// cycle, a plan-room swap to ddrumpf on its fresh session window, needs
/// `--week-at 95` or higher to fire at all.
///
/// Nothing here argues the default is wrong. The alert ends nothing: the
/// rotator waits for the next limit event or the earliest recorded reset
/// and tries again. But an operator who wants #1804's behaviour at 92% of
/// a week has to say so with `--week-at`, and that is a change this run
/// makes and does not hide.
#[test]
fn at_the_default_week_at_the_1804_replay_has_no_target_at_all() {
    let v = replay().week_at(90).run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
    let v = replay().run();
    assert_eq!(
        v.decision,
        swap("ddrumpf", Reason::PlanRoom),
        "at --week-at {REPLAY_WEEK_AT}: {v:?}"
    );
}

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

/// The replay's two closest logins, with ddrumpf live. Issue #2910 made
/// this cycle say the same thing for a different reason at each bound, and
/// both are pinned, because the old single assertion would now pass at the
/// default without the flapping guard playing any part.
///
/// At [`REPLAY_WEEK_AT`] ddrumpf's week of 92 is under the bound, so it
/// does not need a swap at all and the cycle stays. At the default 90 it is
/// over the bound, so it does need one, and ccapstone is over the same
/// bound and cannot take it: the cycle alerts, and the rotator waits for a
/// reset rather than moving work onto a login with no more room than the
/// one it is leaving.
#[test]
fn the_flapping_guard_keeps_ddrumpf_live() {
    let case = || {
        Case::new("ddrumpf", measured(0.0, 92.0, FAR)).cand("ccapstone", measured(11.0, 92.0, FAR))
    };
    let v = case().week_at(REPLAY_WEEK_AT).run();
    assert_eq!(v.decision, Decision::Stay, "{v:?}");
    let v = case().run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
}

#[test]
fn the_flapping_guard_still_lets_ccapstone_move_to_ddrumpf_once() {
    // Issue #2910: a cycle only reaches the plan-room pass when its live
    // login needed a swap, which now means the live login is over a bound
    // and so is never in the pool. ccapstone's week moved from 92 to 96, so
    // it is over --week-at 95 and asks for a move, while ddrumpf at 92 is
    // under it and can take one. The guard itself is pinned by
    // `choose_returns_none_when_the_live_login_ranks_first`.
    let v = Case::new("ccapstone", measured(11.0, 96.0, FAR))
        .week_at(REPLAY_WEEK_AT)
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
    // Issue #2910 moved spare's week from 95 to 89, under --week-at 90, so
    // that it is still a target. Its headroom of 11 is one short of work's
    // plus --margin, which is what keeps the first move a plan-room one.
    let first = Case::new("work", measured(90.0, 80.0, FAR))
        .cand("spare", measured(80.0, 89.0, FAR))
        .run();
    assert_eq!(first.decision, swap("spare", Reason::PlanRoom), "{first:?}");
    // The cycle after it used to reach the flapping guard. Under #2910 it
    // does not get that far: spare is under both thresholds, so it needs no
    // swap and the cycle stays. Nothing flaps back either way, and work, at
    // 90% of its session, is no more a target now than it was then.
    let second = Case::new("spare", measured(80.0, 89.0, FAR))
        .cand("work", measured(90.0, 80.0, FAR))
        .run();
    assert_eq!(second.decision, Decision::Stay, "no flap back: {second:?}");
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
/// a tie that the name gives to ccapstone, and since issue #2910 the
/// threshold swap that names it stands.
#[test]
fn a_threshold_swap_now_names_only_a_login_with_capacity() {
    // Issue #2910 made `plan_usable` the filter inside `decide::best`, so a
    // threshold swap can no longer name a login without capacity. The arm
    // in `overflow::decide_restricted` that handed such a swap to the
    // plan-room rank could not be taken, and was removed; only an ALERT now
    // reaches the plan-room pass. What is pinned here is that a threshold
    // swap's target always has capacity.
    //
    // This case used to assert that the threshold target ccapstone, at 92%
    // of its week and so without capacity, gave way to the plan-room rank
    // and ddrumpf. At --week-at 95 ccapstone has capacity and the threshold
    // swap stands; at the default 90 neither login has capacity, nothing is
    // a target, and the cycle alerts rather than moving onto either.
    let case = || {
        Case::new("iivanovich", measured(98.0, 61.0, FAR))
            .cand("ccapstone", measured(11.0, 92.0, FAR))
            .cand("ddrumpf", measured(0.0, 92.0, FAR))
            .cand("eelsworth", measured(98.0, 17.0, FAR))
    };
    let v = case().week_at(REPLAY_WEEK_AT).run();
    assert_eq!(v.decision, swap("ccapstone", Reason::Threshold), "{v:?}");
    let v = case().run();
    assert_eq!(v.decision, Decision::Alert, "{v:?}");
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

/// Issue #2910 changed the reason this cycle records, not where it goes. It
/// used to be the old #1804 intent on a login the #2857 rule allowed: under
/// `--session-at`, with a gain over the live login short of `--margin`, so
/// a plan-room move rather than a threshold swap. That gain can no longer
/// be short.
/// An exhausted live login has no headroom, and every login the rotator may
/// now move onto has at least eleven points of it, so the threshold pass
/// clears `--margin` and names spare before the plan-room pass is reached.
/// Spare's week moved from 97 to 80 because at 97 it is not a target at
/// all.
#[test]
fn an_exhausted_live_login_swaps_onto_a_login_under_both_thresholds() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", measured(50.0, 80.0, FAR))
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
fn a_plan_login_with_room_comes_before_a_usable_credit_profile() {
    // D1: credit profiles are billed per token too, so a plan login with
    // room wins over them. Issue #2857: that login must be under
    // --session-at (85); issue #2910: and under --week-at (90), so spare's
    // week moved from 97 to 85.
    //
    // The reason moved with it. The live login is spent, so it has no
    // headroom, and every login the rotator may now move onto has at least
    // eleven points of it; the gain therefore clears --margin and the
    // threshold pass names spare before the plan-room pass is reached. What
    // this test is about is unchanged: the rotator goes to the plan login
    // and spends nothing, which `tag: None` is the record of.
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(60.0, 85.0, FAR))
        .credits(measured(10.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold), "{v:?}");
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
fn a_plan_login_with_room_comes_before_the_api_key() {
    // Issue #2857: the plan login must be under --session-at (85). Issue
    // #2910: and under --week-at (90), so its week moved from 97 to 85, and
    // the reason from plan-room to threshold for the reason given in
    // `a_plan_login_with_room_comes_before_a_usable_credit_profile`. The
    // API key is still never chosen while a plan login has room.
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(60.0, 85.0, FAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold), "{v:?}");
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
    // Issue #2857: "room" for a move is now under --session-at (85). Issue
    // #2910 made --week-at a bound on every pass, so work's week moved from
    // 95 to 85: a login short of capacity is no longer a way off the key.
    let mut c = Case::new("api-key", Probed::platform())
        .cand("work", measured(60.0, 85.0, FAR))
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
    // Issue #2910 moved work's week from 95 under --week-at 90.
    let mut c = Case::new("api-key", Probed::platform())
        .cand("work", measured(60.0, 85.0, FAR))
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
    // 60/85 at --session-at 85 and --week-at 90: room under both. Credits
    // cost money, so the rotator returns to the plan login. Issue #2857:
    // the fixture was 92/40, which is past --session-at; see the next test.
    // Issue #2910 then moved its week from 95 to 85, because a login past
    // --week-at is not a target in any pass.
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", measured(60.0, 85.0, FAR))
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
    // Issue #2857: the plan login must be under --session-at (85). Issue
    // #2910: and under --week-at (90), so work's week moved from 95 to 85.
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", measured(60.0, 85.0, FAR))
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
fn neither_hold_stops_the_swap_to_a_plan_login_which_costs_nothing() {
    for c in [
        Case::new("work", exhausted(FAR)).cand("timedout", Probed::failed()),
        Case::new("work", exhausted(FAR)).refused("locked"),
    ] {
        // Issue #2910: spare's week moved from 97 to 80, and the reason
        // with it. An exhausted live login has no headroom, so any login
        // the rotator may now move onto clears --margin and the threshold
        // pass names it first. Neither hold stops the move, which is the
        // point; it still costs nothing, because spare is a plan login.
        let v = c.cand("spare", measured(50.0, 80.0, FAR)).run();
        assert_eq!(v.decision, swap("spare", Reason::Threshold), "{v:?}");
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

/// SEC-1: the chooser only narrows. Over a grid of readings and both
/// thresholds, a lone plan login is chosen exactly when `plan_usable` says
/// the rotator may move onto it, and whatever is chosen also has the room
/// `has_room` asks for and the session room `has_room_under` asks for.
///
/// Issue #2910 changed the first equality's right-hand side from
/// `has_room_under`, which bounded the week at 100. On 21 November 2026 at
/// 07:20:31Z that bound put candide on a login at 98% of its week, and the
/// next request met the organisation's spend limit. The two weaker rules
/// are still asserted, as implications rather than equalities, so the
/// chooser cannot be loosened past either of them without this failing.
#[test]
fn choose_picks_a_lone_login_exactly_when_plan_usable_and_never_otherwise() {
    let values = [
        0.0,
        50.0,
        84.9,
        85.0,
        89.9,
        90.0,
        98.0,
        99.9,
        100.0,
        f64::NAN,
    ];
    for session_at in [1u8, 50, 85, 100] {
        for week_at in [1u8, 50, 90, 95, 100] {
            let s = settings_at(session_at, week_at);
            for session in values {
                for week in values {
                    let probed = measured(session, week, FAR);
                    let picked = choose(&s, None, &[m("spare", &probed)]);
                    assert_eq!(
                        picked.is_some(),
                        plan_usable(&s, &probed.reading),
                        "session_at {session_at}, week_at {week_at}, {session}/{week}"
                    );
                    if picked.is_some() {
                        assert!(has_room(&probed.reading), "{session}/{week}");
                        assert!(
                            has_room_under(&probed.reading, session_at),
                            "{session}/{week}"
                        );
                    }
                }
            }
        }
    }
}

/// Issue #2910 gave `needs_swap` and the target test the same two bounds,
/// so a measured login needs a swap exactly when the rotator may not move
/// onto it. Two consequences follow, and both are stated here rather than
/// left to be rediscovered.
///
/// The first is the point of #2910: the rotator cannot answer a login that
/// is over a threshold by moving to another login that is over the same
/// one. On 21 November 2026 at 18:00Z, on calypso, it did, and nnorth was
/// spent within minutes.
///
/// The second is a cost. [`plan_room::choose`] is only ever called for a
/// cycle whose live login needed a swap, so that login now fails
/// `plan_usable` and never reaches the ranking. The flapping guard inside
/// `choose` is therefore unreachable from a cycle; it is kept, and pinned
/// by [`choose_returns_none_when_the_live_login_ranks_first`], because the
/// rule it states is still the right one.
///
/// An unreadable (NaN) window is the one reading that is neither: it fails
/// every comparison, so it needs no swap and is no target.
#[test]
fn a_login_needs_a_swap_exactly_when_it_is_not_a_target() {
    let s = settings(85);
    for session in [0.0, 50.0, 84.9, 85.0, 89.9, 90.0, 99.9, 100.0] {
        for week in [0.0, 50.0, 84.9, 85.0, 89.9, 90.0, 99.9, 100.0] {
            let r = Reading::Measured { session, week };
            assert_ne!(
                needs_swap(&s, &r),
                plan_usable(&s, &r),
                "{session}/{week} is both or neither"
            );
        }
    }
    for r in [Reading::Failed, Reading::EXHAUSTED] {
        assert!(needs_swap(&s, &r), "{r:?}");
        assert!(!plan_usable(&s, &r), "{r:?}");
    }
    let nan = Reading::Measured {
        session: f64::NAN,
        week: 10.0,
    };
    assert!(
        !needs_swap(&s, &nan),
        "an unreadable window asks for nothing"
    );
    assert!(!plan_usable(&s, &nan), "and is never a target");
}

/// The incident's own ledger line, and the cost the design accepts: where every
/// plan login is at or above `--session-at`, even with room, there is no
/// plan-room swap, so the decision is what it is without one.
#[test]
fn a_plan_whose_logins_all_have_room_but_none_under_session_at_gives_no_swap() {
    let s = settings(85);
    let plan = vec![
        m("aanderson", &measured(98.0, 60.0, FAR)),
        m("bbuck", &measured(99.0, 40.0, FAR)),
    ];
    assert!(plan.iter().all(|p| has_room(&p.reading)), "all have room");
    assert_eq!(choose(&s, None, &plan), None);
}

#[path = "tests_issue_1804_msg.rs"]
mod tests_issue_1804_msg;
