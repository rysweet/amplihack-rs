//! Issue #1804: the plan-room swap and explicit consent for the API-key
//! profile, in the rotator's pure decision. No I/O. See "Rotate plan-room
//! swap" and "Rotate decision order" in docs/reference/profile-command.md.
//!
//! Contract (`rotate::plan_room`):
//! - `has_room(&Reading) -> bool`: a `Measured` reading with session and
//!   week both below 100. `Failed` and `Exhausted` never have room.
//! - `choose(&Settings, live: Option<&Measured>, plan: &[Measured]) ->
//!   Option<ProfileName>`: pass 1 takes logins with session < `session_at`
//!   and week < 100; pass 2, only when pass 1 (the live login included) is
//!   empty, takes session < 100 and week < 100. Ranked by week, then
//!   session, then name. `None` when nothing qualifies or when the winner
//!   is the live login (the flapping guard).
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
use super::plan_room::{choose, has_room};
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
        },
        spend_limit: false,
        unknown: false,
    }
}

fn exhausted(reset: Option<i64>) -> Probed {
    Probed {
        reading: Reading::Exhausted,
        windows: Windows {
            session: Some(Win {
                used: 100.0,
                resets_at: reset,
            }),
            week: Some(Win {
                used: 100.0,
                resets_at: reset,
            }),
        },
        spend_limit: false,
        unknown: false,
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
    assert!(!has_room(&Reading::Exhausted));
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

#[test]
fn choose_uses_pass_2_only_when_pass_1_is_empty() {
    let s = settings(85);
    let plan = vec![
        m("spare", &measured(95.0, 40.0, FAR)),
        m("extra", &measured(88.0, 60.0, FAR)),
        m("gone", &exhausted(FAR)),
    ];
    assert_eq!(
        choose(&s, None, &plan),
        Some(n("spare")),
        "every login past --session-at: the lowest week wins"
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
fn choose_counts_the_live_login_in_pass_1_so_pass_2_is_not_used() {
    let s = settings(85);
    // The live login qualifies for pass 1 and wins it; spare, with a lower
    // week but past --session-at, is only a pass-2 login.
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

#[test]
fn pass_2_picks_the_lowest_week_when_every_login_is_past_session_at() {
    let v = Case::new("work", measured(90.0, 50.0, FAR))
        .cand("spare", measured(95.0, 40.0, FAR))
        .cand("extra", measured(88.0, 60.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::PlanRoom), "{v:?}");
}

#[test]
fn d5_pass_2_swaps_once_and_the_next_cycle_makes_no_further_swap() {
    // No login has session room (--session-at 85), so pass 2 ranks by week.
    let first = Case::new("work", measured(90.0, 80.0, FAR))
        .cand("spare", measured(95.0, 60.0, FAR))
        .run();
    assert_eq!(first.decision, swap("spare", Reason::PlanRoom), "{first:?}");
    // The next cycle, over the same readings, from the new live login.
    let second = Case::new("spare", measured(95.0, 60.0, FAR))
        .cand("work", measured(90.0, 80.0, FAR))
        .run();
    assert_eq!(second.decision, Decision::Alert, "no flap back: {second:?}");
    assert_eq!(second.tag, None);
}

#[test]
fn d5_an_exhausted_live_login_ranks_last_in_pass_2() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", measured(99.0, 99.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::PlanRoom), "{v:?}");
}

#[test]
fn d5_margin_plays_no_part_in_plan_room() {
    // spare beats the live login by one point of week only: far short of
    // --margin 5, but strictly better on the ranking.
    let v = Case::new("work", measured(90.0, 61.0, FAR))
        .cand("spare", measured(95.0, 60.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::PlanRoom), "{v:?}");
}

#[test]
fn pass_2_keeps_a_live_login_that_has_the_lowest_week() {
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

#[test]
fn plan_room_fires_for_an_exhausted_live_login() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", measured(97.0, 10.0, FAR))
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
    // room wins over them.
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(99.0, 50.0, FAR))
        .credits(measured(10.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::PlanRoom), "{v:?}");
    assert_eq!(v.tag, None, "not an overflow move");
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
    let v = Case::new("work", measured(100.0, 40.0, FAR))
        .cand("spare", measured(99.0, 50.0, FAR))
        .marked_key()
        .run();
    assert_eq!(v.decision, swap("spare", Reason::PlanRoom), "{v:?}");
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
    // 92/40 at --session-at 85: no capacity, but room. Credits cost money,
    // so the rotator returns to the plan login.
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
    let mut c = Case::new("credits", measured(10.0, 10.0, FAR))
        .cand("work", measured(95.0, 40.0, FAR))
        .marked_key()
        .spend_blocked("credits");
    c.overflow.push(OverflowEntry::new(n("credits"), 1));
    let got = c.run();
    assert_ne!(got.decision, swap("api-key", Reason::OverflowIn), "{got:?}");
    assert_eq!(got.decision, swap("work", Reason::OverflowOut), "{got:?}");
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

#[path = "tests_issue_1804_msg.rs"]
mod tests_issue_1804_msg;
