//! Issue #3458: the live login's re-read, and a swap never decided with the
//! live side unread.
//!
//! The real `run_cycle` runs on a temporary profile root with the #2211
//! stand-in usage source (`lane!`). The live re-read (`Ctx::reread_live`) is
//! a stand-in of its own, separate from the candidate re-read
//! (`Ctx::reread`), so one can fail while the other succeeds. Time is
//! synthetic. The tests assert on outcomes and on the fixed log text in
//! `fresh_msg`; they pin no literal wording and no time.

use super::super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::super::fresh_msg;
use super::super::tests_issue_1870::rd;
use super::super::tests_revalidate_cause::{ctx_with, log_of};
use super::super::tests_swap::{env, ino_at};
use super::tests_issue_2211_cycle::Live;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use std::sync::atomic::Ordering;
use std::time::Duration;

const INTERVAL: i64 = 120;
/// Seconds a successful live read takes on the synthetic clock.
const READ_SECS: i64 = 4;

/// A reading as the stand-in lanes expect it; no reset time is supplied.
fn in_window(p: super::super::decide::Probed, _resets: i64) -> super::super::decide::Probed {
    p
}

fn work() -> ProfileName {
    ProfileName::parse("work").unwrap()
}

lane!(failing);
lane!(finishing);

/// The live re-read that always fails.
fn live_fails(
    _: &Ctx,
    _: &ProfileName,
    _: Duration,
) -> (
    super::super::decide::Probed,
    Option<super::super::measure::ProbeLabel>,
) {
    (super::super::decide::Probed::failed(), None)
}

/// The live re-read that succeeds, dated when it finishes: after the clock
/// the cycle started with.
fn live_finishes_late(
    _: &Ctx,
    _: &ProfileName,
    _: Duration,
) -> (
    super::super::decide::Probed,
    Option<super::super::measure::ProbeLabel>,
) {
    let done = finishing::CLOCK.fetch_add(READ_SECS, Ordering::SeqCst) + READ_SECS;
    (rd(10.0, 14.0, Some(done)), None)
}

fn swapped_lines(log: &str) -> usize {
    log.lines()
        .filter(|l| {
            l.split_once(' ')
                .is_some_and(|(_, rest)| rest.starts_with("swap "))
        })
        .count()
}

fn cycle_with(
    clock: fn() -> i64,
    measure: super::super::cycle::MeasureFn,
    confirm: super::super::fresh::RereadFn,
    live: super::super::fresh::RereadFn,
) -> (super::super::tests_swap::Env, Ctx) {
    let e = env();
    let mut c = ctx_with(&e, measure);
    c.reread = Some(confirm);
    c.reread_live = Some(live);
    c.now = clock;
    c.settings.interval = INTERVAL as u64;
    c.settings.session_at = 70;
    c.settings.margin = 5;
    (e, c)
}

/// The cached live reading is old and over the swap point, `spare` has room.
fn seed_stale_live(lane_clock: &std::sync::atomic::AtomicI64) {
    lane_clock.store(now(), Ordering::SeqCst);
}

#[test]
fn a_failed_live_reread_keeps_the_live_login_and_says_so() {
    seed_stale_live(&failing::CLOCK);
    *failing::LIVE.lock().unwrap() = vec![Live::Passive(95.0, 14.0, 3 * INTERVAL)];
    *failing::OTHERS.lock().unwrap() = vec![("spare", rd(0.0, 0.0, None))];
    let (e, mut c) = cycle_with(
        failing::clock,
        failing::measure,
        failing::confirm,
        live_fails,
    );
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    let kept = fresh_msg::kept_live(&work());
    assert_ne!(
        got,
        CycleOutcome::Swapped,
        "swapped on an unread live side: {log}"
    );
    assert_eq!(swapped_lines(&log), 0, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
    assert!(log.contains(&kept), "the keep-live line is missing: {log}");
}

#[test]
fn a_live_reread_that_finishes_after_the_cycle_clock_is_used() {
    seed_stale_live(&finishing::CLOCK);
    *finishing::LIVE.lock().unwrap() = vec![Live::Passive(95.0, 14.0, 3 * INTERVAL)];
    *finishing::OTHERS.lock().unwrap() = vec![("spare", rd(0.0, 0.0, None))];
    let (e, mut c) = cycle_with(
        finishing::clock,
        finishing::measure,
        finishing::confirm,
        live_finishes_late,
    );
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    let failed = fresh_msg::failed(&work());
    let kept = fresh_msg::kept_live(&work());
    assert!(
        !log.contains(&failed),
        "the finished read was discarded: {log}"
    );
    assert!(!log.contains(&kept), "{log}");
    assert_ne!(
        got,
        CycleOutcome::Swapped,
        "decided on the stale 95%: {log}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
}
