//! Issue #1870: the freshness gate and swap-back damping in a real cycle.
//!
//! The swap, journal, ledger and log are the real ones, under a temporary
//! HOME and profile root (`tests_swap::env()`: `work` live, `spare` stored,
//! plus the stored profiles a test adds). Probing is stubbed with
//! `Ctx::measure`, which calls the real decision (`decide_measured`); the
//! cycle applies the gate to the stub's measurement as it does to a real
//! one. The re-read is stubbed with `Ctx::reread`, which records each call,
//! so no test reads a login, starts `claude`, uses the network or a tmux
//! server, or inherits the developer's profile root.
//!
//! Each test has its own statics, since tests run on parallel threads.
//!
//! Lettered tests follow requirement 7 of the issue's final requirements.

use super::cycle::{Ctx, CycleOutcome, MeasureFn, Mode, run_cycle};
use super::decide::{Decision, Probed, Reason};
use super::fresh::RereadFn;
use super::log::msg;
use super::observe::{Measurement, Snap, decide_measured};
use super::tests_issue_1870::{exh, rd};
use super::tests_revalidate_cause::{add_stored, ctx_with, log_of, probes_for, spent};
use super::tests_swap::{Env, env, ino_at, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Liveness, now};
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

type Calls = Mutex<Vec<String>>;

fn record(calls: &Calls, name: &ProfileName) {
    calls.lock().unwrap().push(name.to_string());
}

fn calls(c: &Calls) -> Vec<String> {
    c.lock().unwrap().clone()
}

fn ctx(e: &Env, measure: MeasureFn, reread: Option<RereadFn>) -> Ctx {
    let mut c = ctx_with(e, measure);
    c.reread = reread;
    c
}

fn swaps(log: &str) -> usize {
    log.lines().filter(|l| l.contains(" swap ")).count()
}

fn damped(e: &Env) -> Vec<String> {
    e.rs.load_ledger()
        .unwrap()
        .map(|l| l.swapped_away.keys().cloned().collect())
        .unwrap_or_default()
}

// --- (a) The regression: an old s=11% w=95% reading reads again at 100% ----

static CALLS_A: Calls = Mutex::new(Vec::new());

fn reread_a(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    record(&CALLS_A, name);
    exh(43.0, 100.0)
}

/// The 13:09Z cycle: `work` live past `--week-at`; `spare` has a passive
/// reading from a minute ago; `spare2` is still being read.
fn measure_a(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let mut running = Probed::failed();
    running.still_running = true;
    let probes = probes_for(
        snap,
        &[
            ("spare", rd(11.0, 95.0, Some(at - 60))),
            ("spare2", running),
        ],
    );
    decide_measured(ctx, snap, rd(0.0, 97.0, None), probes, at)
}

#[test]
fn a_regression_an_old_reading_is_read_again_and_the_full_login_is_not_chosen() {
    let e = env();
    add_stored(&e, "spare2");
    let mut c = ctx(&e, measure_a, Some(reread_a));
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    // Issue #1907: `work` reads 0/97, below 100%, so the no-capacity ALERT
    // is advisory and the cycle is a stay with capacity.
    assert_eq!(got, CycleOutcome::Stayed, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t), "spare untouched");
    assert_eq!(swaps(&log), 0, "{log}");
    assert_eq!(calls(&CALLS_A), ["spare"]);
    assert!(
        log.contains("reread: spare reading stale s=11% w=95%; fresh s=43% w=100%"),
        "{log}"
    );
    assert!(
        log.contains("ALERT: no profile has capacity live=work s=0% w=97% (candidates: 1 failed, 1 exhausted)"),
        "{log}"
    );
    assert!(
        log.find("reread: spare") > log.find(" cycle live=work"),
        "the reread line follows the cycle line:\n{log}"
    );
}

// --- (b) A stale target is read again and the decision uses the new figures --

static CALLS_B: Calls = Mutex::new(Vec::new());

fn reread_b(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    record(&CALLS_B, name);
    rd(0.0, 50.0, None)
}

/// `work` spent; `spare` looks best on an old reading; `third`, read now,
/// beats `spare`'s fresh figures.
fn measure_b(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(
        snap,
        &[
            ("spare", rd(0.0, 10.0, Some(at - 30))),
            ("third", rd(0.0, 30.0, None)),
        ],
    );
    let m = decide_measured(ctx, snap, spent(), probes, at)?;
    assert!(
        matches!(&m.decision, Decision::Swap { to, .. } if to == &n("spare")),
        "before the gate spare is chosen: {:?}",
        m.decision
    );
    Ok(m)
}

#[test]
fn b_a_stale_target_is_read_again_and_the_decision_uses_the_fresh_figures() {
    let e = env();
    let third = add_stored(&e, "third");
    let mut c = ctx(&e, measure_b, Some(reread_b));
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(third), "third's login is live");
    assert_eq!(calls(&CALLS_B), ["spare"], "only the stale target is read");
    assert!(
        log.contains("reread: spare reading stale s=0% w=10%; fresh s=0% w=50%"),
        "{log}"
    );
    assert!(log.contains("swap work->third "), "{log}");
    assert!(
        log.find("reread: spare") < log.find("swap work->third "),
        "{log}"
    );
}

// --- (c) A fresh target is chosen without a re-read --------------------------

static CALLS_C: Calls = Mutex::new(Vec::new());

fn reread_c(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    record(&CALLS_C, name);
    exh(100.0, 100.0)
}

fn measure_c(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(10.0, 20.0, None))]);
    decide_measured(ctx, snap, spent(), probes, at)
}

#[test]
fn c_a_fresh_target_is_chosen_without_a_reread() {
    let e = env();
    let mut c = ctx(&e, measure_c, Some(reread_c));
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert!(calls(&CALLS_C).is_empty(), "no re-read: {log}");
    assert!(!log.contains("reread:"), "{log}");
}

static CALLS_C2: Calls = Mutex::new(Vec::new());

fn reread_c2(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    record(&CALLS_C2, name);
    exh(100.0, 100.0)
}

/// A passive reading taken at the same second as the live one is fresh.
fn measure_c2(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(10.0, 20.0, Some(at)))]);
    decide_measured(ctx, snap, spent(), probes, at)
}

#[test]
fn c_a_passive_reading_as_new_as_the_live_one_is_fresh() {
    let e = env();
    let mut c = ctx(&e, measure_c2, Some(reread_c2));
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert!(calls(&CALLS_C2).is_empty(), "{log}");
}

// --- (d) A failed re-read: not chosen, not recorded as exhausted ---------------

fn reread_d(_: &Ctx, _: &ProfileName, _: Duration) -> Probed {
    Probed::failed()
}

fn measure_d(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(11.0, 95.0, Some(at - 60)))]);
    decide_measured(ctx, snap, rd(0.0, 97.0, None), probes, at)
}

#[test]
fn d_a_failed_reread_is_unknown_and_never_recorded_as_without_room() {
    let e = env();
    let mut c = ctx(&e, measure_d, Some(reread_d));
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    // Issue #1907: `work` reads 0/97, below 100%, so the no-capacity ALERT
    // is advisory and the cycle is a stay with capacity.
    assert_eq!(got, CycleOutcome::Stayed, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "no swap");
    assert!(
        log.contains("reread: spare failed; treated as unknown"),
        "{log}"
    );
    assert!(log.contains("(candidates: 1 failed, 0 exhausted)"), "{log}");
    let l = e.rs.load_ledger().unwrap().unwrap();
    let no_room: Vec<String> = l.no_room.map(|r| r.profiles).unwrap_or_default();
    assert!(
        !no_room.contains(&"spare".to_string()),
        "an unknown login is not without room: {no_room:?}"
    );
    assert!(
        l.swapped_away.is_empty(),
        "nothing is damped without a swap"
    );
}

// --- (e) A plan-room target is gated (the 2 Nov 19:52Z instance) ---------------

static CALLS_E: Calls = Mutex::new(Vec::new());

fn reread_e(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    record(&CALLS_E, name);
    exh(43.0, 100.0)
}

fn measure_e(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(11.0, 93.0, Some(at - 90)))]);
    // `work` is past --week-at; spare's gain (4) is under --margin (5) and
    // its week is lower, so the decision is a plan-room move.
    let m = decide_measured(ctx, snap, rd(0.0, 97.0, None), probes, at)?;
    assert_eq!(
        m.decision,
        Decision::Swap {
            to: n("spare"),
            reason: Reason::PlanRoom
        },
        "the fixture must reach a plan-room swap before the gate"
    );
    Ok(m)
}

#[test]
fn e_a_plan_room_target_is_read_again_before_the_move() {
    let e = env();
    let mut c = ctx(&e, measure_e, Some(reread_e));
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(calls(&CALLS_E), ["spare"]);
    assert!(
        log.contains("reread: spare reading stale s=11% w=93%; fresh s=43% w=100%"),
        "{log}"
    );
    assert!(!log.contains("reason=plan-room"), "{log}");
}

// --- (f) Two-login replay: one swap, no swap back on a pre-swap reading ------

static CYCLE1_F: AtomicI64 = AtomicI64::new(0);
static CALLS_F: Calls = Mutex::new(Vec::new());

fn reread_f(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    record(&CALLS_F, name);
    exh(100.0, 22.0)
}

/// Cycle 1: `work` past --session-at, `spare` with room.
fn cycle1_f(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    CYCLE1_F.store(at, Ordering::SeqCst);
    let probes = probes_for(snap, &[("spare", rd(50.0, 30.0, None))]);
    decide_measured(ctx, snap, rd(86.0, 20.0, None), probes, at)
}

/// Cycle 2: `spare` is now live and full; `work`'s stored reading is the
/// one taken before the swap, on which it looks better than `spare`.
fn cycle2_f(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let before_swap = CYCLE1_F.load(Ordering::SeqCst);
    let probes = probes_for(snap, &[("work", rd(86.0, 20.0, Some(before_swap)))]);
    let m = decide_measured(ctx, snap, rd(95.0, 35.0, None), probes, at)?;
    assert!(
        matches!(&m.decision, Decision::Swap { to, .. } if to == &n("work")),
        "without the gate this cycle swaps back: {:?}",
        m.decision
    );
    Ok(m)
}

#[test]
fn f_two_login_replay_swaps_once_and_never_back_on_a_pre_swap_reading() {
    let e = env();
    let mut c = ctx(&e, cycle1_f, Some(reread_f));
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped
    );
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare is live");
    assert_eq!(damped(&e), ["work"], "work was left full, so it is damped");
    assert!(calls(&CALLS_F).is_empty(), "spare was fresh");

    c.measure = Some(cycle2_f);
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(swaps(&log), 1, "at most one swap:\n{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare stays live");
    assert_eq!(calls(&CALLS_F), ["work"]);
    assert!(
        log.contains("damp: work swapped away exhausted; no later reading shows room"),
        "{log}"
    );
    assert_eq!(damped(&e), ["work"], "still damped");
}

// --- (g) Damping is lifted by a later reading that shows room ------------------

static CYCLE1_G: AtomicI64 = AtomicI64::new(0);

fn cycle1_g(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    CYCLE1_G.store(at, Ordering::SeqCst);
    let probes = probes_for(snap, &[("spare", rd(50.0, 30.0, None))]);
    decide_measured(ctx, snap, rd(86.0, 20.0, None), probes, at)
}

/// `work`, read in this cycle, has room again.
fn cycle2_g(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("work", rd(10.0, 20.0, None))]);
    decide_measured(ctx, snap, rd(95.0, 35.0, None), probes, at)
}

fn backdate(e: &Env, name: &str, at: i64) {
    let mut l = e.rs.load_ledger().unwrap().unwrap();
    l.swapped_away.insert(name.into(), at);
    e.rs.save_ledger(&l).unwrap();
}

#[test]
fn g_a_reading_after_the_swap_that_shows_room_lifts_the_damping() {
    let e = env();
    let mut c = ctx(&e, cycle1_g, None);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped
    );
    assert_eq!(damped(&e), ["work"]);
    // The swap away was ten minutes ago, so this cycle's reading is later.
    backdate(&e, "work", now() - 600);

    c.measure = Some(cycle2_g);
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "work is live again");
    let lifted = "damp: work later reading shows room; swap-back allowed";
    assert!(log.contains(lifted), "{log}");
    assert!(
        log.find("swap spare->work ") < log.find(lifted),
        "the lifted line is written in the record step, after the swap:\n{log}"
    );
    assert!(
        !damped(&e).contains(&"work".to_string()),
        "work's entry removed"
    );
    assert_eq!(damped(&e), ["spare"], "spare was left full in turn");
}

static CALLS_G2: Calls = Mutex::new(Vec::new());

fn reread_g2(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    record(&CALLS_G2, name);
    rd(10.0, 20.0, None)
}

/// `work`'s reading is later than the swap away but over 300 s old.
fn cycle2_g2(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("work", rd(10.0, 20.0, Some(at - 400)))]);
    decide_measured(ctx, snap, rd(95.0, 35.0, None), probes, at)
}

#[test]
fn g_a_reread_that_shows_room_lifts_the_damping() {
    let e = env();
    let mut c = ctx(&e, cycle1_g, Some(reread_g2));
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped
    );
    backdate(&e, "work", now() - 600);

    c.measure = Some(cycle2_g2);
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(calls(&CALLS_G2), ["work"]);
    assert!(
        log.contains("damp: work later reading shows room; swap-back allowed"),
        "{log}"
    );
    assert!(!damped(&e).contains(&"work".to_string()));
}

// --- (h) Damping survives a reload between one-shot runs ------------------------

static CYCLE1_H: AtomicI64 = AtomicI64::new(0);
static CALLS_H: Calls = Mutex::new(Vec::new());

fn reread_h(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    record(&CALLS_H, name);
    exh(100.0, 22.0)
}

fn cycle1_h(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    CYCLE1_H.store(at, Ordering::SeqCst);
    let probes = probes_for(snap, &[("spare", rd(50.0, 30.0, None))]);
    decide_measured(ctx, snap, rd(86.0, 20.0, None), probes, at)
}

fn cycle2_h(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let before_swap = CYCLE1_H.load(Ordering::SeqCst);
    let probes = probes_for(snap, &[("work", rd(86.0, 20.0, Some(before_swap)))]);
    decide_measured(ctx, snap, rd(95.0, 35.0, None), probes, at)
}

fn only_names_and_integers(raw: &str) {
    let v: serde_json::Value = serde_json::from_str(raw).unwrap();
    let map = v["swapped_away"]
        .as_object()
        .expect("swapped_away is an object");
    assert!(!map.is_empty());
    for (k, at) in map {
        assert!(ProfileName::parse(k).is_ok(), "key {k:?} is a profile name");
        assert!(at.is_i64(), "{k}: {at} is whole epoch seconds");
    }
    // Only the new field is checked: the ledger's `account` predates #1870.
    let field = serde_json::to_string(map).unwrap();
    for leak in ["@", "u-work", "u-spare", "SENTINEL", "SECRET"] {
        assert!(!field.contains(leak), "swapped_away holds `{leak}`: {raw}");
    }
}

#[test]
fn h_damping_survives_between_one_shot_runs_and_holds_only_names_and_integers() {
    let e = env();
    let mut first = ctx(&e, cycle1_h, Some(reread_h));
    assert_eq!(
        run_cycle(&mut first, Mode::OneShot).unwrap(),
        CycleOutcome::Swapped
    );
    drop(first);
    let raw = fs::read_to_string(&e.rs.ledger).unwrap();
    only_names_and_integers(&raw);

    // A new process: nothing carried in memory.
    let mut second = ctx(&e, cycle2_h, Some(reread_h));
    let got = run_cycle(&mut second, Mode::OneShot).unwrap();
    let log = log_of(&e);
    assert_ne!(got, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare stays live");
    assert_eq!(calls(&CALLS_H), ["work"], "the damped login was read again");
    assert_eq!(damped(&e), ["work"]);
}

// --- (i) The re-read limit -------------------------------------------------------

static CALLS_I: Calls = Mutex::new(Vec::new());

fn reread_i(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    record(&CALLS_I, name);
    exh(100.0, 100.0)
}

/// Four candidates, each best on a stale reading and full when read again.
fn measure_i(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let old = Some(at - 60);
    let probes = probes_for(
        snap,
        &[
            ("spare", rd(0.0, 10.0, old)),
            ("spare2", rd(0.0, 20.0, old)),
            ("third", rd(0.0, 30.0, old)),
            ("fourth", rd(0.0, 40.0, old)),
        ],
    );
    decide_measured(ctx, snap, spent(), probes, at)
}

#[test]
fn i_a_cycle_makes_at_most_three_rereads_and_each_name_once() {
    let e = env();
    for p in ["spare2", "third", "fourth"] {
        add_stored(&e, p);
    }
    let mut c = ctx(&e, measure_i, Some(reread_i));
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Alerted, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "no swap");
    assert_eq!(calls(&CALLS_I), ["spare", "spare2", "third"]);
    assert!(
        log.contains("reread: fourth limit reached; treated as unknown"),
        "{log}"
    );
    assert!(!log.contains("swap work->"), "{log}");
}

// --- SR-3: revalidation under the lock never reads -------------------------------

static SPARE_BUSY: AtomicBool = AtomicBool::new(false);

fn scan_busy(_: &Path) -> Liveness {
    let mut live = BTreeSet::new();
    if SPARE_BUSY.load(Ordering::SeqCst) {
        live.insert("spare".to_string());
    }
    Liveness::Known(live)
}

fn never(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    panic!("SR-3: {name} was read while the lock was held")
}

/// `spare`, read now, is chosen; it then gets a session, so revalidation
/// reconsiders. The next best, `spare2`, has only an old passive reading.
fn measure_sr3(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(
        snap,
        &[
            ("spare", rd(0.0, 10.0, None)),
            ("spare2", rd(0.0, 20.0, Some(at - 60))),
        ],
    );
    SPARE_BUSY.store(true, Ordering::SeqCst);
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn sr3_revalidation_treats_a_stale_retry_target_as_unknown_without_reading() {
    let e = env();
    add_stored(&e, "spare2");
    let mut c = ctx(&e, measure_sr3, Some(never));
    c.scan = scan_busy;
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Skipped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "nothing moved");
    assert!(!log.contains("reread:"), "{log}");
    assert!(!log.contains("damp:"), "{log}");
    assert!(log.contains(msg::CHANGED), "{log}");
}

// --- SR-9: --dry-run reads again but leaves the ledger as it was ---------------

static CALLS_DRY: Calls = Mutex::new(Vec::new());

fn reread_dry(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    record(&CALLS_DRY, name);
    rd(11.0, 20.0, None)
}

/// `work` past --week-at; `spare`, damped two minutes ago, has a passive
/// reading older than the live one.
fn measure_dry(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(11.0, 95.0, Some(at - 60)))]);
    decide_measured(ctx, snap, rd(0.0, 97.0, None), probes, at)
}

#[test]
fn sr9_a_dry_run_reads_again_and_leaves_the_ledger_bytes_unchanged() {
    let e = env();
    let at = now();
    let mut l = super::ledger::RotateLedger::default();
    l.swapped_away.insert("spare".into(), at - 120);
    l.swapped_away.insert("third".into(), at - 30);
    e.rs.save_ledger(&l).unwrap();
    let before = fs::read(&e.rs.ledger).unwrap();
    let mut c = ctx(&e, measure_dry, Some(reread_dry));
    let got = run_cycle(&mut c, Mode::DryRun).unwrap();
    assert_eq!(calls(&CALLS_DRY), ["spare"], "the dry run reads again");
    assert_eq!(
        got,
        CycleOutcome::Swapped,
        "the fresh reading shows room, so a real cycle would swap"
    );
    assert_eq!(
        fs::read(&e.rs.ledger).unwrap(),
        before,
        "SR-9: no damping is lifted, recorded or pruned"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
}

// --- A stop request before a re-read: no read, no swap -----------------------

static CALLS_STOP: Calls = Mutex::new(Vec::new());

/// A slow login: the read would take a probe timeout.
fn reread_stop(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    record(&CALLS_STOP, name);
    std::thread::sleep(std::time::Duration::from_secs(1));
    rd(0.0, 50.0, None)
}

/// As (b), and `rotate stop` arrives while the cycle measures.
fn measure_stop(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    if let Some(stop) = &ctx.stop {
        stop.store(true, Ordering::SeqCst);
    }
    measure_b(ctx, snap)
}

#[test]
fn a_stop_request_before_a_reread_makes_no_read_and_no_swap() {
    let e = env();
    add_stored(&e, "third");
    let mut c = ctx(&e, measure_stop, Some(reread_stop));
    c.stop = Some(std::sync::Arc::new(AtomicBool::new(false)));
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(got, CycleOutcome::Stopped, "{log}");
    assert!(
        calls(&CALLS_STOP).is_empty(),
        "no re-read runs after a stop"
    );
    assert_eq!(swaps(&log), 0, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
}
