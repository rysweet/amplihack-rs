//! Issue #1870: the freshness gate and swap-back damping, unit level.
//!
//! Contract (`rotate::fresh`, `rotate::swap_back`, `rotate::fresh_msg`):
//! - `fresh::MAX_READING_AGE_SECS: i64 = 300` and
//!   `fresh::MAX_REREADS_PER_CYCLE: usize = 3`.
//! - `fresh::RereadFn = fn(&Ctx, &ProfileName, Duration) -> Probed`, and
//!   `fresh::Reread::{Allowed, Never}`.
//! - `fresh::read_at(&Probed, measured_at) -> Option<i64>`: `None` for a
//!   carried, still-running, unknown or failed probe; else `observed_at`, or
//!   `measured_at` for a probe read in this cycle.
//! - `fresh::stale(read_at, live_read_at, damped_at, now) -> bool`: `None`
//!   is stale; older than 300 s is stale; strictly older than the live
//!   reading is stale (equal is fresh); with a damping time, a reading not
//!   strictly later than it is stale.
//! - `fresh::gated_target(&Decision) -> Option<&ProfileName>`: the target of
//!   a `threshold`, `upgrade`, `plan-room`, `failed-probe` or
//!   `overflow-out` swap; `None` for `overflow-in`, a stay and an ALERT.
//! - `fresh::gate(&Ctx, &Snap, Measurement, Reread) -> Result<Measurement,
//!   ProfileError>`; its log lines go to `Measurement::fresh_lines` and it
//!   writes nothing.
//! - `swap_back::{MAX_ENTRIES = 16, MAX_AGE_SECS = 7 days}`,
//!   `swap_back::valid(BTreeMap<String, i64>, now) -> BTreeMap<String,
//!   i64>`, `swap_back::prune(&mut BTreeMap<String, i64>, now)`,
//!   `swap_back::{full, room}(&Reading, &Settings) -> bool`.
//! - `RotateLedger::swapped_away: BTreeMap<String, i64>`, omitted when
//!   empty, filtered by `swap_back::valid` on load and cleared by
//!   `for_older_readers(.., true)`.
//! - `fresh_msg::{stale, failed, limit, time_limit, still_damped, lifted}`: the six
//!   fixed log lines of D11.
//!
//! No test here runs `claude`, reads a login, or touches the network, a
//! real home or a tmux server: the re-read is always a stub through
//! `Ctx::reread`.

use super::cycle::{Ctx, MeasureFn};
use super::decide::{Decision, Probed, Reading, Reason, Win, Windows};
use super::fresh::{self, MAX_READING_AGE_SECS, MAX_REREADS_PER_CYCLE, Reread, RereadFn};
use super::fresh_msg;
use super::ledger::{RotateLedger, for_older_readers};
use super::log::Shown;
use super::observe::{Measurement, Snap, decide_measured, snapshot};
use super::settings::Settings;
use super::swap_back::{self, MAX_AGE_SECS, MAX_ENTRIES};
use super::tests_revalidate_cause::{ctx_with, spent};
use super::tests_swap::{Env, env, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, now};
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;
use std::fs;
use std::sync::Mutex;
use std::time::Duration;

const T: i64 = 1_793_634_010;

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
    }
}

fn windows(session: f64, week: f64) -> Windows {
    Windows {
        session: Some(Win {
            used: session,
            resets_at: None,
        }),
        week: Some(Win {
            used: week,
            resets_at: None,
        }),
        no_session_limit: false,
    }
}

/// A measured probe with both windows, read at `observed_at` (passive) or
/// in this cycle (`None`).
pub(super) fn rd(session: f64, week: f64, observed_at: Option<i64>) -> Probed {
    Probed {
        reading: Reading::Measured { session, week },
        windows: windows(session, week),
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at,
        carried: false,
        passive: false,
    }
}

/// An exhausted probe that keeps both figures.
pub(super) fn exh(session: f64, week: f64) -> Probed {
    Probed {
        reading: Reading::Exhausted {
            session: Some(session),
            week: Some(week),
        },
        windows: windows(session, week),
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at: None,
        carried: false,
        passive: false,
    }
}

// --- Constants --------------------------------------------------------------

#[test]
fn the_age_limit_is_300_seconds_and_matches_the_passive_and_history_limits() {
    assert_eq!(MAX_READING_AGE_SECS, 300);
    assert_eq!(
        MAX_READING_AGE_SECS,
        crate::commands::profile::usage_history::READING_MAX_AGE,
        "a reading the gate accepts is one the usage history accepts"
    );
    assert!(
        MAX_READING_AGE_SECS as u64
            <= crate::commands::profile::probe_limits::DEFAULT_PASSIVE_MAX_AGE_SECS,
        "a passive reading accepted by default is never too old on age alone"
    );
    assert_eq!(MAX_REREADS_PER_CYCLE, 3);
}

#[test]
fn damping_keeps_at_most_16_entries_for_at_most_7_days() {
    assert_eq!(MAX_ENTRIES, 16);
    assert_eq!(MAX_AGE_SECS, 7 * 86_400);
}

// --- read_at ----------------------------------------------------------------

#[test]
fn a_passive_reading_is_read_at_its_observed_time() {
    assert_eq!(
        fresh::read_at(&rd(11.0, 95.0, Some(T - 40)), T),
        Some(T - 40)
    );
}

#[test]
fn a_probe_read_in_this_cycle_is_read_when_the_cycle_started_measuring() {
    assert_eq!(fresh::read_at(&rd(11.0, 95.0, None), T), Some(T));
}

#[test]
fn a_result_carried_from_an_earlier_cycle_has_no_reading_time() {
    let mut p = rd(11.0, 95.0, None);
    p.carried = true;
    assert_eq!(fresh::read_at(&p, T), None);
}

#[test]
fn a_still_running_unknown_or_failed_probe_has_no_reading_time() {
    let mut running = Probed::failed();
    running.still_running = true;
    assert_eq!(fresh::read_at(&running, T), None);
    assert_eq!(fresh::read_at(&Probed::failed(), T), None);
    // An unknown row that kept figures from a passive source.
    let mut unknown = rd(11.0, 95.0, Some(T - 10));
    unknown.unknown = true;
    assert_eq!(fresh::read_at(&unknown, T), None);
}

// --- stale ------------------------------------------------------------------

#[test]
fn no_reading_time_is_stale() {
    assert!(fresh::stale(None, Some(T), None, T));
    assert!(fresh::stale(None, None, None, T));
}

#[test]
fn a_reading_300_seconds_old_is_fresh_and_301_is_stale() {
    assert!(!fresh::stale(Some(T - 300), None, None, T));
    assert!(fresh::stale(Some(T - 301), None, None, T));
}

#[test]
fn a_reading_older_than_the_live_reading_is_stale_and_an_equal_one_fresh() {
    assert!(fresh::stale(Some(T - 1), Some(T), None, T));
    assert!(!fresh::stale(Some(T), Some(T), None, T));
    assert!(!fresh::stale(Some(T), Some(T - 60), None, T));
}

#[test]
fn with_the_live_probe_failed_only_the_age_test_applies() {
    assert!(!fresh::stale(Some(T - 200), None, None, T));
    assert!(fresh::stale(Some(T - 400), None, None, T));
}

#[test]
fn a_damped_login_needs_a_reading_strictly_later_than_the_swap_away() {
    assert!(fresh::stale(Some(T - 30), Some(T - 30), Some(T - 30), T));
    assert!(fresh::stale(Some(T - 40), None, Some(T - 30), T));
    assert!(!fresh::stale(Some(T - 29), Some(T - 30), Some(T - 30), T));
}

#[test]
fn a_clock_far_behind_the_reading_does_not_overflow() {
    assert!(!fresh::stale(Some(i64::MAX), None, None, T));
    assert!(fresh::stale(Some(i64::MIN), None, Some(i64::MIN), i64::MAX));
}

// --- gated_target -------------------------------------------------------------

#[test]
fn every_swap_chosen_by_reading_is_gated_and_overflow_in_is_not() {
    let to = n("ggrant");
    for reason in [
        Reason::Threshold,
        Reason::Upgrade,
        Reason::PlanRoom,
        Reason::FailedProbe,
        Reason::OverflowOut,
    ] {
        let d = Decision::Swap {
            to: to.clone(),
            reason,
        };
        assert_eq!(fresh::gated_target(&d), Some(&to), "{reason:?}");
    }
    let d = Decision::Swap {
        to: to.clone(),
        reason: Reason::OverflowIn,
    };
    assert_eq!(fresh::gated_target(&d), None);
    assert_eq!(fresh::gated_target(&Decision::Stay), None);
    assert_eq!(fresh::gated_target(&Decision::Alert), None);
}

// --- Log lines ----------------------------------------------------------------

#[test]
fn the_reread_lines_hold_only_the_name_and_whole_percentages() {
    let g = n("ggrant");
    assert_eq!(
        fresh_msg::stale(
            &g,
            &Shown::of(&rd(11.4, 95.2, Some(T - 60))),
            &Shown::of(&exh(43.0, 100.0))
        ),
        "reread: ggrant reading stale s=11% w=95%; fresh s=43% w=100%"
    );
    assert_eq!(
        fresh_msg::failed(&g),
        "reread: ggrant failed; treated as unknown"
    );
    assert_eq!(
        fresh_msg::limit(&g),
        "reread: ggrant limit reached; treated as unknown"
    );
    assert_eq!(
        fresh_msg::time_limit(&g),
        "reread: ggrant time limit reached; treated as unknown"
    );
}

#[test]
fn the_damp_lines_are_fixed_text() {
    let g = n("ggrant");
    assert_eq!(
        fresh_msg::still_damped(&g),
        "damp: ggrant swapped away exhausted; no later reading shows room"
    );
    assert_eq!(
        fresh_msg::lifted(&g),
        "damp: ggrant later reading shows room; swap-back allowed"
    );
}

// --- swap_back: full, room ------------------------------------------------------

#[test]
fn a_login_at_or_over_a_threshold_or_exhausted_is_full() {
    let s = settings();
    let m = |session, week| Reading::Measured { session, week };
    assert!(swap_back::full(&m(85.0, 0.0), &s));
    assert!(swap_back::full(&m(0.0, 90.0), &s));
    assert!(swap_back::full(&Reading::EXHAUSTED, &s));
    assert!(!swap_back::full(&m(84.0, 89.0), &s));
    assert!(
        !swap_back::full(&Reading::Failed, &s),
        "unknown is not full"
    );
}

#[test]
fn room_is_below_both_thresholds_and_never_for_an_unknown_reading() {
    let s = settings();
    let m = |session, week| Reading::Measured { session, week };
    assert!(swap_back::room(&m(84.0, 89.0), &s));
    assert!(!swap_back::room(&m(85.0, 0.0), &s));
    assert!(!swap_back::room(&m(0.0, 90.0), &s));
    assert!(!swap_back::room(&Reading::EXHAUSTED, &s));
    assert!(!swap_back::room(&Reading::Failed, &s));
}

// --- swap_back: valid, prune ----------------------------------------------------

#[test]
fn valid_drops_bad_names_and_impossible_times() {
    let now = T;
    let mut m = BTreeMap::new();
    m.insert("ggrant".to_string(), now - 60);
    m.insert("bad name".to_string(), now - 60);
    m.insert("../etc".to_string(), now - 60);
    m.insert(String::new(), now - 60);
    m.insert("zero".to_string(), 0);
    m.insert("negative".to_string(), -5);
    m.insert("future".to_string(), now + MAX_READING_AGE_SECS + 1);
    m.insert("skew".to_string(), now + MAX_READING_AGE_SECS);
    let got = swap_back::valid(m, now);
    let keys: Vec<&str> = got.keys().map(String::as_str).collect();
    assert_eq!(keys, ["ggrant", "skew"]);
}

#[test]
fn valid_keeps_the_16_newest_entries() {
    let now = T;
    let m: BTreeMap<String, i64> = (0..20)
        .map(|i| (format!("p{i:02}"), now - 1000 + i))
        .collect();
    let got = swap_back::valid(m, now);
    assert_eq!(got.len(), MAX_ENTRIES);
    for i in 0..4 {
        assert!(
            !got.contains_key(&format!("p{i:02}")),
            "oldest p{i:02} dropped"
        );
    }
    assert!(got.contains_key("p19"));
}

#[test]
fn prune_removes_entries_older_than_7_days_and_keeps_the_cap() {
    let now = T;
    let mut m = BTreeMap::new();
    m.insert("old".to_string(), now - MAX_AGE_SECS - 1);
    m.insert("edge".to_string(), now - MAX_AGE_SECS);
    m.insert("new".to_string(), now - 10);
    swap_back::prune(&mut m, now);
    let keys: Vec<&str> = m.keys().map(String::as_str).collect();
    assert_eq!(keys, ["edge", "new"]);

    let mut big: BTreeMap<String, i64> = (0..17)
        .map(|i| (format!("p{i:02}"), now - 100 + i))
        .collect();
    swap_back::prune(&mut big, now);
    assert_eq!(big.len(), MAX_ENTRIES);
    assert!(!big.contains_key("p00"), "the oldest is dropped first");
}

// --- Ledger ---------------------------------------------------------------------

#[test]
fn an_empty_damping_map_is_not_written() {
    let text = serde_json::to_string(&RotateLedger::default()).unwrap();
    assert!(!text.contains("swapped_away"), "{text}");
}

#[test]
fn the_ledger_loads_damping_and_drops_bad_entries_rather_than_failing() {
    let e = env();
    let t = now();
    let mut l = RotateLedger::default();
    l.swapped_away.insert("ggrant".into(), t - 60);
    l.swapped_away.insert("bad name".into(), t - 60);
    l.swapped_away.insert("future".into(), t + 10_000);
    e.rs.save_ledger(&l).unwrap();
    let got = e.rs.load_ledger().unwrap().expect("a ledger");
    let keys: Vec<&str> = got.swapped_away.keys().map(String::as_str).collect();
    assert_eq!(keys, ["ggrant"]);
    assert_eq!(got.swapped_away["ggrant"], t - 60);
}

#[test]
fn an_older_reader_never_sees_the_damping_map() {
    let mut l = RotateLedger::default();
    l.swapped_away.insert("ggrant".into(), T);
    assert!(for_older_readers(&l, true).swapped_away.is_empty());
    assert_eq!(for_older_readers(&l, false).swapped_away.len(), 1);
}

// --- gate ---------------------------------------------------------------------

/// Never called: the measure stub of a context used only for `gate`.
fn unused_measure(_: &Ctx, _: &Snap) -> Result<Measurement, ProfileError> {
    panic!("the gate tests call decide_measured themselves")
}

fn gate_ctx(e: &Env, reread: Option<RereadFn>) -> Ctx {
    let mut c = ctx_with(e, unused_measure as MeasureFn);
    c.reread = reread;
    c
}

fn snap(c: &Ctx) -> Snap {
    snapshot(c, &State::new(c.store.root()).unwrap()).unwrap()
}

fn panics(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    panic!("no read may happen here, but {name} was read")
}

static GATE_CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn spare_full(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    GATE_CALLS.lock().unwrap().push(name.to_string());
    exh(43.0, 100.0)
}

#[test]
fn only_production_call_sites_wire_the_direct_reread() {
    let e = env();
    let s = super::settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let c = Ctx::new(e.store.clone(), e.home.clone(), s);
    assert!(
        c.reread.is_none(),
        "SR-10: Ctx::new reads nothing, so no test reaches a live read"
    );
    let wired = c.with_rereads();
    assert!(
        wired.reread.is_some() && wired.reread_live.is_some(),
        "Ctx::with_rereads installs both re-read hooks"
    );
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/rotate");
    let code_has = |file: &str, needle: &str| {
        let text = fs::read_to_string(dir.join(file)).unwrap();
        text.lines()
            .any(|l| l.split("//").next().unwrap_or_default().contains(needle))
    };
    assert!(
        code_has("cycle_ctx.rs", "fresh::reread_as_configured)"),
        "Ctx::with_rereads no longer wires fresh::reread_as_configured"
    );
    // Explicit commands retain configured re-reads. Event-driven watches
    // consume generation-bound observations and never install age re-reads.
    assert!(!code_has("daemon.rs", ".with_rereads()"));
    assert!(code_has("daemon.rs", "event_runtime::run"));
    assert!(
        code_has("mod.rs", ".with_rereads()"),
        "explicit rotation no longer wires the configured re-reads"
    );
    let switch = fs::read_to_string(dir.join("switch.rs")).unwrap();
    assert!(
        !switch.contains("reread_as_configured") && !switch.contains("with_rereads"),
        "a switch makes no decision, so it wires no re-read"
    );
}

static STUCK_READS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn stuck_read(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    STUCK_READS.lock().unwrap().push(name.to_string());
    Probed::failed()
}

#[test]
fn a_decision_that_keeps_naming_an_unknown_target_is_an_error_within_the_cap() {
    let e = env();
    let c = gate_ctx(&e, Some(stuck_read));
    let s = snap(&c);
    let at = now();
    let m = decide_measured(
        &c,
        &s,
        spent(),
        vec![(n("spare"), rd(11.0, 20.0, Some(at - 60)))],
        at,
    )
    .unwrap();
    let stuck = Decision::Swap {
        to: n("spare"),
        reason: Reason::Threshold,
    };
    assert_eq!(m.decision, stuck, "the stale reading wins first");
    let mut decisions = 0;
    // A decision rule that, unlike decide, picks a failed target.
    let got = fresh::settle(
        &c,
        m,
        Reread::Allowed,
        Duration::from_secs(120),
        |live, probes, at| {
            decisions += 1;
            let mut again = decide_measured(&c, &s, live, probes, at)?;
            again.decision = stuck.clone();
            Ok(again)
        },
    );
    assert!(
        matches!(got, Err(ProfileError::GateUnsettled)),
        "no swap onto an unknown target: {:?}",
        got.map(|m| m.decision)
    );
    assert_eq!(decisions, 2, "one name: read once, made unknown once");
    assert_eq!(*STUCK_READS.lock().unwrap(), ["spare"], "read once");
}

static SLOW_LEFT: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());

/// A slow endpoint: each read takes 200 ms and finds the login full.
fn slow_full(_: &Ctx, name: &ProfileName, left: Duration) -> Probed {
    SLOW_LEFT.lock().unwrap().push((name.to_string(), left));
    std::thread::sleep(Duration::from_millis(200));
    exh(43.0, 100.0)
}

#[test]
fn all_rereads_of_one_gate_share_one_time_limit() {
    let e = env();
    let c = gate_ctx(&e, Some(slow_full));
    let s = snap(&c);
    let at = now();
    let stale = |sess, week| rd(sess, week, Some(at - 60));
    let m = decide_measured(
        &c,
        &s,
        spent(),
        vec![
            (n("first"), stale(5.0, 5.0)),
            (n("second"), stale(10.0, 10.0)),
            (n("third"), stale(15.0, 15.0)),
        ],
        at,
    )
    .unwrap();
    assert!(
        matches!(&m.decision, Decision::Swap { .. }),
        "without the gate a stale reading wins: {:?}",
        m.decision
    );
    let budget = Duration::from_millis(300);
    let got = fresh::settle(&c, m, Reread::Allowed, budget, |live, probes, at| {
        decide_measured(&c, &s, live, probes, at)
    })
    .unwrap();
    let left = SLOW_LEFT.lock().unwrap().clone();
    assert_eq!(
        left.len(),
        2,
        "the third read is never started; the limit is spent: {left:?}"
    );
    assert!(left[0].1 <= budget, "the first read gets at most the limit");
    assert!(
        left[1].1 < Duration::from_millis(150),
        "the second read gets only what the first left, not a new deadline: {left:?}"
    );
    let third = got
        .fresh_lines
        .iter()
        .find(|l| l.contains("time limit reached"))
        .unwrap_or_else(|| panic!("no time-limit line: {:?}", got.fresh_lines));
    assert!(
        !left.iter().any(|(name, _)| third.contains(name.as_str())),
        "the time-limit line names the login that was not read: {third}"
    );
    assert!(
        !matches!(got.decision, Decision::Swap { .. }),
        "no swap onto a login that was not read again: {:?}",
        got.decision
    );
}

#[test]
fn a_spent_time_limit_makes_the_target_unknown_with_no_read() {
    let e = env();
    let c = gate_ctx(&e, Some(panics));
    let s = snap(&c);
    let at = now();
    let m = decide_measured(
        &c,
        &s,
        spent(),
        vec![(n("spare"), rd(11.0, 20.0, Some(at - 60)))],
        at,
    )
    .unwrap();
    let got = fresh::settle(
        &c,
        m,
        Reread::Allowed,
        Duration::ZERO,
        |live, probes, at| decide_measured(&c, &s, live, probes, at),
    )
    .unwrap();
    assert_eq!(
        got.fresh_lines,
        ["reread: spare time limit reached; treated as unknown"]
    );
    assert!(
        !matches!(got.decision, Decision::Swap { .. }),
        "{:?}",
        got.decision
    );
}

#[test]
fn gate_rereads_a_stale_target_and_decides_again_without_writing() {
    let e = env();
    let c = gate_ctx(&e, Some(spare_full));
    let s = snap(&c);
    let at = now();
    let m = decide_measured(
        &c,
        &s,
        spent(),
        vec![(n("spare"), rd(11.0, 20.0, Some(at - 60)))],
        at,
    )
    .unwrap();
    assert!(
        matches!(&m.decision, Decision::Swap { to, .. } if to == &n("spare")),
        "without the gate the stale reading wins: {:?}",
        m.decision
    );
    let failed = m.failed;
    let got = fresh::gate(&c, &s, m, Reread::Allowed).unwrap();
    assert_eq!(*GATE_CALLS.lock().unwrap(), ["spare"]);
    assert_eq!(
        got.decision,
        Decision::Alert,
        "spare is full when read again"
    );
    assert_eq!(got.failed, failed, "a failed live probe is counted once");
    let spare = got.others.iter().find(|(n_, _)| n_ == &n("spare")).unwrap();
    assert_eq!(
        spare.1,
        exh(43.0, 100.0).reading,
        "the fresh figures replace the stale ones"
    );
    let probed = &got.probed.as_ref().expect("probes kept").1;
    let p = probed.iter().find(|(n_, _)| n_ == &n("spare")).unwrap();
    assert_eq!(p.1.reading, exh(43.0, 100.0).reading);
    assert_eq!(
        got.fresh_lines,
        ["reread: spare reading stale s=11% w=20%; fresh s=43% w=100%"]
    );
    assert_eq!(
        e.rs.load_ledger().unwrap().unwrap().account.as_deref(),
        Some("u-work"),
        "the gate preserves the ledger"
    );
    assert!(!e.rs.log.exists(), "the gate writes no log; the cycle does");
}

#[test]
fn gate_never_reads_under_the_lock_and_a_stale_target_is_unknown() {
    let e = env();
    let c = gate_ctx(&e, Some(panics));
    let s = snap(&c);
    let at = now();
    let m = decide_measured(
        &c,
        &s,
        spent(),
        vec![(n("spare"), rd(11.0, 20.0, Some(at - 60)))],
        at,
    )
    .unwrap();
    let got = fresh::gate(&c, &s, m, Reread::Never).unwrap();
    assert_eq!(got.decision, Decision::Alert);
    assert!(got.fresh_lines.is_empty(), "the Never path writes no line");
}

#[test]
fn gate_never_treats_a_target_damped_since_its_reading_as_unknown() {
    let e = env();
    let c = gate_ctx(&e, Some(panics));
    let s = snap(&c);
    let at = now();
    // Another rotator recorded the damping while this one measured.
    let mut l = RotateLedger::default();
    l.swapped_away.insert("spare".into(), at);
    e.rs.save_ledger(&l).unwrap();
    let m = decide_measured(
        &c,
        &s,
        spent(),
        vec![(n("spare"), rd(11.0, 20.0, None))],
        at,
    )
    .unwrap();
    assert!(
        matches!(&m.decision, Decision::Swap { .. }),
        "{:?}",
        m.decision
    );
    let got = fresh::gate(&c, &s, m, Reread::Never).unwrap();
    assert_eq!(got.decision, Decision::Alert);
}

#[test]
fn gate_without_a_hook_treats_a_stale_target_as_unknown() {
    let e = env();
    let c = gate_ctx(&e, None);
    let s = snap(&c);
    let at = now();
    let m = decide_measured(
        &c,
        &s,
        spent(),
        vec![(n("spare"), rd(11.0, 20.0, Some(at - 60)))],
        at,
    )
    .unwrap();
    let got = fresh::gate(&c, &s, m, Reread::Allowed).unwrap();
    assert_eq!(got.decision, Decision::Alert);
}

#[test]
fn gate_leaves_a_fresh_target_and_a_stay_alone() {
    let e = env();
    let c = gate_ctx(&e, Some(panics));
    let s = snap(&c);
    let at = now();
    let m = decide_measured(
        &c,
        &s,
        spent(),
        vec![(n("spare"), rd(11.0, 20.0, None))],
        at,
    )
    .unwrap();
    let got = fresh::gate(&c, &s, m, Reread::Allowed).unwrap();
    assert!(
        matches!(&got.decision, Decision::Swap { to, .. } if to == &n("spare")),
        "{:?}",
        got.decision
    );
    assert!(got.fresh_lines.is_empty());
    // A stay is never gated, however old its candidates' readings.
    let m = decide_measured(
        &c,
        &s,
        rd(10.0, 10.0, None),
        vec![(n("spare"), rd(11.0, 20.0, Some(at - 4000)))],
        at,
    )
    .unwrap();
    assert_eq!(m.decision, Decision::Stay);
    let got = fresh::gate(&c, &s, m, Reread::Allowed).unwrap();
    assert_eq!(got.decision, Decision::Stay);
}

#[test]
fn a_gate_that_reads_again_leaves_the_ledger_bytes_unchanged() {
    let e = env();
    let c = gate_ctx(&e, Some(spare_full_quiet));
    let s = snap(&c);
    let at = now();
    let mut l = RotateLedger::default();
    l.swapped_away.insert("third".into(), at - 30);
    e.rs.save_ledger(&l).unwrap();
    let before = fs::read(&e.rs.ledger).unwrap();
    let m = decide_measured(
        &c,
        &s,
        spent(),
        vec![(n("spare"), rd(11.0, 20.0, Some(at - 60)))],
        at,
    )
    .unwrap();
    fresh::gate(&c, &s, m, Reread::Allowed).unwrap();
    assert_eq!(fs::read(&e.rs.ledger).unwrap(), before, "SR-9: read-only");
}

fn spare_full_quiet(_: &Ctx, _: &ProfileName, _: Duration) -> Probed {
    exh(43.0, 100.0)
}

/// The gate passes a measurement with no probes unchanged; only test stubs
/// build one. Outside tests the one `Measurement` literal is in
/// `decide_measured`, and it keeps the probes.
#[test]
fn every_production_measurement_keeps_its_probes() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/rotate");
    let mut built = Vec::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        if file.starts_with("tests") || path.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        for (i, l) in text.lines().enumerate() {
            let code = l.split("//").next().unwrap_or_default();
            // A destructuring `let Measurement {` and the definition build nothing.
            if code.contains("Measurement {")
                && !code.contains("let Measurement {")
                && !code.contains("struct Measurement")
            {
                let rest: String = text.lines().skip(i).take(4).collect();
                built.push((file.clone(), rest));
            }
        }
    }
    assert_eq!(built.len(), 1, "one production literal: {built:?}");
    assert_eq!(built[0].0, "observe.rs");
    assert!(built[0].1.contains("probed: Some("), "{built:?}");
}
