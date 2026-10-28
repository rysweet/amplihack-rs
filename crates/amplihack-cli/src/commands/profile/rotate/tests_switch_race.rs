//! Issue #1691: a `profile switch` made while a rotate cycle measures without
//! the lock. Step 6 sees the changed snapshot and skips the swap; it must not
//! also replace the switch's ledger record with `skipped`, or the held cycles
//! after it record a bare `held` and the limit-resume hook never wakes the
//! sessions stopped before the switch.
//!
//! In process, on the fixture in `tests_swap.rs` (`work` live, `spare`
//! stored). `Ctx::measure` replaces finding `claude`, measuring and deciding,
//! so no binary is stubbed and nothing outside the temp dir is touched.

use super::cycle::{Ctx, CycleOutcome, MeasureFn, Mode, run_cycle};
use super::decide::{Decision, Reading, Reason};
use super::ledger::{LastCycle, SWITCHED};
use super::log::msg;
use super::observe::Measurement;
use super::pin::{self, PinState};
use super::settings;
use super::switch::{Switched, switch_live};
use super::tests_swap::{Env, env, ino_at, json, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use amplihack_utils::rotator_status::parse_cycle;
use std::fs;

fn watch_ctx(e: &Env, measure: MeasureFn) -> Ctx {
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut ctx = Ctx::new(e.store.clone(), e.home.clone(), s);
    ctx.measure = Some(measure);
    ctx
}

/// What a real measurement would decide: `work` is spent, swap to `spare`.
fn decided(measured_at: i64) -> Measurement {
    Measurement {
        live: Reading::Exhausted,
        others: vec![(
            n("spare"),
            Reading::Measured {
                session: 0.1,
                week: 0.1,
            },
        )],
        decision: Decision::Swap {
            to: n("spare"),
            reason: Reason::Threshold,
        },
        note: None,
        failed: 0,
        tag: None,
        new_blocks: Vec::new(),
        next_plan_reset: None,
        measured_at,
    }
}

/// While the cycle measures: the person runs `profile switch spare`.
fn a_switch(ctx: &Ctx) {
    match switch_live(&ctx.store, &ctx.home, &n("spare")) {
        Ok(Ok(Switched::Moved { .. })) => {}
        other => panic!("switch during measurement failed: {other:?}"),
    }
}

/// While the cycle measures: the pin changes by hand, and nothing else.
fn a_hand_pin(ctx: &Ctx) {
    pin::write(&ctx.rs, &n("work")).unwrap();
}

/// While the cycle measures: the pin changes by hand and another writer
/// records a cycle.
fn a_hand_pin_and_a_record(ctx: &Ctx) {
    pin::write(&ctx.rs, &n("work")).unwrap();
    let mut l = ctx.rs.load_ledger().unwrap().unwrap_or_default();
    l.last_cycle = Some(LastCycle {
        at: now(),
        measured_at: Some(now()),
        outcome: "stayed".into(),
        overflow: None,
    });
    ctx.rs.save_ledger(&l).unwrap();
}

fn measure_with(during: fn(&Ctx), ctx: &Ctx) -> Result<Measurement, ProfileError> {
    let measured_at = now();
    during(ctx);
    Ok(decided(measured_at))
}

#[cfg(target_os = "linux")]
#[test]
fn a_switch_during_measurement_keeps_its_record_through_later_held_cycles() {
    let e = env();
    let mut ctx = watch_ctx(&e, |c, _| measure_with(a_switch, c));
    assert_eq!(
        run_cycle(&mut ctx, Mode::Watch).unwrap(),
        CycleOutcome::Skipped,
        "the switch changed the snapshot, so step 6 does not swap"
    );
    let log = fs::read_to_string(&e.rs.log).unwrap_or_default();
    assert!(
        log.contains(msg::CHANGED),
        "the skip is still logged: {log}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare is live, as switched");
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("spare")));

    let switched = json(&e.rs.ledger)["last_cycle"].clone();
    assert_eq!(switched["outcome"], SWITCHED, "{switched}");
    let switch_at = switched["measured_at"].as_i64().unwrap();

    for i in 0..2 {
        assert_eq!(
            run_cycle(&mut ctx, Mode::Watch).unwrap(),
            CycleOutcome::Held,
            "cycle {i} after the switch is held"
        );
        assert_eq!(
            json(&e.rs.ledger)["last_cycle"],
            switched,
            "held cycle {i} keeps the switch record"
        );
        let raw = fs::read(&e.rs.ledger).unwrap();
        let status = parse_cycle(&raw, now()).expect("the hook reads the ledger");
        assert!(status.has_capacity(), "held cycle {i}");
        assert!(
            status.fresh_capacity(switch_at - 1),
            "a session stopped before the switch is woken (held cycle {i})"
        );
    }
}

#[test]
fn a_skip_still_records_itself_when_nothing_newer_was_written() {
    let e = env();
    let before = now();
    let mut ctx = watch_ctx(&e, |c, _| measure_with(a_hand_pin, c));
    assert_eq!(
        run_cycle(&mut ctx, Mode::Watch).unwrap(),
        CycleOutcome::Skipped
    );
    let last = &json(&e.rs.ledger)["last_cycle"];
    assert_eq!(last["outcome"], "skipped", "{last}");
    assert!(last["measured_at"].as_i64().is_some_and(|t| t >= before));
    e.assert_unchanged();
}

#[test]
fn a_skip_never_replaces_a_record_written_while_it_measured() {
    let e = env();
    let mut ctx = watch_ctx(&e, |c, _| measure_with(a_hand_pin_and_a_record, c));
    assert_eq!(
        run_cycle(&mut ctx, Mode::Watch).unwrap(),
        CycleOutcome::Skipped
    );
    let last = &json(&e.rs.ledger)["last_cycle"];
    assert_eq!(last["outcome"], "stayed", "the newer record stands: {last}");
}
