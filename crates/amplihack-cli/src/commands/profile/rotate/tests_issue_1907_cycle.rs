//! Issue #1907: what a rotate cycle records as capacity for the
//! limit-resume hook.
//!
//! The rule: capacity is a fresh, successful reading of the live login below
//! 100% on both windows, or a swap or switch to such a login. A stay on a
//! failed live probe is not capacity, and a no-capacity ALERT while the live
//! login reads below 100% does not block a resume.
//!
//! The swap, ledger and log are the real ones under a temporary HOME and
//! profile root (`tests_swap::env()`: `work` live, `spare` stored). Probing
//! is stubbed with `Ctx::measure`, which calls the real decision
//! (`decide_measured`), so failed-probe damping runs as in production. No
//! test reads a login, starts `claude`, uses the network or a tmux server,
//! or reads the developer's profile root.
//!
//! The ledger is checked as the hook reads it: its bytes on disk go through
//! `amplihack_utils::rotator_status::parse_cycle`, and the result's
//! `has_capacity` and `fresh_capacity` are what `limit-resume` resumes on.
//!
//! Fail-first (D7): (a), (a2), (a3) and (d) use only APIs and string outcomes that
//! exist on `main`, and fail there. (b), (c), (d2) and (e1) to (e3) are guards
//! and pass on `main`.

use super::super::cycle::{Ctx, CycleOutcome, MeasureFn, Mode, run_cycle};
use super::super::decide::Probed;
use super::super::observe::{Measurement, Snap, decide_measured};
use super::super::tests_issue_1870::{exh, rd};
use super::super::tests_revalidate_cause::{ctx_with, log_of, probes_for};
use super::super::tests_swap::{Env, env, ino_at};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use amplihack_utils::rotator_status::{CycleStatus, parse_cycle};
use std::cell::Cell;
use std::fs;

// --- Helpers ---------------------------------------------------------------

/// The ledger as the hook sees it.
fn hook_view(e: &Env) -> CycleStatus {
    let raw = fs::read(&e.rs.ledger).expect("the cycle wrote a ledger");
    parse_cycle(&raw, now() + 1).expect("the hook parses the ledger")
}

/// The ledger's raw JSON, for fields the hook does not parse.
fn ledger_json(e: &Env) -> serde_json::Value {
    serde_json::from_slice(&fs::read(&e.rs.ledger).unwrap()).unwrap()
}

/// A limit hit just before the cycle: `fresh_capacity` needs a cycle
/// measured strictly later.
fn limit_hit() -> i64 {
    now() - 1
}

fn run(e: &Env, measure: MeasureFn) -> CycleOutcome {
    let mut c = ctx_with(e, measure);
    run_cycle(&mut c, Mode::Watch).unwrap()
}

/// Sets a stored ALERT key, as an earlier alerted cycle leaves it.
fn seed_alert(e: &Env, key: &str) {
    let mut l = e.rs.load_ledger().unwrap().unwrap_or_default();
    l.alert = Some(key.to_string());
    e.rs.save_ledger(&l).unwrap();
}

fn count(log: &str, needle: &str) -> usize {
    log.lines().filter(|l| l.contains(needle)).count()
}

const NO_CAPACITY_LINE: &str = "ALERT: no profile has capacity live=work";

// --- (a) A stay on a failed live probe is not capacity ---------------------

/// The live probe fails and `spare` has room: the decision is a threshold
/// swap, which damping turns into `stay: live probe failed (1/3)`.
fn measure_failed_live(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(10.0, 10.0, None))]);
    decide_measured(ctx, snap, Probed::failed(), probes, at)
}

#[test]
fn a_a_stay_on_a_failed_live_probe_is_not_capacity() {
    let e = env();
    let start = limit_hit();
    let got = run(&e, measure_failed_live);
    let log = log_of(&e);
    assert!(
        log.contains("stay: live probe failed (1/3)"),
        "the setup must be a damped stay:\n{log}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
    assert_ne!(
        got,
        CycleOutcome::Stayed,
        "a failed probe is not a stay with capacity"
    );
    let v = hook_view(&e);
    assert_eq!(v.outcome.as_deref(), Some("stayed-unread"), "{v:?}");
    assert!(!v.has_capacity(), "{v:?}");
    assert!(!v.fresh_capacity(start), "{v:?}");
}

/// A stayed-unread cycle leaves a stored ALERT as it was: it knows nothing
/// about the live login that would clear it.
#[test]
fn a2_a_failed_probe_stay_leaves_a_stored_alert_in_place() {
    let e = env();
    seed_alert(&e, "no-capacity:work");
    run(&e, measure_failed_live);
    let v = hook_view(&e);
    assert!(v.alert, "{v:?}");
    assert_eq!(ledger_json(&e)["alert"], "no-capacity:work");
    assert!(!v.has_capacity());
}

// --- (a3) A stay on a carried result is not capacity -----------------------

/// The live result was carried from an earlier cycle: it has figures, but no
/// reading was taken in this cycle. `spare` reads the same, so no upgrade.
fn measure_carried_live(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let mut live = rd(40.0, 30.0, None);
    live.carried = true;
    let probes = probes_for(snap, &[("spare", rd(40.0, 30.0, None))]);
    decide_measured(ctx, snap, live, probes, at)
}

#[test]
fn a3_a_stay_on_a_carried_live_result_is_not_capacity() {
    let e = env();
    let start = limit_hit();
    run(&e, measure_carried_live);
    let v = hook_view(&e);
    assert_eq!(v.outcome.as_deref(), Some("stayed-unread"), "{v:?}");
    assert!(!v.fresh_capacity(start), "{v:?}");
}

// --- (b) A fresh live reading below the thresholds is capacity -------------

/// `spare` reads the same as `work`, so no upgrade applies.
fn measure_fresh_stay(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(40.0, 30.0, None))]);
    decide_measured(ctx, snap, rd(40.0, 30.0, None), probes, at)
}

#[test]
fn b_a_fresh_stay_below_the_thresholds_is_capacity() {
    let e = env();
    let start = limit_hit();
    let got = run(&e, measure_fresh_stay);
    assert_eq!(got, CycleOutcome::Stayed, "{}", log_of(&e));
    let v = hook_view(&e);
    assert_eq!(v.outcome.as_deref(), Some("stayed"));
    assert!(!v.alert);
    assert!(v.has_capacity());
    assert!(v.fresh_capacity(start));
    assert!(
        ledger_json(&e)
            .get("advisory_alert")
            .is_none_or(|a| a.is_null()),
        "a plain stay stores no advisory alert"
    );
}

// --- (c) A successful cycle after (a) restores capacity --------------------

#[test]
fn c_a_fresh_cycle_after_a_failed_probe_restores_capacity_and_clears_the_alert() {
    let e = env();
    seed_alert(&e, "no-capacity:work");
    run(&e, measure_failed_live);
    let start = limit_hit();
    let got = run(&e, measure_fresh_stay);
    assert_eq!(got, CycleOutcome::Stayed, "{}", log_of(&e));
    let v = hook_view(&e);
    assert!(!v.alert, "a fresh stay clears the stored ALERT: {v:?}");
    assert_eq!(v.outcome.as_deref(), Some("stayed"));
    assert!(v.fresh_capacity(start), "{v:?}");
}

// --- (d) A no-capacity ALERT with the live login below 100% ----------------

/// `work` is past `--session-at` (85) but below 100% on both windows; the
/// only candidate is full, so the decision is the no-capacity ALERT.
fn measure_live_past_threshold(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", exh(100.0, 100.0))]);
    decide_measured(ctx, snap, rd(92.0, 60.0, None), probes, at)
}

#[test]
fn d_a_threshold_alert_with_the_live_login_below_100_is_capacity() {
    let e = env();
    let start = limit_hit();
    run(&e, measure_live_past_threshold);
    run(&e, measure_live_past_threshold);
    let log = log_of(&e);
    assert_eq!(
        count(&log, NO_CAPACITY_LINE),
        1,
        "the ALERT line is logged once across both cycles:\n{log}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l), "no swap onto a full login");
    let v = hook_view(&e);
    assert!(!v.alert, "an advisory ALERT does not block a resume: {v:?}");
    assert_eq!(v.outcome.as_deref(), Some("stayed"), "{v:?}");
    assert!(v.fresh_capacity(start), "{v:?}");
    assert_eq!(
        ledger_json(&e)["advisory_alert"],
        "no-capacity:work",
        "the de-duplication key is kept"
    );
}

/// The advisory key moves back to `alert` once the live login reaches 100%,
/// and the hook stops resuming. A guard: it passes on `main`, where both
/// cycles alert.
#[test]
fn d2_an_advisory_alert_becomes_hard_when_the_live_login_fills() {
    let e = env();
    run(&e, measure_live_past_threshold);
    run(&e, measure_exhausted_live);
    let v = hook_view(&e);
    assert!(v.alert, "{v:?}");
    assert_eq!(v.outcome.as_deref(), Some("alerted"));
    assert!(
        ledger_json(&e)
            .get("advisory_alert")
            .is_none_or(|a| a.is_null()),
        "a hard ALERT clears the advisory key"
    );
    let log = log_of(&e);
    assert_eq!(
        count(&log, NO_CAPACITY_LINE),
        1,
        "the same key moving from advisory to hard is not logged again:\n{log}"
    );
}

// --- (e) No capacity anywhere: the ALERT blocks ----------------------------

fn measure_exhausted_live(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", exh(100.0, 100.0))]);
    decide_measured(ctx, snap, exh(100.0, 100.0), probes, at)
}

#[test]
fn e1_an_exhausted_live_login_with_no_room_elsewhere_blocks() {
    let e = env();
    let start = limit_hit();
    let got = run(&e, measure_exhausted_live);
    assert_eq!(got, CycleOutcome::Alerted, "{}", log_of(&e));
    let v = hook_view(&e);
    assert!(v.alert);
    assert_eq!(v.outcome.as_deref(), Some("alerted"));
    assert!(!v.has_capacity());
    assert!(!v.fresh_capacity(start));
}

/// A plan login at its spend limit, with no candidate that has room.
fn measure_spend_limited_live(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let mut live = Probed::failed();
    live.unknown = false;
    live.spend_limit = true;
    let probes = probes_for(snap, &[("spare", exh(100.0, 100.0))]);
    decide_measured(ctx, snap, live, probes, at)
}

#[test]
fn e2_a_spend_limited_live_login_with_no_room_elsewhere_blocks() {
    let e = env();
    let start = limit_hit();
    let got = run(&e, measure_spend_limited_live);
    assert_eq!(got, CycleOutcome::Alerted, "{}", log_of(&e));
    let v = hook_view(&e);
    assert!(v.alert);
    assert!(!v.fresh_capacity(start));
}

/// A live reading at exactly 100% on one window is not below 100%.
fn measure_live_at_100(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", exh(100.0, 100.0))]);
    decide_measured(ctx, snap, rd(50.0, 100.0, None), probes, at)
}

#[test]
fn e3_a_live_reading_at_exactly_100_keeps_the_alert_hard() {
    let e = env();
    run(&e, measure_live_at_100);
    let v = hook_view(&e);
    assert!(v.alert, "{v:?}");
    assert!(!v.has_capacity());
}

// --- (f) A passive reading taken before the limit is not fresh capacity ---

/// Seconds between the passive reading and the cycle in (f): the reading is
/// taken first, the limit is hit next, and the cycle runs after both.
const READING_AGE: i64 = 30;

thread_local! {
    /// The time `measure_old_passive_stay` measured at, for this test's
    /// thread. `MeasureFn` is a plain `fn`, so the stub publishes its own
    /// timestamp here and the test derives its limits from that same value
    /// instead of sampling the clock a second time.
    static MEASURED_AT: Cell<Option<i64>> = const { Cell::new(None) };
}

/// The time the stub measured at in this test's `run()`. Consumes the slot,
/// so a test never reads a value left by another test on the same thread,
/// and panics if the stub did not run.
fn measured_at() -> i64 {
    MEASURED_AT
        .with(Cell::take)
        .expect("measure_old_passive_stay ran on this thread")
}

/// `work`'s passive reading below 100% was taken `READING_AGE` seconds
/// before the cycle measured; `spare` reads the same, so the cycle stays.
/// The measurement time is shared with the test through `MEASURED_AT`.
fn measure_old_passive_stay(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    MEASURED_AT.with(|m| m.set(Some(at)));
    let read = Some(at - READING_AGE);
    let probes = probes_for(snap, &[("spare", rd(40.0, 30.0, read))]);
    decide_measured(ctx, snap, rd(40.0, 30.0, read), probes, at)
}

/// `observed_at < limit start < measured_at`: the reading shows room, but it
/// was taken before the limit, so it says nothing about the login after it.
/// The cycle records the time of the reading, not the time of the cycle, and
/// the hook does not resume a session stopped after the reading. The reading
/// time and both limits come from the stub's own measurement time, so the
/// result does not depend on how long the cycle or the test takes.
#[test]
fn f_a_passive_reading_taken_before_the_limit_is_not_fresh_capacity() {
    let e = env();
    let got = run(&e, measure_old_passive_stay);
    assert_eq!(got, CycleOutcome::Stayed, "{}", log_of(&e));
    let read = measured_at() - READING_AGE;
    let v = hook_view(&e);
    assert_eq!(
        v.measured_at,
        Some(read),
        "a stay records when its passive reading was taken: {v:?}"
    );
    assert!(
        !v.fresh_capacity(read + READING_AGE / 2),
        "a reading older than the limit is no capacity after it: {v:?}"
    );
    assert!(
        v.fresh_capacity(read - 1),
        "a limit hit before the reading is answered by it: {v:?}"
    );
}

/// The same stay on a reading taken after the limit is capacity.
#[test]
fn f2_a_passive_reading_taken_after_the_limit_is_fresh_capacity() {
    let e = env();
    run(&e, measure_old_passive_stay);
    let read = measured_at() - READING_AGE;
    let v = hook_view(&e);
    assert_eq!(v.outcome.as_deref(), Some("stayed"), "{v:?}");
    assert!(v.fresh_capacity(read - 1), "{v:?}");
}

// --- (g) A live credit login with an active spend block --------------------

/// `work` is a credit (overflow) profile with an unexpired spend block in
/// the ledger; its reading is below 100% and shows no spend limit this
/// cycle. `spare` is full, so there is nowhere to go.
fn measure_blocked_credit_live(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", exh(100.0, 100.0))]);
    decide_measured(ctx, snap, rd(40.0, 30.0, None), probes, at)
}

fn block_live_credit_profile(e: &Env) {
    use crate::commands::profile::auto_switch::config::{self, ReadMode};
    use crate::commands::profile::overflow::OverflowEntry;
    config::update(&e.root, ReadMode::Editable, |c| {
        c.overflow
            .push(OverflowEntry::new(super::super::tests_swap::n("work"), 1));
        Ok(())
    })
    .unwrap();
    let mut l = e.rs.load_ledger().unwrap().unwrap_or_default();
    let at = now();
    l.overflow_blocked.insert(
        "work".into(),
        super::super::ledger::OverflowBlock {
            at,
            until: at + 86_400,
            reason: "spend_limit".into(),
        },
    );
    e.rs.save_ledger(&l).unwrap();
}

/// A spend block means the live login cannot spend, whatever its plan
/// windows read: the ALERT is hard, never an advisory stay with capacity.
#[test]
fn g_an_active_spend_block_on_the_live_credit_login_keeps_the_alert_hard() {
    let e = env();
    block_live_credit_profile(&e);
    let start = limit_hit();
    let got = run(&e, measure_blocked_credit_live);
    assert_eq!(got, CycleOutcome::Alerted, "{}", log_of(&e));
    let v = hook_view(&e);
    assert!(v.alert, "{v:?}");
    assert_eq!(v.outcome.as_deref(), Some("alerted"), "{v:?}");
    assert!(!v.has_capacity(), "{v:?}");
    assert!(!v.fresh_capacity(start), "{v:?}");
    let j = ledger_json(&e);
    assert!(j["alert"].is_string(), "{j}");
    assert!(
        j.get("advisory_alert").is_none_or(|a| a.is_null()),
        "a blocked login's ALERT is never advisory: {j}"
    );
}
