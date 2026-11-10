//! Issue #2211: live reading freshness and the projected swap, in real
//! cycles.
//!
//! The swap, journal, ledger and log are the real ones, under a temporary
//! HOME and profile root (`tests_swap::env()`: `work` live, `spare` stored).
//! The credential files there are fakes that hold no real token. Probing is
//! stubbed with `Ctx::measure`, which calls the real decision
//! (`decide_measured`); the live re-read is stubbed with `Ctx::reread`, which
//! records each name it is asked to read. No test reads a real login,
//! starts `claude`, uses the network or touches the developer's HOME.
//!
//! Time is synthetic: `Ctx::now` is set to a per-test static clock, seeded
//! once from the real clock and moved forward by [`INTERVAL`] seconds per
//! cycle by the test itself. Rates and ages come only from those synthetic
//! timestamps; there is no sleep, no `Instant` wait and no wall-clock window.
//!
//! Every test has its own statics (via [`lane!`]), since tests run on
//! parallel threads. Settings follow the 9 Nov rotator: `--interval 120
//! --session-at 70 --margin 5`.

use super::super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::super::decide::Probed;
use super::super::tests_issue_1870::{exh, rd};
use super::super::tests_revalidate_cause::{ctx_with, log_of};
use super::super::tests_swap::{Env, env, ino_at};
use crate::commands::profile::auto_switch::kit::now;

/// `--interval` in these tests, and the clock step between cycles.
const INTERVAL: i64 = 120;

/// What the live probe returns in one cycle.
#[derive(Clone)]
pub(super) enum Live {
    /// A direct read made this cycle: no `observed_at`.
    Direct(f64, f64),
    /// A passive reading taken `age` seconds before the cycle's clock.
    Passive(f64, f64, i64),
    /// The probe failed.
    Failed,
}

/// One test's statics and its stub functions. A test fills `LIVE` (one
/// entry per cycle; the last repeats), `OTHERS` (the candidates' probes,
/// the same every cycle) and `REREAD` (what each live re-read returns; the
/// last repeats; empty means a failed read), then calls [`run`].
macro_rules! lane {
    ($m:ident) => {
        // Not every test uses every stub of its lane.
        #[allow(dead_code)]
        mod $m {
            use super::super::super::cycle::Ctx;
            use super::super::super::decide::Probed;
            use super::super::super::observe::{Measurement, Snap, decide_measured};
            use super::super::super::tests_issue_1870::rd;
            use super::super::super::tests_revalidate_cause::probes_for;
            use super::Live;
            use crate::commands::profile::ProfileError;
            use crate::commands::profile::name::ProfileName;
            use std::sync::Mutex;
            use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
            use std::time::Duration;

            pub static CLOCK: AtomicI64 = AtomicI64::new(0);
            pub static STEP: AtomicUsize = AtomicUsize::new(0);
            pub static LIVE: Mutex<Vec<Live>> = Mutex::new(Vec::new());
            pub static OTHERS: Mutex<Vec<(&'static str, Probed)>> = Mutex::new(Vec::new());
            pub static REREAD: Mutex<Vec<Probed>> = Mutex::new(Vec::new());
            pub static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

            pub fn clock() -> i64 {
                CLOCK.load(Ordering::SeqCst)
            }

            pub fn measure(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
                let at = (ctx.now)();
                let i = STEP.fetch_add(1, Ordering::SeqCst);
                let script = LIVE.lock().unwrap().clone();
                let step = script
                    .get(i)
                    .or(script.last())
                    .cloned()
                    .unwrap_or(Live::Failed);
                let live = match step {
                    Live::Direct(s, w) => rd(s, w, None),
                    Live::Passive(s, w, age) => {
                        let mut p = rd(s, w, Some(at - age));
                        p.passive = true;
                        p
                    }
                    Live::Failed => Probed::failed(),
                };
                let table = OTHERS.lock().unwrap().clone();
                let probes = probes_for(snap, &table);
                decide_measured(ctx, snap, live, probes, at)
            }

            pub fn reread(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
                let mut calls = CALLS.lock().unwrap();
                calls.push(name.to_string());
                let nth = calls.len() - 1;
                let queue = REREAD.lock().unwrap();
                queue
                    .get(nth)
                    .or(queue.last())
                    .cloned()
                    .unwrap_or_else(Probed::failed)
            }

            pub fn calls() -> Vec<String> {
                CALLS.lock().unwrap().clone()
            }

            /// The re-reads of the live login (`work`) only.
            pub fn live_calls() -> usize {
                calls().iter().filter(|n| n.as_str() == "work").count()
            }
        }
    };
}

/// A context for `e` on the synthetic `clock`, with `--interval 120
/// --session-at 70 --margin 5`, the stubbed measurement and, when `reread`
/// is set, the stubbed re-read.
fn ctx(
    e: &Env,
    clock: fn() -> i64,
    measure: super::super::cycle::MeasureFn,
    reread: Option<super::super::fresh::RereadFn>,
) -> Ctx {
    let mut c = ctx_with(e, measure);
    c.reread = reread;
    c.now = clock;
    c.settings.interval = INTERVAL as u64;
    c.settings.session_at = 70;
    c.settings.margin = 5;
    c
}

/// Runs one watch cycle, then moves `clock` on by one interval.
fn step(c: &mut Ctx, clock: &std::sync::atomic::AtomicI64) -> CycleOutcome {
    let got = run_cycle(c, Mode::Watch).unwrap();
    clock.fetch_add(INTERVAL, std::sync::atomic::Ordering::SeqCst);
    got
}

fn seed(clock: &std::sync::atomic::AtomicI64) {
    clock.store(now(), std::sync::atomic::Ordering::SeqCst);
}

fn swaps(log: &str) -> usize {
    log.lines().filter(|l| l.contains(" swap ")).count()
}

const STALE: &str = "stale live reading; re-read";

fn stale_lines(log: &str) -> usize {
    log.lines().filter(|l| l.ends_with(STALE)).count()
}

/// `spare` with room: headroom 60, so `work` at 63% gains 23 (below the
/// default `--upgrade-margin` 30) and an upgrade never stands in for the
/// projected swap.
fn spare_room() -> Vec<(&'static str, Probed)> {
    vec![
        ("spare", rd(0.0, 40.0, None)),
        ("work", rd(0.0, 40.0, None)),
    ]
}

fn spare_full() -> Vec<(&'static str, Probed)> {
    vec![("spare", exh(100.0, 100.0))]
}

fn set<T: Clone>(m: &std::sync::Mutex<Vec<T>>, v: &[T]) {
    *m.lock().unwrap() = v.to_vec();
}

// --- (a) A frozen reading is read again on the third identical cycle -------

lane!(la);

#[test]
fn a_a_frozen_live_reading_is_read_again_and_decided_on_the_fresh_value() {
    let e = env();
    seed(&la::CLOCK);
    set(&la::LIVE, &[Live::Direct(40.0, 20.0)]);
    set(&la::OTHERS, &spare_room());
    set(&la::REREAD, &[rd(80.0, 20.0, None)]);
    let mut c = ctx(&e, la::clock, la::measure, Some(la::reread));
    assert_eq!(step(&mut c, &la::CLOCK), CycleOutcome::Stayed);
    assert_eq!(step(&mut c, &la::CLOCK), CycleOutcome::Stayed);
    assert_eq!(la::live_calls(), 0, "two identical readings are not frozen");
    let got = step(&mut c, &la::CLOCK);
    let log = log_of(&e);
    assert_eq!(la::live_calls(), 1, "the third identical reading: {log}");
    assert_eq!(stale_lines(&log), 1, "{log}");
    assert_eq!(got, CycleOutcome::Swapped, "decided on s=80%: {log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    assert!(log.contains("reason=threshold"), "{log}");
    assert!(
        log.find(STALE) < log.find(" swap work->spare "),
        "the re-read comes before the swap:\n{log}"
    );
}

// --- (a') A re-read that returns the same value is true; the count restarts -

lane!(lb);

#[test]
fn a2_a_reread_with_the_same_value_is_accepted_and_the_count_restarts() {
    let e = env();
    seed(&lb::CLOCK);
    set(&lb::LIVE, &[Live::Direct(40.0, 20.0)]);
    set(&lb::OTHERS, &spare_room());
    set(&lb::REREAD, &[rd(40.0, 20.0, None)]);
    let mut c = ctx(&e, lb::clock, lb::measure, Some(lb::reread));
    for _ in 0..3 {
        assert_eq!(step(&mut c, &lb::CLOCK), CycleOutcome::Stayed);
    }
    assert_eq!(lb::live_calls(), 1, "re-read on cycle 3");
    step(&mut c, &lb::CLOCK);
    assert_eq!(lb::live_calls(), 1, "cycle 4: one unchanged cycle since");
    step(&mut c, &lb::CLOCK);
    assert_eq!(lb::live_calls(), 2, "cycle 5: two unchanged cycles since");
    let log = log_of(&e);
    assert_eq!(stale_lines(&log), 2, "{log}");
    assert_eq!(swaps(&log), 0, "{log}");
}

// --- A session of 0% is never frozen ---------------------------------------

lane!(lz);

#[test]
fn a3_an_idle_login_at_zero_percent_is_never_read_again() {
    let e = env();
    seed(&lz::CLOCK);
    set(&lz::LIVE, &[Live::Direct(0.0, 20.0)]);
    set(&lz::OTHERS, &spare_room());
    let mut c = ctx(&e, lz::clock, lz::measure, Some(lz::reread));
    for _ in 0..5 {
        assert_eq!(step(&mut c, &lz::CLOCK), CycleOutcome::Stayed);
    }
    let log = log_of(&e);
    assert_eq!(lz::live_calls(), 0, "{log}");
    assert_eq!(stale_lines(&log), 0, "{log}");
}

// --- (b) A rising rate swaps before the threshold --------------------------

lane!(lr);

#[test]
fn b_a_rising_rate_swaps_with_reason_projected_before_the_threshold() {
    let e = env();
    seed(&lr::CLOCK);
    set(
        &lr::LIVE,
        &[Live::Direct(50.0, 41.0), Live::Direct(63.0, 41.0)],
    );
    set(&lr::OTHERS, &spare_room());
    let mut c = ctx(&e, lr::clock, lr::measure, Some(lr::reread));
    assert_eq!(step(&mut c, &lr::CLOCK), CycleOutcome::Stayed);
    let got = step(&mut c, &lr::CLOCK);
    let log = log_of(&e);
    assert_eq!(
        got,
        CycleOutcome::Swapped,
        "63 + 13 = 76 >= 70, and revalidation keeps it: {log}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    assert!(
        log.contains("swap work->spare live s=63% w=41% target s=0% w=40% reason=projected"),
        "the swap line shows the measured figures:\n{log}"
    );
    assert!(
        log.contains(" cycle live=work work:s=63%,w=41%"),
        "the cycle line keeps the real reading:\n{log}"
    );
    assert_eq!(
        lr::live_calls(),
        0,
        "a moving, fresh reading is not re-read"
    );
    assert_eq!(stale_lines(&log), 0, "{log}");
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.live.as_deref(), Some("spare"), "the ledger records it");
}

// --- (b') The hold does not block a projected swap --------------------------

lane!(lh);

#[test]
fn b2_a_projected_swap_is_not_held() {
    let e = env();
    seed(&lh::CLOCK);
    let mut l = e.rs.load_ledger().unwrap().unwrap();
    l.swapped_at = Some(lh::clock() - 10);
    e.rs.save_ledger(&l).unwrap();
    set(
        &lh::LIVE,
        &[Live::Direct(50.0, 41.0), Live::Direct(63.0, 41.0)],
    );
    set(&lh::OTHERS, &spare_room());
    let mut c = ctx(&e, lh::clock, lh::measure, Some(lh::reread));
    c.settings.hold = 3600;
    step(&mut c, &lh::CLOCK);
    let got = step(&mut c, &lh::CLOCK);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert!(log.contains("reason=projected"), "{log}");
}

// --- (c) A steady low rate does not swap ------------------------------------

lane!(ll);

#[test]
fn c_a_steady_low_rate_stays() {
    let e = env();
    seed(&ll::CLOCK);
    set(
        &ll::LIVE,
        &[Live::Direct(20.0, 20.0), Live::Direct(22.0, 20.0)],
    );
    set(&ll::OTHERS, &spare_room());
    let mut c = ctx(&e, ll::clock, ll::measure, Some(ll::reread));
    assert_eq!(step(&mut c, &ll::CLOCK), CycleOutcome::Stayed);
    assert_eq!(step(&mut c, &ll::CLOCK), CycleOutcome::Stayed);
    let log = log_of(&e);
    assert_eq!(swaps(&log), 0, "22 + 2 = 24 < 70: {log}");
    assert!(!log.contains("projected"), "{log}");
    assert_eq!(ll::live_calls(), 0, "{log}");
}

// --- (d) A failed live probe near the threshold is never a stay ------------

lane!(ld);

#[test]
fn d_a_failed_probe_near_the_threshold_swaps_with_reason_projected() {
    let e = env();
    seed(&ld::CLOCK);
    // One point at 66% (no rate): near, since 66 >= 70 - 5.
    set(&ld::LIVE, &[Live::Direct(66.0, 41.0), Live::Failed]);
    set(&ld::OTHERS, &spare_room());
    let mut c = ctx(&e, ld::clock, ld::measure, Some(ld::reread));
    assert_eq!(step(&mut c, &ld::CLOCK), CycleOutcome::Stayed);
    let got = step(&mut c, &ld::CLOCK);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "not a damped stay: {log}");
    assert!(log.contains("swap work->spare "), "{log}");
    assert!(log.contains("reason=projected"), "{log}");
}

lane!(ldn);

#[test]
fn d2_a_failed_probe_far_from_the_threshold_keeps_failed_probe_damping() {
    let e = env();
    seed(&ldn::CLOCK);
    set(&ldn::LIVE, &[Live::Direct(30.0, 20.0), Live::Failed]);
    set(&ldn::OTHERS, &spare_room());
    let mut c = ctx(&e, ldn::clock, ldn::measure, Some(ldn::reread));
    step(&mut c, &ldn::CLOCK);
    let got = step(&mut c, &ldn::CLOCK);
    let log = log_of(&e);
    assert_eq!(swaps(&log), 0, "{log}");
    assert_ne!(got, CycleOutcome::Swapped, "{log}");
    assert!(!log.contains("projected"), "{log}");
}

lane!(lde);

#[test]
fn d3_a_failed_first_probe_with_no_series_keeps_failed_probe_damping() {
    let e = env();
    seed(&lde::CLOCK);
    set(&lde::LIVE, &[Live::Failed]);
    set(&lde::OTHERS, &spare_room());
    let mut c = ctx(&e, lde::clock, lde::measure, Some(lde::reread));
    step(&mut c, &lde::CLOCK);
    let log = log_of(&e);
    assert_eq!(swaps(&log), 0, "{log}");
    assert!(!log.contains("projected"), "{log}");
}

// --- (e) An old live reading is read again ---------------------------------

lane!(le1);

#[test]
fn e1_an_old_live_reading_is_read_again_and_decided_on_the_fresh_value() {
    let e = env();
    seed(&le1::CLOCK);
    set(&le1::LIVE, &[Live::Passive(40.0, 20.0, INTERVAL + 1)]);
    set(&le1::OTHERS, &spare_room());
    set(&le1::REREAD, &[rd(80.0, 20.0, None)]);
    let mut c = ctx(&e, le1::clock, le1::measure, Some(le1::reread));
    let got = step(&mut c, &le1::CLOCK);
    let log = log_of(&e);
    assert_eq!(le1::calls(), ["work"], "one live re-read: {log}");
    assert_eq!(stale_lines(&log), 1, "{log}");
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert!(log.contains("reason=threshold"), "{log}");
}

lane!(le0);

#[test]
fn e1b_a_reading_exactly_one_interval_old_is_not_read_again() {
    let e = env();
    seed(&le0::CLOCK);
    set(&le0::LIVE, &[Live::Passive(40.0, 20.0, INTERVAL)]);
    set(&le0::OTHERS, &spare_room());
    set(&le0::REREAD, &[rd(80.0, 20.0, None)]);
    let mut c = ctx(&e, le0::clock, le0::measure, Some(le0::reread));
    assert_eq!(step(&mut c, &le0::CLOCK), CycleOutcome::Stayed);
    assert_eq!(le0::live_calls(), 0);
    assert_eq!(stale_lines(&log_of(&e)), 0);
}

lane!(le2);

#[test]
fn e2_an_old_reading_near_the_threshold_whose_reread_fails_swaps() {
    let e = env();
    seed(&le2::CLOCK);
    let at1 = le2::clock();
    // Cycle 1 reads 66% directly; cycle 2 sees only that same reading, now
    // older than one interval, and its re-read fails.
    set(&le2::LIVE, &[Live::Direct(66.0, 41.0)]);
    set(&le2::OTHERS, &spare_room());
    let mut c = ctx(&e, le2::clock, le2::measure, Some(le2::reread));
    assert_eq!(step(&mut c, &le2::CLOCK), CycleOutcome::Stayed);
    le2::CLOCK.store(at1 + 2 * INTERVAL, std::sync::atomic::Ordering::SeqCst);
    set(&le2::LIVE, &[Live::Passive(66.0, 41.0, 2 * INTERVAL)]);
    le2::STEP.store(0, std::sync::atomic::Ordering::SeqCst);
    let got = step(&mut c, &le2::CLOCK);
    let log = log_of(&e);
    assert_eq!(le2::live_calls(), 1, "{log}");
    assert_eq!(got, CycleOutcome::Swapped, "never a stay: {log}");
    assert!(log.contains("reason=projected"), "{log}");
}

lane!(le3);

#[test]
fn e3_an_old_reading_with_no_reread_hook_and_far_from_the_threshold_stays() {
    let e = env();
    seed(&le3::CLOCK);
    set(&le3::LIVE, &[Live::Passive(40.0, 20.0, 3 * INTERVAL)]);
    set(&le3::OTHERS, &spare_room());
    let mut c = ctx(&e, le3::clock, le3::measure, None);
    assert_eq!(step(&mut c, &le3::CLOCK), CycleOutcome::Stayed);
    let log = log_of(&e);
    assert_eq!(stale_lines(&log), 0, "no read was made: {log}");
    assert_eq!(swaps(&log), 0, "{log}");
}

// --- The 9 Nov kkane history -----------------------------------------------

fn kkane() -> Vec<Live> {
    [22.0, 32.0, 42.0, 50.0, 63.0]
        .into_iter()
        .map(|s| Live::Direct(s, 41.0))
        .collect()
}

lane!(lk);

#[test]
fn kkane_replay_stays_until_63_then_swaps_with_reason_projected() {
    let e = env();
    seed(&lk::CLOCK);
    set(&lk::LIVE, &kkane());
    set(&lk::OTHERS, &spare_room());
    let mut c = ctx(&e, lk::clock, lk::measure, Some(lk::reread));
    for s in [22, 32, 42, 50] {
        let got = step(&mut c, &lk::CLOCK);
        assert_eq!(got, CycleOutcome::Stayed, "s={s}%: {}", log_of(&e));
    }
    let got = step(&mut c, &lk::CLOCK);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "63 projects to 73.3: {log}");
    assert_eq!(swaps(&log), 1, "{log}");
    assert!(
        log.contains("swap work->spare live s=63% w=41%") && log.contains("reason=projected"),
        "{log}"
    );
    assert_eq!(lk::live_calls(), 0, "{log}");
}

lane!(lkn);

#[test]
fn kkane_with_no_candidate_alerts_at_63_and_rereads_the_third_identical_reading() {
    let e = env();
    seed(&lkn::CLOCK);
    let mut script = kkane();
    script.push(Live::Direct(63.0, 41.0)); // cycle 6: unchanged once
    script.push(Live::Direct(63.0, 41.0)); // cycle 7: unchanged twice
    set(&lkn::LIVE, &script);
    set(&lkn::OTHERS, &spare_full());
    set(&lkn::REREAD, &[rd(63.0, 41.0, None)]);
    let mut c = ctx(&e, lkn::clock, lkn::measure, Some(lkn::reread));
    for _ in 0..4 {
        step(&mut c, &lkn::CLOCK);
    }
    assert!(
        !log_of(&e).contains("ALERT: no profile has capacity"),
        "no ALERT below the projection: {}",
        log_of(&e)
    );
    step(&mut c, &lkn::CLOCK);
    let log = log_of(&e);
    assert!(
        log.contains("ALERT: no profile has capacity live=work s=63% w=41%"),
        "cycle 5 alerts on the projection: {log}"
    );
    step(&mut c, &lkn::CLOCK);
    assert_eq!(lkn::live_calls(), 0, "cycle 6: one unchanged cycle");
    step(&mut c, &lkn::CLOCK);
    let log = log_of(&e);
    assert_eq!(lkn::live_calls(), 1, "cycle 7 is re-read: {log}");
    assert_eq!(stale_lines(&log), 1, "{log}");
    assert_eq!(swaps(&log), 0, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
}

// --- A swap clears the series ----------------------------------------------

lane!(ls);

#[test]
fn a_failed_probe_just_after_a_projected_swap_is_damped_as_usual() {
    let e = env();
    seed(&ls::CLOCK);
    set(
        &ls::LIVE,
        &[
            Live::Direct(50.0, 41.0),
            Live::Direct(63.0, 41.0),
            Live::Failed,
        ],
    );
    set(&ls::OTHERS, &spare_room());
    let mut c = ctx(&e, ls::clock, ls::measure, Some(ls::reread));
    step(&mut c, &ls::CLOCK);
    assert_eq!(step(&mut c, &ls::CLOCK), CycleOutcome::Swapped);
    // `spare` is live now; its first failed probe has no series behind it.
    let got = step(&mut c, &ls::CLOCK);
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(swaps(&log), 1, "no flip back to work: {log}");
}

// --- Dry run --------------------------------------------------------------

lane!(ldr);

#[test]
fn a_dry_run_rereads_an_old_live_reading_and_writes_nothing() {
    let e = env();
    seed(&ldr::CLOCK);
    set(&ldr::LIVE, &[Live::Passive(40.0, 20.0, 3 * INTERVAL)]);
    set(&ldr::OTHERS, &spare_room());
    set(&ldr::REREAD, &[rd(80.0, 20.0, None)]);
    let before = std::fs::read(&e.rs.ledger).unwrap();
    let mut c = ctx(&e, ldr::clock, ldr::measure, Some(ldr::reread));
    let got = run_cycle(&mut c, Mode::DryRun).unwrap();
    assert_eq!(
        ldr::calls(),
        ["work"],
        "the dry run re-reads the live login"
    );
    assert_eq!(
        got,
        CycleOutcome::Swapped,
        "a real cycle would swap on s=80%"
    );
    assert_eq!(std::fs::read(&e.rs.ledger).unwrap(), before, "no write");
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
}
