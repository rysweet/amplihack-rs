//! Issue #2211: live reading freshness and the projected swap, in real
//! cycles.
//!
//! The swap, journal, ledger and log are the real ones, under a temporary
//! HOME and profile root (`tests_swap::env()`: `work` live, `spare` stored).
//! The credential files there are fakes that hold no real token. Probing is
//! stubbed with `Ctx::measure`, which calls the real decision
//! (`decide_measured`); the live re-read is stubbed with `Ctx::reread_live`
//! (and the target re-read with `Ctx::reread`), which records each name it
//! is asked to read. No test reads a real login,
//! starts `claude`, uses the network or touches the developer's HOME.
//!
//! Time is synthetic: `Ctx::now` is set to a per-test static clock, seeded
//! once from the real clock and moved forward by [`INTERVAL`] seconds per
//! cycle by the test itself. A test can also make the measurement take time
//! (`MEASURE_SECS`), so the live step and the target gate run on a later
//! clock than `measured_at`, as they do in production. Rates and ages come
//! only from those synthetic timestamps; there is no sleep, no `Instant`
//! wait and no wall-clock window.
//!
//! Every test has its own statics (via [`lane!`]), since tests run on
//! parallel threads. Settings follow the 9 Nov rotator: `--interval 120
//! --session-at 70 --margin 5`.

use super::super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::super::decide::Probed;
use super::super::fresh::live::{Basis, FROZEN_CYCLES};
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
/// the same every cycle) and `REREAD` (what each re-read, live or target,
/// returns in call order; the last repeats; empty means a failed read), and
/// may set `MEASURE_SECS` (how long each measurement takes on the clock).
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
            use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
            use std::time::Duration;

            pub static CLOCK: AtomicI64 = AtomicI64::new(0);
            pub static STEP: AtomicUsize = AtomicUsize::new(0);
            pub static LIVE: Mutex<Vec<Live>> = Mutex::new(Vec::new());
            pub static OTHERS: Mutex<Vec<(&'static str, Probed)>> = Mutex::new(Vec::new());
            pub static REREAD: Mutex<Vec<Probed>> = Mutex::new(Vec::new());
            pub static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());
            /// Seconds each measurement takes: the clock moves on by this
            /// much after `measured_at` is taken.
            pub static MEASURE_SECS: AtomicI64 = AtomicI64::new(0);
            /// Whether the context was read-only during the last re-read.
            pub static READ_ONLY: AtomicBool = AtomicBool::new(false);

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
                let decided = decide_measured(ctx, snap, live, probes, at);
                CLOCK.fetch_add(MEASURE_SECS.load(Ordering::SeqCst), Ordering::SeqCst);
                decided
            }

            pub fn reread(ctx: &Ctx, name: &ProfileName, _: Duration) -> Probed {
                READ_ONLY.store(ctx.read_only, Ordering::SeqCst);
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
/// is set, the stubbed re-read, as both the live re-read
/// (`Ctx::reread_live`) and the target re-read (`Ctx::reread`).
fn ctx(
    e: &Env,
    clock: fn() -> i64,
    measure: super::super::cycle::MeasureFn,
    reread: Option<super::super::fresh::RereadFn>,
) -> Ctx {
    let mut c = ctx_with(e, measure);
    c.reread = reread;
    c.reread_live = reread;
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
const FAILED: &str = "reread: work failed; treated as unknown";

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
    let (stale, failed) = (log.find(STALE), log.find(FAILED));
    assert!(failed.is_some() && stale < failed, "failure logged:\n{log}");
}

// --- A substituted figure is never lower than the last known one ----------

lane!(lsf);

#[test]
fn e2b_a_stale_95_percent_whose_reread_fails_still_swaps_to_a_70_percent_candidate() {
    let e = env();
    seed(&lsf::CLOCK);
    let at1 = lsf::clock();
    set(&lsf::LIVE, &[Live::Direct(66.0, 41.0)]);
    set(
        &lsf::OTHERS,
        &[
            ("spare", rd(70.0, 40.0, None)),
            ("work", rd(0.0, 40.0, None)),
        ],
    );
    let mut c = ctx(&e, lsf::clock, lsf::measure, Some(lsf::reread));
    assert_eq!(step(&mut c, &lsf::CLOCK), CycleOutcome::Stayed);
    // Cycle 2 sees a stale 95%, and its re-read fails.
    lsf::CLOCK.store(at1 + 2 * INTERVAL, std::sync::atomic::Ordering::SeqCst);
    set(&lsf::LIVE, &[Live::Passive(95.0, 41.0, 2 * INTERVAL)]);
    lsf::STEP.store(0, std::sync::atomic::Ordering::SeqCst);
    let got = step(&mut c, &lsf::CLOCK);
    let log = log_of(&e);
    assert_eq!(lsf::live_calls(), 1, "{log}");
    assert_eq!(got, CycleOutcome::Swapped, "decided on 95, not 70: {log}");
    assert!(log.contains("reason=threshold"), "95% needs a swap: {log}");
    assert!(!log.contains("ALERT: no profile has capacity"), "{log}");
}

lane!(lsg);

#[test]
fn e2c_a_failed_probe_after_90_percent_is_decided_on_90_not_the_threshold() {
    let e = env();
    seed(&lsg::CLOCK);
    set(&lsg::LIVE, &[Live::Direct(90.0, 41.0)]);
    set(&lsg::OTHERS, &spare_full());
    let mut c = ctx(&e, lsg::clock, lsg::measure, Some(lsg::reread));
    assert_ne!(step(&mut c, &lsg::CLOCK), CycleOutcome::Swapped);
    // `spare` at 75%: 15 more headroom than 90%, none more than 70%.
    set(&lsg::LIVE, &[Live::Failed]);
    set(
        &lsg::OTHERS,
        &[
            ("spare", rd(75.0, 40.0, None)),
            ("work", rd(0.0, 40.0, None)),
        ],
    );
    lsg::STEP.store(0, std::sync::atomic::Ordering::SeqCst);
    let got = step(&mut c, &lsg::CLOCK);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "not an ALERT: {log}");
    assert!(log.contains("swap work->spare "), "{log}");
    assert!(log.contains("reason=projected"), "{log}");
}

// --- A live re-read preserves an explicit evidence timestamp ----------------

lane!(lst);

#[test]
fn e2d_a_live_reread_preserves_its_evidence_timestamp() {
    let e = env();
    seed(&lst::CLOCK);
    let t1 = lst::clock();
    set(
        &lst::LIVE,
        &[
            Live::Direct(40.0, 41.0),
            Live::Direct(50.0, 41.0),
            Live::Passive(50.0, 41.0, INTERVAL + 1),
        ],
    );
    set(&lst::OTHERS, &spare_room());
    // Stamped at t1 + 140, 100 s before cycle 3's clock: 40% to 58% over
    // 140 s projects 58 + 15.4 = 73.4. Dated by the clock (t1 + 240) it
    // would be 18 points over 240 s, 58 + 9 = 67, and a stay.
    set(&lst::REREAD, &[rd(58.0, 41.0, Some(t1 + 140))]);
    let mut c = ctx(&e, lst::clock, lst::measure, Some(lst::reread));
    assert_eq!(step(&mut c, &lst::CLOCK), CycleOutcome::Stayed);
    assert_eq!(
        step(&mut c, &lst::CLOCK),
        CycleOutcome::Stayed,
        "50 + 10 = 60"
    );
    let got = step(&mut c, &lst::CLOCK);
    let log = log_of(&e);
    assert_eq!(lst::live_calls(), 1, "{log}");
    assert_eq!(got, CycleOutcome::Swapped, "58 + 15.4 >= 70: {log}");
    assert_eq!(swaps(&log), 1, "{log}");
    assert!(log.contains("reason=projected"), "{log}");
}

// --- A live reading from the future is read again ---------------------------

lane!(lfu);

#[test]
fn e2e_a_live_reading_stamped_after_the_clock_is_read_again() {
    let e = env();
    seed(&lfu::CLOCK);
    set(&lfu::LIVE, &[Live::Passive(40.0, 20.0, -60)]);
    set(&lfu::OTHERS, &spare_room());
    set(&lfu::REREAD, &[rd(80.0, 20.0, None)]);
    let mut c = ctx(&e, lfu::clock, lfu::measure, Some(lfu::reread));
    let got = step(&mut c, &lfu::CLOCK);
    let log = log_of(&e);
    assert_eq!(lfu::live_calls(), 1, "observed_at = t + 60: {log}");
    assert_eq!(stale_lines(&log), 1, "{log}");
    assert_eq!(got, CycleOutcome::Swapped, "decided on s=80%: {log}");
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

// --- A login rising 10 points a cycle -----------------------------------------------

fn rising() -> Vec<Live> {
    [22.0, 32.0, 42.0, 50.0, 63.0]
        .into_iter()
        .map(|s| Live::Direct(s, 41.0))
        .collect()
}

lane!(lk);

#[test]
fn rising_replay_stays_until_63_then_swaps_with_reason_projected() {
    let e = env();
    seed(&lk::CLOCK);
    set(&lk::LIVE, &rising());
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
fn rising_with_no_candidate_alerts_at_63_and_rereads_the_third_identical_reading() {
    let e = env();
    seed(&lkn::CLOCK);
    let mut script = rising();
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
    assert!(
        ldr::READ_ONLY.load(std::sync::atomic::Ordering::SeqCst),
        "the re-read ran on a read-only context"
    );
    assert_eq!(
        got,
        CycleOutcome::Swapped,
        "a real cycle would swap on s=80%"
    );
    assert_eq!(std::fs::read(&e.rs.ledger).unwrap(), before, "no write");
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
}

// --- The live re-read goes through `Ctx::reread_live` only ------------------

lane!(lhk);

#[test]
fn the_live_reread_uses_the_live_hook_and_never_the_target_hook() {
    let e = env();
    seed(&lhk::CLOCK);
    set(&lhk::LIVE, &[Live::Direct(40.0, 20.0)]);
    set(&lhk::OTHERS, &spare_room());
    set(&lhk::REREAD, &[rd(40.0, 20.0, None)]);
    let mut c = ctx(&e, lhk::clock, lhk::measure, None);
    // The target hook reads profile directories with refresh allowed; the
    // live login must never be read through it.
    c.reread = Some(lhk::reread);
    for _ in 0..3 {
        assert_eq!(step(&mut c, &lhk::CLOCK), CycleOutcome::Stayed);
    }
    let log = log_of(&e);
    assert_eq!(
        lhk::live_calls(),
        0,
        "no live read by the target hook: {log}"
    );
    assert_eq!(stale_lines(&log), 0, "no live hook, no re-read: {log}");

    c.reread = None;
    c.reread_live = Some(lhk::reread);
    // With no live hook, cycle 3's re-read could not be made, and a re-read
    // that was not made proves nothing: the frozen count was kept, so the
    // fourth identical reading is still frozen and is read by the live hook.
    step(&mut c, &lhk::CLOCK);
    let log = log_of(&e);
    assert_eq!(lhk::live_calls(), 1, "frozen, read by the live hook: {log}");
    assert_eq!(stale_lines(&log), 1, "{log}");
    // That re-read agreed (40%): the count restarts.
    step(&mut c, &lhk::CLOCK);
    assert_eq!(lhk::live_calls(), 1, "one unchanged cycle after the reset");
}

// --- A cache that stays frozen below a re-read is read again every cycle ---

lane!(lfz);

#[test]
fn a_cache_frozen_below_a_disagreeing_reread_is_read_again_every_cycle() {
    let e = env();
    seed(&lfz::CLOCK);
    // The status-line cache stays at 63% while the login climbs.
    set(&lfz::LIVE, &[Live::Direct(63.0, 20.0)]);
    set(&lfz::OTHERS, &spare_room());
    set(
        &lfz::REREAD,
        &[
            rd(65.0, 20.0, None),
            rd(68.0, 20.0, None),
            rd(71.0, 20.0, None),
        ],
    );
    let mut c = ctx(&e, lfz::clock, lfz::measure, Some(lfz::reread));
    assert_eq!(step(&mut c, &lfz::CLOCK), CycleOutcome::Stayed);
    assert_eq!(step(&mut c, &lfz::CLOCK), CycleOutcome::Stayed);
    assert_eq!(lfz::live_calls(), 0);
    // Cycle 3: frozen, re-read at 65%.
    assert_eq!(step(&mut c, &lfz::CLOCK), CycleOutcome::Stayed);
    assert_eq!(lfz::live_calls(), 1);
    // The re-read disagreed: the cache is behind, and the frozen count is
    // kept on the cached 63%, not restarted by the re-read figure.
    assert!(c.live_series.behind(), "cache marked behind after cycle 3");
    let unchanged3 = c.live_series.unchanged();
    assert!(
        unchanged3 >= FROZEN_CYCLES,
        "frozen count after cycle 3: {unchanged3}"
    );
    let len3 = c.live_series.len();
    // Cycle 4: the cache still says 63%. It is not a new session window and
    // not a fresh change: it is read again, at 68%.
    assert_eq!(step(&mut c, &lfz::CLOCK), CycleOutcome::Stayed);
    assert_eq!(lfz::live_calls(), 2, "{}", log_of(&e));
    assert!(c.live_series.behind(), "cache still behind after cycle 4");
    let unchanged4 = c.live_series.unchanged();
    assert_eq!(
        unchanged4,
        unchanged3 + 1,
        "the frozen count keeps rising while the cache stays at 63%"
    );
    assert!(
        c.live_series.len() >= len3,
        "the series was cleared as a new window: {} points after cycle 4, {len3} after cycle 3",
        c.live_series.len()
    );
    let r = c.live_series.rate(INTERVAL);
    assert!(
        r.is_some_and(|r| r > 0.0),
        "rate from the re-read figures: {r:?}"
    );
    // Cycle 5: read again at 71%, over --session-at.
    let got = step(&mut c, &lfz::CLOCK);
    let log = log_of(&e);
    assert_eq!(lfz::live_calls(), 3, "{log}");
    assert_eq!(stale_lines(&log), 3, "{log}");
    assert_eq!(got, CycleOutcome::Swapped, "decided on s=71%: {log}");
    assert!(log.contains("reason=threshold"), "{log}");
    assert_eq!(swaps(&log), 1, "{log}");
    // The swap hands the series to the new live login: the old login's
    // frozen count and behind mark do not follow it.
    assert_eq!(
        (
            c.live_series.unchanged(),
            c.live_series.behind(),
            c.live_series.len()
        ),
        (0, false, 0),
        "series reset by the swap (cycle 4 count was {unchanged4})"
    );
}

lane!(llg);

#[test]
fn a_moving_cache_still_below_a_disagreeing_reread_is_read_again() {
    let e = env();
    seed(&llg::CLOCK);
    set(
        &llg::LIVE,
        &[
            Live::Direct(63.0, 20.0),
            Live::Direct(63.0, 20.0),
            Live::Direct(63.0, 20.0),
            Live::Direct(64.0, 20.0),
            Live::Direct(69.0, 20.0),
        ],
    );
    set(&llg::OTHERS, &spare_room());
    set(&llg::REREAD, &[rd(65.0, 20.0, None), rd(66.0, 20.0, None)]);
    let mut c = ctx(&e, llg::clock, llg::measure, Some(llg::reread));
    for _ in 0..3 {
        assert_eq!(step(&mut c, &llg::CLOCK), CycleOutcome::Stayed);
    }
    assert_eq!(llg::live_calls(), 1, "frozen on cycle 3");
    // Cycle 4: the cache moved to 64%, still below the 65% re-read.
    assert_eq!(step(&mut c, &llg::CLOCK), CycleOutcome::Stayed);
    assert_eq!(llg::live_calls(), 2, "{}", log_of(&e));
    // Cycle 5: the cache is past the last re-read (66%), so it is current
    // again and is not read.
    step(&mut c, &llg::CLOCK);
    assert_eq!(llg::live_calls(), 2, "{}", log_of(&e));
}

// Issue comment: 100% -> 90%, then an older 0%/0% for the login just left.
lane!(bounce);

#[test]
fn a_hundred_to_ninety_swap_never_bounces_back_on_older_zero_capacity() {
    use std::sync::atomic::Ordering;
    let e = env();
    seed(&bounce::CLOCK);
    let before_use = bounce::CLOCK.load(Ordering::SeqCst) - 1;
    set(&bounce::LIVE, &[Live::Direct(100.0, 20.0)]);
    set(&bounce::OTHERS, &[("spare", rd(90.0, 20.0, None))]);
    set(&bounce::REREAD, &[exh(100.0, 20.0)]);
    let mut c = ctx(&e, bounce::clock, bounce::measure, Some(bounce::reread));
    assert_eq!(step(&mut c, &bounce::CLOCK), CycleOutcome::Swapped);
    assert_eq!(ino_at(&e.live()), Some(e.t), "90% spare becomes live");
    // Its next measured value is 100%. Work looks completely unused, but
    // that measurement predates its most recent use.
    set(&bounce::OTHERS, &[("work", rd(0.0, 0.0, Some(before_use)))]);
    let got = step(&mut c, &bounce::CLOCK);
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(swaps(&log), 1, "{log}");
    assert_eq!(
        ino_at(&e.live()),
        Some(e.t),
        "exhausted work is not restored"
    );
    assert_eq!(bounce::calls(), ["work"], "required target reread");
    assert!(
        log.contains("reading stale s=0% w=0%; fresh s=100% w=20%"),
        "{log}"
    );
}

// --- Crusty round 6: a live re-read after a slow measurement ----------------

lane!(lsl);

/// The measurement takes 5 s, so the live re-read is dated 5 s after
/// `measured_at`, where `spare`'s direct reading is dated. The target gate
/// must not read `spare` again: here that read would fail, and the swap
/// would become a false `no-capacity` ALERT.
#[test]
fn a_live_reread_after_a_five_second_measurement_never_rereads_the_target() {
    use std::sync::atomic::Ordering;
    let e = env();
    seed(&lsl::CLOCK);
    lsl::MEASURE_SECS.store(5, Ordering::SeqCst);
    set(&lsl::LIVE, &[Live::Passive(40.0, 20.0, INTERVAL + 1)]);
    set(&lsl::OTHERS, &spare_room());
    // Call 1, the live re-read: 75%. Call 2 would be spare's: failed.
    set(&lsl::REREAD, &[rd(75.0, 20.0, None), Probed::failed()]);
    let mut c = ctx(&e, lsl::clock, lsl::measure, Some(lsl::reread));
    let got = step(&mut c, &lsl::CLOCK);
    let log = log_of(&e);
    assert_eq!(lsl::calls(), ["work"], "no target re-read: {log}");
    assert_eq!(stale_lines(&log), 1, "{log}");
    assert!(!log.contains("reread: spare"), "{log}");
    assert!(!log.contains("ALERT: no profile has capacity"), "{log}");
    assert_eq!(got, CycleOutcome::Swapped, "decided on s=75%: {log}");
    assert!(log.contains("reason=threshold"), "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
}

// --- Crusty round 6: a frozen 66% near the threshold whose re-read fails ---
//
// The status line keeps rewriting an old 66% (`--session-at 70 --margin
// 5`). Whether or not its observation time moves, the third identical
// reading is read again; when that read fails, the cycle is decided on 70%:
// a swap or the ALERT, never a stay. The failure keeps the frozen count, so
// the next identical cycle reads again rather than trusting the 66%.
//
// A `no-capacity` ALERT while the live reading is below 100% is advisory
// (#1907, `capacity::Room::alert`): the cycle returns `Stayed` and the
// ledger keeps the key in `advisory_alert`. A decided stay clears that
// field, so it tells the ALERT from the stay these tests rule out.

/// The ledger's advisory ALERT key, set by a no-capacity ALERT and cleared
/// by a stay.
fn advisory(e: &Env) -> Option<String> {
    e.rs.load_ledger().unwrap().unwrap().advisory_alert
}

fn basis(c: &Ctx) -> Option<Basis> {
    c.live_outlook.as_ref().map(|o| o.basis)
}

lane!(lfm);

#[test]
fn a_frozen_66_with_a_moving_cache_time_whose_reread_fails_swaps() {
    let e = env();
    seed(&lfm::CLOCK);
    // Observed at each cycle's own clock: the time moves, the figures do not.
    set(&lfm::LIVE, &[Live::Passive(66.0, 41.0, 0)]);
    set(&lfm::OTHERS, &spare_room());
    let mut c = ctx(&e, lfm::clock, lfm::measure, Some(lfm::reread));
    assert_eq!(step(&mut c, &lfm::CLOCK), CycleOutcome::Stayed);
    assert_eq!(step(&mut c, &lfm::CLOCK), CycleOutcome::Stayed);
    assert_eq!(lfm::live_calls(), 0);
    let got = step(&mut c, &lfm::CLOCK);
    let log = log_of(&e);
    assert_eq!(lfm::live_calls(), 1, "the third identical reading: {log}");
    let (stale, failed) = (log.find(STALE), log.find(FAILED));
    assert!(failed.is_some() && stale < failed, "failure logged:\n{log}");
    assert_eq!(got, CycleOutcome::Swapped, "never a stay on 66%: {log}");
    assert!(
        log.contains("swap work->spare live s=66% w=41%") && log.contains("reason=projected"),
        "{log}"
    );
}

lane!(lfa);

#[test]
fn a_frozen_66_with_a_moving_cache_time_whose_reread_fails_alerts_and_is_read_again() {
    let e = env();
    seed(&lfa::CLOCK);
    set(&lfa::LIVE, &[Live::Passive(66.0, 41.0, 0)]);
    set(&lfa::OTHERS, &spare_full());
    let mut c = ctx(&e, lfa::clock, lfa::measure, Some(lfa::reread));
    assert_eq!(step(&mut c, &lfa::CLOCK), CycleOutcome::Stayed);
    assert_eq!(step(&mut c, &lfa::CLOCK), CycleOutcome::Stayed);
    assert_eq!(advisory(&e), None, "no ALERT on 66% as measured");
    let got = step(&mut c, &lfa::CLOCK);
    let log = log_of(&e);
    assert_eq!(lfa::live_calls(), 1, "{log}");
    assert_eq!(basis(&c), Some(Basis::Substituted), "decided on 70%: {log}");
    assert!(
        log.contains("ALERT: no profile has capacity live=work s=66% w=41%"),
        "never a stay on 66%: {log}"
    );
    assert_eq!(got, CycleOutcome::Stayed, "an advisory ALERT: {log}");
    assert!(advisory(&e).is_some(), "the ALERT is recorded: {log}");
    assert!(
        c.live_series.unchanged() >= FROZEN_CYCLES,
        "the failed re-read kept the frozen count: {}",
        c.live_series.unchanged()
    );
    // Cycle 4: the same 66% is still frozen and is read again, not trusted.
    step(&mut c, &lfa::CLOCK);
    let log = log_of(&e);
    assert_eq!(lfa::live_calls(), 2, "read again on cycle 4: {log}");
    assert_eq!(stale_lines(&log), 2, "{log}");
    assert_eq!(basis(&c), Some(Basis::Substituted), "{log}");
    assert!(advisory(&e).is_some(), "still the ALERT, not a stay: {log}");
    assert_eq!(swaps(&log), 0, "{log}");
}

lane!(lff);

#[test]
fn a_frozen_66_with_a_fixed_cache_time_whose_reread_fails_alerts_and_is_read_again() {
    let e = env();
    seed(&lff::CLOCK);
    // Observed once, at cycle 1's clock: the reading only ages.
    set(
        &lff::LIVE,
        &[
            Live::Passive(66.0, 41.0, 0),
            Live::Passive(66.0, 41.0, INTERVAL),
            Live::Passive(66.0, 41.0, 2 * INTERVAL),
            Live::Passive(66.0, 41.0, 3 * INTERVAL),
        ],
    );
    set(&lff::OTHERS, &spare_full());
    let mut c = ctx(&e, lff::clock, lff::measure, Some(lff::reread));
    assert_eq!(step(&mut c, &lff::CLOCK), CycleOutcome::Stayed);
    assert_eq!(step(&mut c, &lff::CLOCK), CycleOutcome::Stayed);
    assert_eq!(lff::live_calls(), 0);
    step(&mut c, &lff::CLOCK);
    let log = log_of(&e);
    assert_eq!(lff::live_calls(), 1, "{log}");
    assert_eq!(basis(&c), Some(Basis::Substituted), "{log}");
    assert!(
        log.contains("ALERT: no profile has capacity live=work s=66% w=41%"),
        "{log}"
    );
    assert!(advisory(&e).is_some(), "{log}");
    step(&mut c, &lff::CLOCK);
    let log = log_of(&e);
    assert_eq!(lff::live_calls(), 2, "{log}");
    assert_eq!(basis(&c), Some(Basis::Substituted), "{log}");
    assert!(advisory(&e).is_some(), "still the ALERT, not a stay: {log}");
    assert_eq!(swaps(&log), 0, "{log}");
}
