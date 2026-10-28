//! Which rule wins when a `profile switch` pin (#1691) and the overflow
//! policy (#1606) both apply.
//!
//! The rule: **the pin wins in both directions.** The pin gate runs before
//! measuring, so `decide_overflow` never runs while a pin is in force.
//! - A pin on an overflow profile is the user choosing to spend. The rotator
//!   does not move off it, even when a plan login has capacity, until
//!   `profile switch --none`. `rotate status` reports
//!   `on overflow: yes (<name>, held by switch)`.
//! - A pin on a plan login never moves onto overflow, even when every plan
//!   login is exhausted. Nothing is spent until the pin is released.
//! - Moves the rotator makes itself (onto or off overflow) never write a pin.
//!
//! "The hold" in `overflow.rs` ("off overflow ... ignoring the hold") is the
//! `--hold` minimum time after a rotator swap, not this pin.
//!
//! In process, on the `tests_swap.rs` fixture. `Ctx::measure` is replaced by
//! a stub that returns the decision the overflow policy would make, and
//! records that it ran, so each test shows that the pin, not the readings,
//! decided.

use super::control::on_overflow_line;
use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::decide::{Decision, Reading, Reason};
use super::observe::Measurement;
use super::overflow::Tag;
use super::pin::{self, PinState};
use super::settings;
use super::switch::{Switched, release, switch_live};
use super::tests_swap::{Env, env, ino_at, json, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::config::{self, ReadMode};
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::overflow::{OverflowEntry, overflow_names};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

fn mark_overflow(e: &Env, name: &str) {
    config::update(&e.root, ReadMode::Editable, |c| {
        c.overflow.push(OverflowEntry::new(n(name), 1));
        Ok(())
    })
    .unwrap();
}

fn switch_to(e: &Env, name: &str) {
    match switch_live(&e.store, &e.home, &n(name)) {
        Ok(Ok(Switched::Moved { .. })) => {}
        other => panic!("switch {name} failed: {other:?}"),
    }
}

fn ctx(e: &Env, measure: super::cycle::MeasureFn) -> Ctx {
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut c = Ctx::new(e.store.clone(), e.home.clone(), s);
    c.measure = Some(measure);
    c
}

fn on_overflow(e: &Env) -> String {
    let l = e.rs.load_ledger().unwrap().unwrap_or_default();
    let tag = l.last_cycle.and_then(|c| c.overflow);
    on_overflow_line(
        &e.store,
        &e.rs,
        Some(&e.home),
        &overflow_names(&e.root),
        tag.as_deref(),
        &pin::read(&e.rs),
    )
}

fn log(e: &Env) -> String {
    fs::read_to_string(&e.rs.log).unwrap_or_default()
}

static OFF_MEASURED: AtomicUsize = AtomicUsize::new(0);

/// Live on overflow `spare`; plan login `work` has capacity. The policy
/// moves off overflow onto `work`.
fn off_overflow(_: &Ctx, _: &super::observe::Snap) -> Result<Measurement, ProfileError> {
    OFF_MEASURED.fetch_add(1, Ordering::SeqCst);
    let at = now();
    Ok(Measurement {
        live: Reading::Measured {
            session: 0.1,
            week: 0.1,
        },
        others: vec![(
            n("work"),
            Reading::Measured {
                session: 0.1,
                week: 0.1,
            },
        )],
        decision: Decision::Swap {
            to: n("work"),
            reason: Reason::OverflowOut,
        },
        note: None,
        failed: 0,
        tag: Some(Tag::Out),
        new_blocks: Vec::new(),
        next_plan_reset: None,
        measured_at: at,
    })
}

#[test]
fn a_pin_on_an_overflow_profile_holds_it_while_a_plan_login_has_capacity() {
    let e = env();
    mark_overflow(&e, "spare");
    switch_to(&e, "spare");
    let held = ino_at(&e.live());
    assert_eq!(held, Some(e.t), "spare's login is live");
    let mut c = ctx(&e, off_overflow);

    for i in 0..2 {
        assert_eq!(
            run_cycle(&mut c, Mode::Watch).unwrap(),
            CycleOutcome::Held,
            "cycle {i}: the pin wins over moving off overflow"
        );
        assert_eq!(ino_at(&e.live()), held, "cycle {i} moved the login");
    }
    assert_eq!(
        OFF_MEASURED.load(Ordering::SeqCst),
        0,
        "the overflow policy never ran while pinned"
    );
    assert!(json(&e.rs.ledger)["last_cycle"]["overflow"].is_null());
    assert_eq!(on_overflow(&e), "yes (spare, held by switch)");

    // Released, the next cycle moves straight off overflow; the move writes
    // no pin.
    release(&e.store).unwrap();
    assert_eq!(
        on_overflow(&e),
        "no (spare is overflow; plan capacity remains)"
    );
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped
    );
    assert_eq!(OFF_MEASURED.load(Ordering::SeqCst), 1);
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login is live again");
    assert_eq!(pin::read(&e.rs), PinState::None, "an auto-move never pins");
    assert_eq!(json(&e.rs.ledger)["last_cycle"]["overflow"], "out");
    assert!(
        log(&e).contains("overflow: off spare; back on plan profile work"),
        "{}",
        log(&e)
    );
}

static ONTO_MEASURED: AtomicUsize = AtomicUsize::new(0);

/// Live on plan login `spare` at its threshold; plan login `work` is
/// exhausted too and is not a candidate. The policy moves onto overflow
/// `work`.
fn onto_overflow(_: &Ctx, _: &super::observe::Snap) -> Result<Measurement, ProfileError> {
    ONTO_MEASURED.fetch_add(1, Ordering::SeqCst);
    Ok(Measurement {
        live: Reading::Exhausted,
        others: vec![(
            n("work"),
            Reading::Measured {
                session: 0.0,
                week: 0.0,
            },
        )],
        decision: Decision::Swap {
            to: n("work"),
            reason: Reason::OverflowIn,
        },
        note: None,
        failed: 0,
        tag: Some(Tag::In),
        new_blocks: Vec::new(),
        next_plan_reset: Some(now() + 4 * 3600),
        measured_at: now(),
    })
}

#[test]
fn a_pin_on_a_plan_login_never_moves_onto_overflow_when_every_plan_login_is_out() {
    let e = env();
    mark_overflow(&e, "work");
    switch_to(&e, "spare");
    let held = ino_at(&e.live());
    let mut c = ctx(&e, onto_overflow);

    for i in 0..2 {
        assert_eq!(
            run_cycle(&mut c, Mode::Watch).unwrap(),
            CycleOutcome::Held,
            "cycle {i}: the pin wins over moving onto overflow"
        );
        assert_eq!(ino_at(&e.live()), held, "cycle {i} moved onto credits");
    }
    assert_eq!(
        ONTO_MEASURED.load(Ordering::SeqCst),
        0,
        "the overflow policy never ran while pinned"
    );
    let last = &json(&e.rs.ledger)["last_cycle"];
    assert!(last["overflow"].is_null(), "nothing spent: {last}");
    assert_eq!(on_overflow(&e), "no");
    assert!(!log(&e).contains("overflow: on "), "{}", log(&e));
}
