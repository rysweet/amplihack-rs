//! Portable cycle contracts. All time, identity and exchange inputs are controlled.
use super::super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::super::decide::{Probed, Win};
use super::super::observe::{Measurement, Snap, decide_measured};
use super::super::tests_revalidate_cause::{add_stored, ctx_with, ok, probes_for};
use super::super::tests_swap::{Env, env, ino_at, n, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, test_exchange};
use std::cell::{Cell, RefCell};
use std::fs;

pub const T: i64 = 4_000_000_000;
#[path = "../../../../tests/support/rotate_2242_legacy.rs"]
mod legacy;
thread_local! {
    static CLOCK: Cell<i64> = const { Cell::new(T) };
    static MEASUREMENTS: Cell<usize> = const { Cell::new(0) };
    static READS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static SAMPLE: RefCell<Probed> = RefCell::new(ok(90.0, 20.0));
}

pub fn clock() -> i64 {
    CLOCK.with(Cell::get)
}
pub fn advance(seconds: i64) {
    CLOCK.with(|c| c.set(c.get() + seconds));
}
pub fn measurements() -> usize {
    MEASUREMENTS.with(Cell::get)
}
pub fn reads() -> Vec<String> {
    READS.with(|r| r.borrow().clone())
}
pub fn sample(session: f64, week: f64, age: i64, reset: i64) {
    let mut p = ok(session, week);
    p.observed_at = Some(clock() - age);
    p.windows.session = Some(Win {
        used: session,
        resets_at: Some(reset),
    });
    p.windows.week = Some(Win {
        used: week,
        resets_at: Some(reset),
    });
    SAMPLE.with(|s| *s.borrow_mut() = p);
}

pub fn measure(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    MEASUREMENTS.with(|c| c.set(c.get() + 1));
    let p = SAMPLE.with(|s| s.borrow().clone());
    decide_measured(
        ctx,
        snap,
        p,
        probes_for(snap, &[("spare", ok(5.0, 10.0)), ("extra", ok(25.0, 25.0))]),
        clock(),
    )
}

pub fn context(e: &Env) -> Ctx {
    CLOCK.with(|c| c.set(T));
    MEASUREMENTS.with(|c| c.set(0));
    READS.with(|r| r.borrow_mut().clear());
    sample(90.0, 20.0, 0, T + 10_000);
    let mut ctx = ctx_with(e, measure);
    ctx.now = clock;
    ctx.settings.interval = 120;
    ctx.settings.hold = 0;
    // Projection tests must distinguish predictive relief from an unrelated
    // preference upgrade onto the fixture's almost unused spare account.
    ctx.settings.upgrade_margin = 100;
    ctx.settings.session_at = 85;
    ctx.settings.week_at = 90;
    ctx.identity = |dir| {
        READS.with(|r| r.borrow_mut().push(dir.to_string_lossy().into_owned()));
        super::tests_fixture_identity::read(dir)
    };
    ctx
}

pub fn fixture() -> Env {
    let e = env();
    add_stored(&e, "extra");
    // Remove unrelated key warnings and seed all ownership records so a
    // deliberately occupied outgoing slot reaches duplicate verification.
    for dir in [e.home.clone(), e.root.join("spare")] {
        let mut v: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join(".claude.json")).unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("primaryApiKey");
        private(&dir.join(".claude.json"), &v.to_string());
    }
    let state = State::new(&e.root).unwrap();
    for name in ["work", "spare", "extra"] {
        let dir = if name == "work" {
            e.home.join(".claude")
        } else {
            e.root.join(name)
        };
        let account = super::tests_fixture_identity::read(&dir).unwrap();
        crate::commands::profile::expected::record(&state, &n(name), &account).unwrap();
    }
    e
}

pub fn drive(e: &Env, ctx: &mut Ctx) -> Result<CycleOutcome, ProfileError> {
    test_exchange::within(e._tmp.path(), || run_cycle(ctx, Mode::Watch))
}

pub fn log(e: &Env) -> String {
    fs::read_to_string(&e.rs.log).unwrap_or_default()
}
pub fn json(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[path = "tests_issue_2242_custody.rs"]
mod custody;
#[path = "tests_issue_2242_duplicate.rs"]
mod duplicate;
#[path = "tests_issue_2242_fallback.rs"]
mod fallback;
#[path = "tests_issue_2242_projection.rs"]
mod projection;
#[path = "tests_issue_2242_publication.rs"]
mod publication;
#[path = "tests_issue_2242_recovery.rs"]
mod recovery;
