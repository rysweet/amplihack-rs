//! Issue #3181: a stale lower reading never ranks a login as having room,
//! and a swap the live login needs is never refused for want of identity
//! reads.
//!
//! They run the real `run_cycle` on a temporary
//! profile root, with the #2211 stand-in usage source (`lane!`), the
//! stand-in identity reader and a synthetic clock. Nothing reads a real
//! login or asserts on log wording.
//!
//! Contract beyond the requirements text:
//!
//! - `Budget::spend()` (`#[cfg(test)]`) leaves the budget used up with no
//!   wait; `Budget::waive()` and `Budget::available()` as in the design.
//! - Ledger JSON: `high_water.<name>.session.{used,resets_at}`.
//!
//! The first test reads the ledger as JSON only, so it fails on main by
//! assertion and not by compilation.

use super::super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::super::decide::Probed;
use super::super::tests_issue_1870::rd;
use super::super::tests_revalidate_cause::{ctx_with, log_of};
use super::super::tests_swap::{Env, env, ino_at};
use super::tests_issue_2211_cycle::Live;
use crate::commands::profile::auto_switch::kit::now;
use std::fs;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

const INTERVAL: i64 = 120;
const OPEN: i64 = 7200;
const WEEK_AHEAD: i64 = 3 * 86_400;

fn in_window(mut p: Probed, resets: i64) -> Probed {
    if resets != 0 {
        if let Some(w) = p.windows.session.as_mut() {
            w.resets_at = Some(resets);
        }
        if let Some(w) = p.windows.week.as_mut() {
            w.resets_at = Some(resets + WEEK_AHEAD);
        }
    }
    p
}

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

fn set<T: Clone>(m: &Mutex<Vec<T>>, v: &[T]) {
    *m.lock().unwrap() = v.to_vec();
}

fn seed(clock: &AtomicI64, reset: &AtomicI64) {
    let t = now();
    clock.store(t, Ordering::SeqCst);
    reset.store(t + OPEN, Ordering::SeqCst);
}

fn step(c: &mut Ctx, clock: &AtomicI64) -> CycleOutcome {
    let got = run_cycle(c, Mode::Watch).unwrap();
    clock.fetch_add(INTERVAL, Ordering::SeqCst);
    got
}

fn swaps(log: &str) -> usize {
    log.lines()
        .filter(|l| {
            l.split_once(' ')
                .is_some_and(|(_, rest)| rest.starts_with("swap "))
        })
        .count()
}

fn ledger(e: &Env) -> serde_json::Value {
    let raw = fs::read_to_string(&e.rs.ledger).expect("the cycle wrote a ledger");
    serde_json::from_str(&raw).unwrap()
}

/// The highest session figure the ledger holds for `name`.
fn peak(e: &Env, name: &str) -> Option<f64> {
    ledger(e)["high_water"][name]["session"]["used"].as_f64()
}

// --- Test 1: a stale lower reading does not bring a login back -------------

/// Cycle 1 leaves `work` at 91% for `spare`. In cycle 2 `spare` is itself
/// over the swap point and `work` is reported at `$stale`% in the same
/// window, read after the swap. `work` must not be the target. Each test
/// has its own lane, so tests running at once never share its statics.
macro_rules! stale_return {
    ($s:ident, $stale:expr, $reset_passes:expr) => {{
        let (stale, reset_passes): (f64, bool) = ($stale, $reset_passes);
        let e = env();
        seed(&$s::CLOCK, &$s::RESET);
        let t = now();
        set(
            &$s::LIVE,
            &[Live::Direct(91.0, 14.0), Live::Direct(75.0, 15.0)],
        );
        let read = rd(stale, 11.0, Some(t + 100));
        let r = $s::RESET.load(Ordering::SeqCst);
        set(
            &$s::OTHERS,
            &[
                ("spare", rd(20.0, 10.0, None)),
                ("work", in_window(read.clone(), r)),
            ],
        );
        set(&$s::REREAD, &[in_window(rd(stale, 11.0, None), r)]);
        let mut c = ctx(&e, $s::clock, $s::measure, Some($s::reread));
        assert_eq!(step(&mut c, &$s::CLOCK), CycleOutcome::Swapped, "left work");
        if reset_passes {
            $s::CLOCK.store(r + 10, Ordering::SeqCst);
            // `work` is in a new window now.
            let later = r + 5 * 3600;
            set(
                &$s::OTHERS,
                &[
                    ("spare", rd(20.0, 10.0, None)),
                    ("work", in_window(rd(stale, 11.0, Some(r + 5)), later)),
                ],
            );
            set(&$s::REREAD, &[in_window(rd(stale, 11.0, None), later)]);
            $s::RESET.store(later, Ordering::SeqCst);
        }
        let got = step(&mut c, &$s::CLOCK);
        (e, got)
    }};
}

lane!(s1a);

#[test]
fn a_stale_lower_reading_does_not_bring_a_login_left_at_91_back() {
    let (e, got) = stale_return!(s1a, 40.0, false);
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "swapped back on 40%: {log}");
    assert_eq!(swaps(&log), 1, "only the first swap: {log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login stays live");
    assert!(
        peak(&e, "work").is_some_and(|p| p >= 91.0),
        "the ledger keeps the highest figure: {:?}",
        ledger(&e)
    );
}

lane!(s1b);

#[test]
fn damping_on_the_left_login_stays_in_place() {
    let (e, _) = stale_return!(s1b, 40.0, false);
    assert!(
        ledger(&e)["swapped_away"].get("work").is_some(),
        "swap-back damping lifted by a stale reading: {:?}",
        ledger(&e)
    );
}

lane!(s1c);

#[test]
fn once_the_window_has_reset_a_lower_reading_is_accepted() {
    let (e, got) = stale_return!(s1c, 5.0, true);
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "a new window has room: {log}");
    assert_eq!(swaps(&log), 2, "{log}");
}

// --- Test 2: a needed swap is never refused for the budget -----------------

/// One cycle with `$live` as the live reading, `spare` with room, and the
/// identity budget spent inside the measurement, so it is used up before the
/// swap step. Each test has its own lane. Returns `(env, ctx, outcome)`.
macro_rules! spent_cycle {
    ($lane:ident, $live:expr) => {{
        fn spend_then_measure(
            c: &Ctx,
            snap: &super::super::observe::Snap,
        ) -> Result<super::super::observe::Measurement, crate::commands::profile::ProfileError>
        {
            c.identity_budget.spend();
            $lane::measure(c, snap)
        }
        let e = env();
        seed(&$lane::CLOCK, &$lane::RESET);
        set(&$lane::LIVE, &[$live]);
        set(&$lane::OTHERS, &[("spare", rd(0.0, 0.0, None))]);
        set(&$lane::REREAD, &[rd(0.0, 0.0, None)]);
        let mut c = ctx(&e, $lane::clock, spend_then_measure, Some($lane::reread));
        let got = step(&mut c, &$lane::CLOCK);
        (e, c, got)
    }};
}

lane!(s2a);

#[test]
fn a_swap_at_the_swap_point_still_happens_with_the_budget_spent() {
    let (e, _, got) = spent_cycle!(s2a, Live::Direct(92.0, 14.0));
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "refused for the budget: {log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
}

lane!(s2b);

#[test]
fn an_optional_upgrade_still_respects_the_spent_budget() {
    let (e, _, got) = spent_cycle!(s2b, Live::Direct(60.0, 14.0));
    assert_ne!(got, CycleOutcome::Swapped, "{}", log_of(&e));
    assert_eq!(swaps(&log_of(&e)), 0);
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
}

lane!(s2c);

#[test]
fn the_cycle_after_a_waived_cycle_is_bound_by_the_budget_again() {
    let (_e, c, got) = spent_cycle!(s2c, Live::Direct(92.0, 14.0));
    assert_eq!(got, CycleOutcome::Swapped);
    // The next cycle's start refills the budget and clears the waiver; the
    // waiver does not stay set.
    c.identity_budget.reset();
    assert!(c.identity_budget.available(), "a fresh allowance");
    c.identity_budget.spend();
    assert!(!c.identity_budget.available(), "no waiver left over");
}

lane!(s3);

#[test]
fn a_waived_swap_still_refuses_a_target_whose_identity_cannot_be_read() {
    // Every other refusal still runs: the stand-in identity reader fails, so
    // the swap is refused though it is waived.
    let e = env();
    seed(&s3::CLOCK, &s3::RESET);
    set(&s3::LIVE, &[Live::Direct(92.0, 14.0)]);
    set(&s3::OTHERS, &[("spare", rd(0.0, 0.0, None))]);
    let mut c = ctx(&e, s3::clock, s3::measure, Some(s3::reread));
    c.identity = |_| Err("login identity unavailable");
    let got = step(&mut c, &s3::CLOCK);
    assert_ne!(got, CycleOutcome::Swapped, "{}", log_of(&e));
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
}
