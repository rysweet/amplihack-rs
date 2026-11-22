//! Issue #2857: what a rotate cycle records as *room* on the login that is live
//! after it, for the limit wait.
//!
//! The rotator measures the live login every cycle. It already knows, at the
//! point it saves the cycle, whether the login now in place is under its own
//! `--session-at` on the session window and under 100% on the week. It records
//! that as one optional bit, `last_cycle.live_room`, and nothing else about the
//! reading: no percentage and no text. The limit wait wakes on a login change
//! only when the bit is `true`; the rotator is the right place to decide,
//! because it is the one that chose the login.
//!
//! The rule the bit records is `plan_room::room_under`: `room(session, week)`
//! and `session < session_at`.
//!
//! Issue #2857 shared that one rule with the plan-room chooser, so the rotator
//! could not swap onto a login its own ledger then reported as full. Issue
//! #2910 split the two, in one direction only. The chooser is now the stricter
//! `overflow::plan_usable`, which also holds the week below `--week-at`, while
//! the recorded bit keeps `room_under` and its 100% week bound exactly, because
//! the usage-limit wait of issue #2911 reads what that bit means. The split is
//! safe in the direction #2857 cared about: every login the chooser accepts
//! also satisfies `room_under`, so a swap still cannot record no room. What is
//! no longer true is the converse, and `d3` below pins it: a login past
//! `--week-at` has room by the recorded rule and is still not a target.
//!
//! Which cycles write it, and which leave it out:
//!
//! - a cycle that measured the live login after its decision writes `Some(..)`:
//!   `true` for a stay or a swap onto a login with room, `false` for one that
//!   reads full;
//! - a cycle that measured nothing about the live login (a failed probe, an
//!   idle record, a `profile switch`, a guided cycle, a skipped one) writes no
//!   bit at all, and so is no room to a reader;
//! - the swap target's reading is found **by name**, never by position, so a
//!   swap from a full login to one with room records room, whichever order the
//!   readings came in.
//!
//! The swap, ledger and log are the real ones under a temporary HOME and
//! profile root (`tests_swap::env()`: `work` live, `spare` stored). Probing is
//! stubbed with `Ctx::measure`, which calls the real decision
//! (`decide_measured`), as in `tests_issue_1907_cycle`. No test reads a login,
//! starts `claude`, uses the network or a tmux server, or reads the developer's
//! profile root.
//!
//! The ledger is checked as the limit wait reads it: its bytes on disk go
//! through `amplihack_utils::rotator_status::parse_cycle`, and the result's
//! `live_room` and `fresh_room` are what the wait wakes on.
//!
//! Wired from `capacity.rs` beside `tests_issue_1907_cycle`.

use super::super::cycle::{Ctx, CycleOutcome, MeasureFn, Mode, run_cycle};
use super::super::decide::Probed;
use super::super::observe::{Measurement, Snap, decide_measured};
use super::super::tests_issue_1870::{exh, rd};
use super::super::tests_revalidate_cause::{ctx_with, log_of, probes_for};
use super::super::tests_swap::{Env, env, ino_at};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use amplihack_utils::rotator_status::{CycleStatus, parse_cycle};
use std::fs;

// --- Helpers ---------------------------------------------------------------

/// The ledger as the limit wait sees it.
fn wait_view(e: &Env) -> CycleStatus {
    let raw = fs::read(&e.rs.ledger).expect("the cycle wrote a ledger");
    parse_cycle(&raw, now() + 1).expect("the wait parses the ledger")
}

/// A limit hit just before the cycle: `fresh_room` needs a cycle measured
/// strictly later.
fn limit_hit() -> i64 {
    now() - 1
}

fn run(e: &Env, measure: MeasureFn) -> CycleOutcome {
    let mut c = ctx_with(e, measure);
    run_cycle(&mut c, Mode::Watch).unwrap()
}

// --- (a) A stay on a login with room records room -----------------------------

/// `spare` reads the same as `work`, so no upgrade applies.
fn measure_stay_with_room(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(40.0, 30.0, None))]);
    decide_measured(ctx, snap, rd(40.0, 30.0, None), probes, at)
}

#[test]
fn a_a_stay_below_the_thresholds_records_room() {
    let e = env();
    let start = limit_hit();
    assert_eq!(run(&e, measure_stay_with_room), CycleOutcome::Stayed);
    let v = wait_view(&e);
    assert_eq!(v.outcome.as_deref(), Some("stayed"));
    assert_eq!(v.live_room, Some(true), "{v:?}");
    assert!(v.fresh_room(start), "{v:?}");
}

// --- (b) A stay past the session threshold is capacity and not room -----------

/// `work` is past `--session-at` (85) but below 100% on both windows; the only
/// candidate is full, so the decision is the advisory no-capacity ALERT and the
/// cycle is recorded `stayed` (issue #1907).
fn measure_stay_past_the_threshold(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", exh(100.0, 100.0))]);
    decide_measured(ctx, snap, rd(92.0, 60.0, None), probes, at)
}

/// The bit sits beside capacity because capacity cannot say this: the hook is
/// right to resume on this login (it can still work), and the limit wait is
/// right not to wake on a change *to* a login this full (it is the login a
/// retry would hit a 429 on).
#[test]
fn b_a_stay_past_session_at_is_capacity_for_the_hook_and_not_room_for_the_wait() {
    let e = env();
    let start = limit_hit();
    run(&e, measure_stay_past_the_threshold);
    let v = wait_view(&e);
    assert_eq!(v.outcome.as_deref(), Some("stayed"), "{v:?}");
    assert!(v.has_capacity(), "{v:?}");
    assert!(
        v.fresh_capacity(start),
        "the limit-resume hook's rule is unchanged: {v:?}"
    );
    assert_eq!(v.live_room, Some(false), "{v:?}");
    assert!(!v.fresh_room(start), "{v:?}");
}

// --- (c) A swap records the login it swapped onto -------------------------------

/// `work` is past `--session-at`, `spare` is not: a threshold swap onto a login
/// with capacity. The bit is about the login now live, `spare`, and not the one
/// left, which reads full.
fn measure_swap_to_room(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(10.0, 10.0, None))]);
    decide_measured(ctx, snap, rd(90.0, 30.0, None), probes, at)
}

#[test]
fn c_a_swap_onto_a_login_with_room_records_room_from_the_target_by_name() {
    let e = env();
    let start = limit_hit();
    assert_eq!(run(&e, measure_swap_to_room), CycleOutcome::Swapped);
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare is live now");
    let v = wait_view(&e);
    assert_eq!(v.outcome.as_deref(), Some("swapped"), "{v:?}");
    assert_eq!(
        v.live_room,
        Some(true),
        "the login left read 90%; the login now live reads 10%: {v:?}"
    );
    assert!(v.fresh_room(start), "{v:?}");
}

// --- (d) The swap the issue is about: no plan-room swap onto a full login -------

/// `work` is exhausted and the only other login reads 98% session: it has room
/// by the 100% rule and is past `--session-at`. Before #2857 the rotator swapped
/// onto it (the plan-room pass 2), and the wait it woke ended in a 429.
fn measure_only_a_full_login_left(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(98.0, 50.0, None))]);
    decide_measured(ctx, snap, exh(100.0, 20.0), probes, at)
}

#[test]
fn d_the_rotator_does_not_swap_onto_a_login_at_98_percent_and_records_no_room() {
    let e = env();
    let start = limit_hit();
    let got = run(&e, measure_only_a_full_login_left);
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "work stays live: {log}");
    assert!(!log.contains("reason=plan-room"), "{log}");
    let v = wait_view(&e);
    assert!(!v.fresh_room(start), "{v:?}");
    assert_ne!(v.live_room, Some(true), "{v:?}");
}

/// The positive control for the case above: a cycle that does reach a plan-room
/// swap records room on the login it moved to.
///
/// Issue #2910 moved both readings. The fixture was `spare` at 60/97 with the
/// live login exhausted, which chose `spare` through the plan-room pass even
/// though its week was past `--week-at`; that is the swap #2910 forbids, and it
/// is kept as `d3` below. A plan-room target must now be under `--week-at` as
/// well, so `spare` reads 60/89; and a target under both bounds has at least 11
/// points of headroom, so the live login needs headroom of its own for the gain
/// to stay under `--margin` and the threshold pass to stand aside. `work`
/// therefore reads 0/92: past `--week-at`, so a swap is wanted, with 8 points of
/// headroom to spare's 11.
fn measure_a_plan_room_login_left(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(60.0, 89.0, None))]);
    decide_measured(ctx, snap, rd(0.0, 92.0, None), probes, at)
}

#[test]
fn d2_a_plan_room_swap_onto_a_login_under_session_at_records_room() {
    let e = env();
    let start = limit_hit();
    assert_eq!(
        run(&e, measure_a_plan_room_login_left),
        CycleOutcome::Swapped
    );
    let log = log_of(&e);
    assert!(log.contains("reason=plan-room"), "{log}");
    let v = wait_view(&e);
    assert_eq!(v.live_room, Some(true), "{v:?}");
    assert!(v.fresh_room(start), "{v:?}");
}

/// Issue #2910, rule 2, on the fixture `d2` used to carry: `work` is exhausted
/// and the only other login reads 60% session and 97% week. It is under
/// `--session-at`, and `room_under` (what `live_room` records) counts it as
/// having room, so before #2910 the plan-room pass swapped onto it. On 21
/// November 2026 at 07:20:31Z that same rule put candide on a login at 98% of
/// its week, and the next request met the organisation's spend limit.
///
/// The week bound now applies in the plan-room pass too, so there is no swap,
/// and nothing is recorded as room because the live login was never read with
/// room of its own.
fn measure_a_week_spent_login_left(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(60.0, 97.0, None))]);
    decide_measured(ctx, snap, exh(100.0, 20.0), probes, at)
}

#[test]
fn d3_the_rotator_does_not_swap_onto_a_login_past_week_at_and_records_no_room() {
    let e = env();
    let start = limit_hit();
    let got = run(&e, measure_a_week_spent_login_left);
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "work stays live: {log}");
    assert!(!log.contains("reason=plan-room"), "{log}");
    let v = wait_view(&e);
    assert!(!v.fresh_room(start), "{v:?}");
    assert_ne!(v.live_room, Some(true), "{v:?}");
}

// --- (e) A cycle that did not read the live login records no room -------------

fn measure_failed_live(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(10.0, 10.0, None))]);
    decide_measured(ctx, snap, Probed::failed(), probes, at)
}

#[test]
fn e_a_stay_on_a_failed_probe_records_no_room() {
    let e = env();
    let start = limit_hit();
    run(&e, measure_failed_live);
    let v = wait_view(&e);
    assert_eq!(v.outcome.as_deref(), Some("stayed-unread"), "{v:?}");
    assert_ne!(v.live_room, Some(true), "{v:?}");
    assert!(!v.fresh_room(start), "{v:?}");
}

fn measure_exhausted_everywhere(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", exh(100.0, 100.0))]);
    decide_measured(ctx, snap, exh(100.0, 100.0), probes, at)
}

#[test]
fn e2_an_alerted_cycle_records_no_room() {
    let e = env();
    let start = limit_hit();
    assert_eq!(run(&e, measure_exhausted_everywhere), CycleOutcome::Alerted);
    let v = wait_view(&e);
    assert_ne!(v.live_room, Some(true), "{v:?}");
    assert!(!v.fresh_room(start), "{v:?}");
}

// --- (f) A later cycle replaces the bit, it does not accumulate --------------------

#[test]
fn f_the_bit_follows_the_latest_cycle() {
    let e = env();
    run(&e, measure_stay_past_the_threshold);
    assert_eq!(wait_view(&e).live_room, Some(false));
    run(&e, measure_stay_with_room);
    assert_eq!(
        wait_view(&e).live_room,
        Some(true),
        "a login that recovered wakes a wait again"
    );
    run(&e, measure_exhausted_everywhere);
    assert_ne!(wait_view(&e).live_room, Some(true));
}

// --- (g) The ledger holds one bit and nothing it implies --------------------------

#[test]
fn g_the_ledger_holds_only_the_bit_no_percentage_and_no_threshold() {
    let e = env();
    run(&e, measure_swap_to_room);
    let raw = fs::read_to_string(&e.rs.ledger).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let last = json["last_cycle"].as_object().expect("last_cycle");
    assert_eq!(last["live_room"], serde_json::Value::Bool(true));
    for key in last.keys() {
        assert!(
            ["at", "measured_at", "outcome", "overflow", "live_room"].contains(&key.as_str()),
            "unexpected last_cycle key {key:?}: {raw}"
        );
    }
    assert!(
        json.get("live_room").is_none(),
        "the bit lives in last_cycle, not at the top level: {raw}"
    );
}

// --- (h) The swap target's reading is found by exact name (SEC-9) ----------------------

/// `live_reading` is what the bit is computed from. It takes the target from
/// the rotator's own decision and looks it up by name in the readings it took:
/// never a prefix, never a position, never anything from the environment.
#[test]
fn h_live_reading_finds_the_swap_target_by_exact_name_in_any_order() {
    use super::super::decide::Reading;
    use super::super::overflow_record::live_reading;
    use crate::commands::profile::name::ProfileName;
    let name = |s: &str| ProfileName::parse(s).unwrap();
    let live = Reading::Measured {
        session: 99.0,
        week: 10.0,
    };
    let full = Reading::Measured {
        session: 98.0,
        week: 10.0,
    };
    let roomy = Reading::Measured {
        session: 10.0,
        week: 10.0,
    };
    let readings = [
        (name("work2"), roomy),
        (name("work"), full),
        (name("spare"), roomy),
    ];
    assert_eq!(
        live_reading(Some(&name("work")), &live, &readings),
        Some(&full)
    );
    assert_eq!(
        live_reading(Some(&name("work2")), &live, &readings),
        Some(&roomy)
    );
    // The same readings in the other order give the same answers.
    let mut reversed = readings;
    reversed.reverse();
    assert_eq!(
        live_reading(Some(&name("work")), &live, &reversed),
        Some(&full)
    );
    assert_eq!(
        live_reading(Some(&name("work2")), &live, &reversed),
        Some(&roomy)
    );
}

/// A cycle that swapped to a login none of its readings names measured nothing
/// about the login now in place, so there is no reading and no bit. It does not
/// fall back to the login it left, whose reading is a reading of another login.
#[test]
fn h_a_swap_to_an_unmeasured_login_has_no_reading_and_a_stay_has_the_live_one() {
    use super::super::decide::Reading;
    use super::super::overflow_record::live_reading;
    use crate::commands::profile::name::ProfileName;
    let name = |s: &str| ProfileName::parse(s).unwrap();
    let live = Reading::Measured {
        session: 10.0,
        week: 10.0,
    };
    let readings = [(
        name("workshop"),
        Reading::Measured {
            session: 50.0,
            week: 50.0,
        },
    )];
    assert_eq!(
        live_reading(Some(&name("work")), &live, &readings),
        None,
        "a name that is a prefix of another is not that login"
    );
    assert_eq!(
        live_reading(Some(&name("absent")), &live, &readings),
        None,
        "a target nobody measured"
    );
    assert_eq!(live_reading(None, &live, &readings), Some(&live), "a stay");
}
