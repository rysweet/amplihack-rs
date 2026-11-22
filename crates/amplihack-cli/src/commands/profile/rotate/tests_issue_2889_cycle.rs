//! Issue #2889: the rotator decides on the freshest reading for the live
//! login, and says when a reading was cached and how old it is.
//!
//! These tests are written before the change and fail until it exists. They
//! use the #2211 cycle harness (`lane!`, `Live::{Direct, Passive, Failed}`,
//! `REREAD`, a synthetic clock, `--interval 120 --session-at 70 --margin 5`)
//! and the real log and ledger under a temporary HOME. Nothing reads a real
//! login, starts `claude` or uses the network.
//!
//! The contract these tests pin, beyond the requirements text:
//!
//! - `fresh::live::Held { at, session, week, resets, week_resets }` and
//!   `Series::hold(&name, Held)`, `Series::held(&name, t) -> Option<Held>`
//!   (offered only while the session and the week window can still be
//!   open), `Series::note(..) -> bool` (accepted or not) and
//!   `fresh::live::week_resets(&Probed)` are re-exported for tests beside
//!   the existing `Point`, `Series` and `window`.
//! - `log::Shown::cached: Option<u64>`, `log::shown(&m, live_excluded, now)`
//!   and `msg::usage` appending `{sep}cached=<n>s`.
//! - `now` for the age is the cycle's own clock, `(ctx.now)()`, the clock
//!   that dates the readings, and not the wall clock the log lines are
//!   stamped with. The cycle tests run on a synthetic clock, so an age taken
//!   from the wall clock would not match.
//! - Ledger bit: `last_cycle.live_room`.

use super::super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::super::decide::{Decision, Probed, Reading};
use super::super::fresh::live::{Held, SESSION_WINDOW_SECS, Series, week_resets};
use super::super::log::{Shown, msg, shown};
use super::super::observe::Measurement;
use super::super::tests_issue_1870::{exh, rd};
use super::super::tests_revalidate_cause::{ctx_with, log_of};
use super::super::tests_swap::{Env, env, n, private};
use super::tests_issue_2211_cycle::Live;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::usage_cmd::ENV_CREDENTIAL_VARS;
use crate::test_support::{EnvGuard, env_lock};
use std::fs;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

const INTERVAL: i64 = 120;
/// How far ahead the session window of every reading resets: well past any
/// test, so the window stays open unless a test says otherwise.
const OPEN: i64 = 7200;
/// The week window resets this long after the session one.
const WEEK_AHEAD: i64 = 3 * 86_400;

const STALE: &str = "stale live reading; re-read";

/// `p` with its session window resetting at `resets` and its week window
/// three days later; unchanged for 0. Used by `lane!` as `super::in_window`.
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

fn seed(clock: &AtomicI64, reset: &AtomicI64, open_for: i64) {
    let t = now();
    clock.store(t, Ordering::SeqCst);
    reset.store(t + open_for, Ordering::SeqCst);
}

fn step(c: &mut Ctx, clock: &AtomicI64) -> CycleOutcome {
    let got = run_cycle(c, Mode::Watch).unwrap();
    clock.fetch_add(INTERVAL, Ordering::SeqCst);
    got
}

fn set<T: Clone>(m: &Mutex<Vec<T>>, v: &[T]) {
    *m.lock().unwrap() = v.to_vec();
}

/// No candidate has room: a stay on the live login is never an upgrade.
fn spare_full() -> Vec<(&'static str, Probed)> {
    vec![("spare", exh(100.0, 100.0))]
}

/// `spare` with room, as in the #2211 tests.
fn spare_room() -> Vec<(&'static str, Probed)> {
    vec![
        ("spare", rd(0.0, 40.0, None)),
        ("work", rd(0.0, 40.0, None)),
    ]
}

/// The `cycle` lines of the log, in order.
fn cycle_lines(log: &str) -> Vec<String> {
    log.lines()
        .filter(|l| l.contains(" cycle live=work "))
        .map(str::to_string)
        .collect()
}

/// The live login's entry of a `cycle` line: `work:s=..,w=..[,cached=..s]`.
fn live_entry(line: &str) -> String {
    line.split_whitespace()
        .find(|t| t.starts_with("work:"))
        .unwrap_or_default()
        .to_string()
}

fn entries(e: &Env) -> Vec<String> {
    cycle_lines(&log_of(e))
        .iter()
        .map(|l| live_entry(l))
        .collect()
}

/// `last_cycle.live_room` of the ledger the last cycle wrote.
fn live_room(e: &Env) -> Option<bool> {
    let raw = fs::read_to_string(&e.rs.ledger).expect("the cycle wrote a ledger");
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    json["last_cycle"]["live_room"].as_bool()
}

fn stale_lines(log: &str) -> usize {
    log.lines().filter(|l| l.ends_with(STALE)).count()
}

fn cached(session: f64, week: f64) -> Live {
    Live::Passive(session, week, 5)
}

// --- Behaviour from the issue: a fresh reading and an older cached one -----

lane!(l1);

#[test]
fn a_fresh_probe_then_an_older_cached_reading_decides_on_the_fresh_one() {
    // Cycle 1 reads 58%/100% directly. In cycle 2 the cache still holds the
    // 5%/95% it had when the login was swapped in, and no re-read can be
    // made. The cycle decides on, and logs, the fresh 58%/100%.
    let e = env();
    seed(&l1::CLOCK, &l1::RESET, OPEN);
    set(&l1::LIVE, &[Live::Direct(58.0, 100.0), cached(5.0, 95.0)]);
    set(&l1::OTHERS, &spare_full());
    set(&l1::REREAD, &[]);
    let mut c = ctx(&e, l1::clock, l1::measure, Some(l1::reread));
    step(&mut c, &l1::CLOCK);
    step(&mut c, &l1::CLOCK);
    let got = entries(&e);
    let log = log_of(&e);
    assert_eq!(got[0], "work:s=58%,w=100%", "a direct reading: {log}");
    assert_eq!(
        got[1], "work:s=58%,w=100%,cached=120s",
        "decided on the held fresh reading, 120 s old: {log}"
    );
    assert_eq!(live_room(&e), Some(false), "the fresh week is full: {log}");
    assert_eq!(stale_lines(&log), 1, "the failure stays loud: {log}");
}

lane!(l2);

#[test]
fn a_cached_reading_then_a_fresh_probe_decides_on_the_fresh_one() {
    // The other order. Cycle 1 has only the cache, so it decides on it, and
    // says so. Cycle 2 reads 58%/100% directly and decides on that, with no
    // cached age.
    let e = env();
    seed(&l2::CLOCK, &l2::RESET, OPEN);
    set(&l2::LIVE, &[cached(5.0, 95.0), Live::Direct(58.0, 100.0)]);
    set(&l2::OTHERS, &spare_full());
    set(&l2::REREAD, &[]);
    let mut c = ctx(&e, l2::clock, l2::measure, Some(l2::reread));
    step(&mut c, &l2::CLOCK);
    assert_eq!(entries(&e)[0], "work:s=5%,w=95%,cached=5s");
    assert_eq!(live_room(&e), Some(true), "nothing fresher is known");
    step(&mut c, &l2::CLOCK);
    assert_eq!(entries(&e)[1], "work:s=58%,w=100%");
    assert_eq!(live_room(&e), Some(false));
}

// --- The report: four cycles that alternated -------------------------------

lane!(l3);

#[test]
fn the_live_figures_do_not_alternate_back_to_the_stale_pair() {
    let e = env();
    seed(&l3::CLOCK, &l3::RESET, OPEN);
    set(
        &l3::LIVE,
        &[
            Live::Direct(56.0, 100.0),
            cached(5.0, 95.0),
            Live::Direct(58.0, 100.0),
            cached(5.0, 95.0),
        ],
    );
    set(&l3::OTHERS, &spare_full());
    set(&l3::REREAD, &[]);
    let mut c = ctx(&e, l3::clock, l3::measure, Some(l3::reread));
    for i in 1..=4 {
        step(&mut c, &l3::CLOCK);
        assert_eq!(live_room(&e), Some(false), "cycle {i}: no room recorded");
    }
    let log = log_of(&e);
    assert_eq!(
        entries(&e),
        [
            "work:s=56%,w=100%",
            "work:s=56%,w=100%,cached=120s",
            "work:s=58%,w=100%",
            "work:s=58%,w=100%,cached=120s",
        ],
        "{log}"
    );
    assert!(!log.contains("work:s=5%,w=95%"), "the stale pair: {log}");
}

lane!(l4);

#[test]
fn a_held_reading_is_not_replaced_by_the_stale_one_it_stood_in_for() {
    // Three cached cycles in a row after one direct read: every one decides
    // on the direct reading, and its age grows from the time it was read.
    let e = env();
    seed(&l4::CLOCK, &l4::RESET, OPEN);
    set(&l4::LIVE, &[Live::Direct(58.0, 100.0), cached(5.0, 95.0)]);
    set(&l4::OTHERS, &spare_full());
    set(&l4::REREAD, &[]);
    let mut c = ctx(&e, l4::clock, l4::measure, Some(l4::reread));
    for _ in 0..4 {
        step(&mut c, &l4::CLOCK);
    }
    assert_eq!(
        entries(&e),
        [
            "work:s=58%,w=100%",
            "work:s=58%,w=100%,cached=120s",
            "work:s=58%,w=100%,cached=240s",
            "work:s=58%,w=100%,cached=360s",
        ]
    );
    assert_eq!(live_room(&e), Some(false));
}

// --- A re-read that succeeds is a reading taken in this cycle --------------

lane!(l5);

#[test]
fn a_successful_reread_decides_the_cycle_and_carries_no_age() {
    let e = env();
    seed(&l5::CLOCK, &l5::RESET, OPEN);
    set(&l5::LIVE, &[Live::Direct(58.0, 100.0), cached(5.0, 95.0)]);
    set(&l5::OTHERS, &spare_full());
    set(&l5::REREAD, &[rd(59.0, 100.0, None)]);
    let mut c = ctx(&e, l5::clock, l5::measure, Some(l5::reread));
    step(&mut c, &l5::CLOCK);
    step(&mut c, &l5::CLOCK);
    let log = log_of(&e);
    assert_eq!(entries(&e)[1], "work:s=59%,w=100%", "{log}");
    assert_eq!(live_room(&e), Some(false), "{log}");
    assert_eq!(stale_lines(&log), 1, "{log}");
}

// --- The annotation --------------------------------------------------------

lane!(l6);

#[test]
fn a_cached_reading_is_annotated_with_its_age_and_a_direct_one_is_not() {
    let e = env();
    seed(&l6::CLOCK, &l6::RESET, OPEN);
    set(
        &l6::LIVE,
        &[Live::Passive(30.0, 20.0, 7), Live::Direct(31.0, 20.0)],
    );
    set(&l6::OTHERS, &spare_full());
    let mut c = ctx(&e, l6::clock, l6::measure, Some(l6::reread));
    step(&mut c, &l6::CLOCK);
    step(&mut c, &l6::CLOCK);
    assert_eq!(
        entries(&e),
        ["work:s=30%,w=20%,cached=7s", "work:s=31%,w=20%"]
    );
}

lane!(l7);

#[test]
fn a_reading_dated_after_the_measurement_began_carries_no_age() {
    // A cache that stamps a reading ahead of the clock is not a reading
    // taken before the measurement began, so it carries no age: never a
    // negative, zero or wrapped one invented for it. The step still judges
    // it stale and says so, and with no held reading the cycle decides on
    // the cached figures as measured.
    let e = env();
    seed(&l7::CLOCK, &l7::RESET, OPEN);
    set(&l7::LIVE, &[Live::Passive(30.0, 20.0, -50)]);
    set(&l7::OTHERS, &spare_full());
    set(&l7::REREAD, &[]);
    let mut c = ctx(&e, l7::clock, l7::measure, Some(l7::reread));
    step(&mut c, &l7::CLOCK);
    let log = log_of(&e);
    assert_eq!(entries(&e), ["work:s=30%,w=20%"], "{log}");
    assert_eq!(stale_lines(&log), 1, "the failure stays loud: {log}");
}

lane!(l8);

#[test]
fn a_swap_line_annotates_the_live_figures_and_never_the_target() {
    let e = env();
    seed(&l8::CLOCK, &l8::RESET, OPEN);
    set(&l8::LIVE, &[Live::Passive(80.0, 20.0, 5)]);
    set(&l8::OTHERS, &spare_room());
    let mut c = ctx(&e, l8::clock, l8::measure, Some(l8::reread));
    assert_eq!(step(&mut c, &l8::CLOCK), CycleOutcome::Swapped);
    let log = log_of(&e);
    assert!(
        log.contains(
            " swap work->spare live s=80% w=20% cached=5s target s=0% w=40% reason=threshold"
        ),
        "{log}"
    );
}

lane!(l9);

#[test]
fn a_swap_line_on_a_direct_reading_carries_no_age() {
    let e = env();
    seed(&l9::CLOCK, &l9::RESET, OPEN);
    set(&l9::LIVE, &[Live::Direct(80.0, 20.0)]);
    set(&l9::OTHERS, &spare_room());
    let mut c = ctx(&e, l9::clock, l9::measure, Some(l9::reread));
    assert_eq!(step(&mut c, &l9::CLOCK), CycleOutcome::Swapped);
    let log = log_of(&e);
    assert!(
        log.contains(" swap work->spare live s=80% w=20% target s=0% w=40% reason=threshold"),
        "{log}"
    );
    assert!(!log.contains("cached="), "{log}");
}

lane!(l10);

#[test]
fn no_candidate_entry_carries_an_age() {
    let e = env();
    seed(&l10::CLOCK, &l10::RESET, OPEN);
    set(&l10::LIVE, &[Live::Direct(30.0, 20.0)]);
    let mut spare = rd(0.0, 40.0, Some(now() - 5));
    spare.passive = true;
    set(&l10::OTHERS, &[("spare", spare)]);
    let mut c = ctx(&e, l10::clock, l10::measure, Some(l10::reread));
    step(&mut c, &l10::CLOCK);
    let line = &cycle_lines(&log_of(&e))[0];
    let spare_entry = line
        .split_whitespace()
        .find(|t| t.starts_with("spare:"))
        .expect("spare is on the cycle line");
    assert!(!spare_entry.contains("cached="), "{line}");
}

// --- A held reading never lowers the view ----------------------------------

lane!(l11);

#[test]
fn a_held_reading_lower_on_the_session_is_not_substituted() {
    // Held 40%/50%; the cache says 45%/30% and is 200 s old (too old, so
    // stale; no re-read possible). The held session is lower, so the view
    // would fall: the cached figures stand, with their age.
    let e = env();
    seed(&l11::CLOCK, &l11::RESET, OPEN);
    set(
        &l11::LIVE,
        &[Live::Direct(40.0, 50.0), Live::Passive(45.0, 30.0, 200)],
    );
    set(&l11::OTHERS, &spare_full());
    set(&l11::REREAD, &[]);
    let mut c = ctx(&e, l11::clock, l11::measure, Some(l11::reread));
    step(&mut c, &l11::CLOCK);
    step(&mut c, &l11::CLOCK);
    assert_eq!(entries(&e)[1], "work:s=45%,w=30%,cached=200s");
}

lane!(l12);

#[test]
fn a_held_reading_lower_on_the_week_is_not_substituted() {
    // Held 50%/40%; the cache says 45%/60%, lower on the session, so it
    // contradicts the held point and is read again, without success. The
    // held week is lower than the cached one, so the held reading is not
    // offered in its place.
    let e = env();
    seed(&l12::CLOCK, &l12::RESET, OPEN);
    set(&l12::LIVE, &[Live::Direct(50.0, 40.0), cached(45.0, 60.0)]);
    set(&l12::OTHERS, &spare_full());
    set(&l12::REREAD, &[]);
    let mut c = ctx(&e, l12::clock, l12::measure, Some(l12::reread));
    step(&mut c, &l12::CLOCK);
    step(&mut c, &l12::CLOCK);
    assert_eq!(entries(&e)[1], "work:s=45%,w=60%,cached=5s");
}

// --- The held reading is offered only while its windows can be open --------

lane!(l13);

#[test]
fn a_held_reading_is_not_used_after_its_session_window_has_reset() {
    // The session window of every reading resets 60 s after cycle 1, so by
    // cycle 2 the held 58% belongs to a window that is over.
    let e = env();
    seed(&l13::CLOCK, &l13::RESET, 60);
    set(&l13::LIVE, &[Live::Direct(58.0, 100.0), cached(5.0, 95.0)]);
    set(&l13::OTHERS, &spare_full());
    set(&l13::REREAD, &[]);
    let mut c = ctx(&e, l13::clock, l13::measure, Some(l13::reread));
    step(&mut c, &l13::CLOCK);
    step(&mut c, &l13::CLOCK);
    assert_eq!(entries(&e)[1], "work:s=5%,w=95%,cached=5s");
}

// --- A held reading is not recorded as one taken in this cycle -------------

lane!(l14);

#[test]
fn the_usage_history_does_not_record_a_held_reading_as_taken_this_cycle() {
    // 520 s pass between the two cycles, over the 300 s a reading may be
    // old and still be recorded. Cycle 2 decides on the held 58%, but the
    // history entry for it must not show 58% as a current reading.
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _vars = EnvGuard::unset(ENV_CREDENTIAL_VARS);
    let e = env();
    seed(&l14::CLOCK, &l14::RESET, OPEN);
    set(&l14::LIVE, &[Live::Direct(58.0, 100.0), cached(5.0, 95.0)]);
    set(&l14::OTHERS, &spare_full());
    set(&l14::REREAD, &[]);
    let mut c = ctx(&e, l14::clock, l14::measure, Some(l14::reread));
    step(&mut c, &l14::CLOCK);
    l14::CLOCK.fetch_add(400, Ordering::SeqCst);
    step(&mut c, &l14::CLOCK);
    let path = e
        .root
        .join(".auto-switch.d")
        .join(crate::commands::profile::usage_history::FILE);
    let body = fs::read_to_string(path).unwrap_or_default();
    let last = body.lines().last().expect("the history has a line");
    let v: serde_json::Value = serde_json::from_str(last).unwrap();
    assert_ne!(
        v["profiles"]["work"]["s"],
        serde_json::json!(58),
        "a 520 s old reading recorded as current: {last}"
    );
    assert_eq!(entries(&e)[1], "work:s=58%,w=100%,cached=520s");
}

lane!(l16);

#[test]
fn the_capacity_file_carries_the_held_figures_not_the_stale_ones() {
    // The capacity file has no reading time, so it cannot record a reading
    // as taken in this cycle. What it must not do is report capacity the
    // held reading denies. The held week is full and the stale cached week
    // is 50%, below the file's 90% threshold, so the two disagree about
    // whether `work` is over; the file follows the held reading.
    let e = env();
    seed(&l16::CLOCK, &l16::RESET, OPEN);
    set(&l16::LIVE, &[Live::Direct(58.0, 100.0), cached(5.0, 50.0)]);
    set(&l16::OTHERS, &spare_full());
    set(&l16::REREAD, &[]);
    // The capacity file is only written while auto-switch is on.
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _toggle = EnvGuard::unset([crate::commands::profile::auto_switch::config::TOGGLE_ENV]);
    private(&e.root.join(".auto-switch"), "enabled=on\n");
    let mut c = ctx(&e, l16::clock, l16::measure, Some(l16::reread));
    step(&mut c, &l16::CLOCK);
    step(&mut c, &l16::CLOCK);
    let raw = fs::read_to_string(e.root.join(".auto-switch.d").join("capacity.json"))
        .expect("the cycle wrote a capacity file");
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let over = json["over"].as_array().expect("an over list").clone();
    assert!(
        over.iter().any(|n| n == "work"),
        "the held full week stands: {raw}"
    );
}

// --- The target gate -------------------------------------------------------

lane!(l15);

#[test]
fn a_target_that_predates_the_held_live_reading_is_still_read_again() {
    // The held reading has a full week, so the cycle swaps off it. The
    // target was read before the held reading was taken, so the gate reads
    // it again, as it does for any target older than the live reading.
    let e = env();
    seed(&l15::CLOCK, &l15::RESET, OPEN);
    set(&l15::LIVE, &[Live::Direct(58.0, 100.0), cached(5.0, 95.0)]);
    let mut spare = rd(0.0, 40.0, Some(now() - 100));
    spare.passive = true;
    set(&l15::OTHERS, &[("spare", spare)]);
    set(&l15::REREAD, &[Probed::failed(), rd(0.0, 40.0, None)]);
    let mut c = ctx(&e, l15::clock, l15::measure, Some(l15::reread));
    step(&mut c, &l15::CLOCK);
    step(&mut c, &l15::CLOCK);
    assert!(
        l15::calls().iter().any(|n| n == "spare"),
        "the target was read again: {:?}",
        l15::calls()
    );
}

// --- Series: the held reading ----------------------------------------------

fn held(at: i64) -> Held {
    Held {
        at,
        session: 58.0,
        week: 100.0,
        resets: Some(at + 3600),
        week_resets: Some(at + WEEK_AHEAD),
    }
}

const T: i64 = 1_800_000_000;

#[test]
fn a_held_reading_is_offered_while_both_windows_can_be_open() {
    let mut s = Series::default();
    s.hold(&n("work"), held(T));
    assert_eq!(s.held(&n("work"), T + 600), Some(held(T)));
}

#[test]
fn a_held_reading_is_not_offered_after_the_session_reset() {
    let mut s = Series::default();
    s.hold(&n("work"), held(T));
    assert_eq!(s.held(&n("work"), T + 3599), Some(held(T)));
    assert_eq!(s.held(&n("work"), T + 3600), None);
}

#[test]
fn a_held_reading_is_not_offered_after_the_week_reset() {
    let mut h = held(T);
    h.resets = Some(T + 10 * 86_400);
    h.week_resets = Some(T + 7200);
    let mut s = Series::default();
    s.hold(&n("work"), h);
    assert_eq!(s.held(&n("work"), T + 7199), Some(h));
    assert_eq!(s.held(&n("work"), T + 7200), None);
}

/// A reset time a reading did not give bounds nothing, so the five hours of
/// `SESSION_WINDOW_SECS` bound the held reading alone. Each window is tested
/// on its own and then both together: the week reset is the one the
/// documented rule singles out ("a reading with no week reset time is
/// bounded by the five hours alone"), and a reading that gave neither must
/// not fall back to being offered for ever.
#[test]
fn a_held_reading_with_no_reset_time_lasts_one_session_window() {
    // A session reset far enough ahead to bind nothing, so only the missing
    // week reset and the five hours are under test, and the same the other
    // way about.
    let far = Some(T + 10 * 86_400);
    let cases = [
        ("no session reset", None, Some(T + WEEK_AHEAD)),
        ("no week reset", far, None),
        ("neither reset", None, None),
    ];
    for (what, resets, week_resets) in cases {
        let mut h = held(T);
        h.resets = resets;
        h.week_resets = week_resets;
        let mut s = Series::default();
        s.hold(&n("work"), h);
        assert_eq!(
            s.held(&n("work"), T + SESSION_WINDOW_SECS - 1),
            Some(h),
            "{what}: inside the window"
        );
        assert_eq!(
            s.held(&n("work"), T + SESSION_WINDOW_SECS),
            None,
            "{what}: the five hours are the whole bound"
        );
    }
}

#[test]
fn a_held_reading_is_for_one_login_only() {
    let mut s = Series::default();
    s.hold(&n("work"), held(T));
    assert_eq!(s.held(&n("spare"), T + 60), None, "another login");
}

#[test]
fn another_login_taking_the_series_drops_the_held_reading() {
    // A rename or a re-login in the same slot: `own` starts afresh.
    let mut s = Series::default();
    s.hold(&n("work"), held(T));
    assert!(s.note(&n("spare"), T + 10, 10.0, None, T + 20));
    assert_eq!(s.held(&n("work"), T + 60), None);
    assert_eq!(s.held(&n("spare"), T + 60), None);
}

#[test]
fn clear_drops_the_held_reading() {
    let mut s = Series::default();
    s.hold(&n("work"), held(T));
    s.clear();
    assert_eq!(s.held(&n("work"), T + 60), None);
}

// --- Series::note says whether it accepted the reading ---------------------

#[test]
fn note_returns_true_for_an_accepted_reading() {
    let mut s = Series::default();
    assert!(s.note(&n("work"), T, 40.0, Some(T + 3600), T + 5));
    assert!(s.note(&n("work"), T + 60, 45.0, Some(T + 3600), T + 65));
}

#[test]
fn note_returns_false_for_a_reading_it_does_not_accept() {
    let mut s = Series::default();
    let name = n("work");
    assert!(s.note(&name, T + 60, 45.0, Some(T + 3600), T + 65));
    assert!(
        !s.note(&name, T + 60, 50.0, Some(T + 3600), T + 65),
        "not newer"
    );
    assert!(
        !s.note(&name, T + 90, 40.0, Some(T + 3600), T + 95),
        "lower, same window"
    );
    assert!(
        !s.note(&name, T + 90, 50.0, Some(T - 7200), T + 95),
        "earlier window"
    );
    assert!(
        !s.note(&name, T + 90, f64::NAN, Some(T + 3600), T + 95),
        "not finite"
    );
    assert!(
        !s.note(&name, T + 900, 50.0, Some(T + 3600), T + 95),
        "from the future"
    );
}

// --- week_resets -----------------------------------------------------------

#[test]
fn week_resets_reads_the_week_window_reset_time() {
    let p = in_window(rd(10.0, 20.0, None), T);
    assert_eq!(week_resets(&p), Some(T + WEEK_AHEAD));
    assert_eq!(week_resets(&rd(10.0, 20.0, None)), None);
}

// --- Shown and the grammar -------------------------------------------------

fn measurement(live: Probed, measured_at: i64) -> Measurement {
    Measurement {
        live: live.reading,
        others: Vec::new(),
        decision: Decision::Stay,
        note: None,
        failed: 0,
        tag: None,
        new_blocks: Vec::new(),
        next_plan_reset: None,
        unknown_plan: Vec::new(),
        key_held: Vec::new(),
        measured_at,
        probed: Some((live, vec![(n("spare"), passive(0.0, 40.0, T - 30))])),
        fresh_lines: Vec::new(),
        rejected: Vec::new(),
    }
}

fn passive(s: f64, w: f64, at: i64) -> Probed {
    let mut p = rd(s, w, Some(at));
    p.passive = true;
    p
}

#[test]
fn shown_gives_a_cached_live_reading_its_age_in_whole_seconds() {
    let m = measurement(passive(5.0, 95.0, T - 612), T);
    let (live, others) = shown(&m, false, T);
    assert_eq!(live.cached, Some(612));
    assert!(others.iter().all(|(_, s)| s.cached.is_none()), "{others:?}");
}

#[test]
fn shown_gives_a_direct_live_reading_no_age() {
    let m = measurement(rd(5.0, 95.0, None), T);
    assert_eq!(shown(&m, false, T).0.cached, None);
}

#[test]
fn shown_gives_a_reading_taken_after_the_measurement_began_no_age() {
    let m = measurement(passive(5.0, 95.0, T + 1), T);
    assert_eq!(shown(&m, false, T + 2).0.cached, None);
}

#[test]
fn shown_gives_a_reading_with_no_reading_time_no_age() {
    let mut live = passive(5.0, 95.0, T - 10);
    live.carried = true;
    let m = measurement(live, T);
    assert_eq!(
        shown(&m, false, T).0.cached,
        None,
        "carried: no reading time"
    );
}

#[test]
fn shown_clamps_a_future_reading_time_to_zero() {
    let m = measurement(passive(5.0, 95.0, T - 10), T);
    assert_eq!(shown(&m, false, T - 500).0.cached, Some(0));
}

#[test]
fn shown_survives_extreme_reading_times() {
    for at in [i64::MIN, i64::MAX, i64::MIN + 1, i64::MAX - 1] {
        let m = measurement(passive(5.0, 95.0, at), T);
        let got = shown(&m, false, T).0.cached;
        assert!(got.is_none() || got.is_some(), "no panic at {at}");
    }
    let m = measurement(passive(5.0, 95.0, i64::MAX), T);
    assert_eq!(
        shown(&m, false, T).0.cached,
        None,
        "read after the measurement began: no age is claimed for it"
    );
    let m = measurement(passive(5.0, 95.0, i64::MIN), T);
    assert!(
        shown(&m, false, T).0.cached.is_some_and(|n| n > 0),
        "an ancient reading reports a large age, never a wrapped one"
    );
}

#[test]
fn shown_builders_default_to_no_age() {
    assert_eq!(Shown::of(&passive(5.0, 95.0, T)).cached, None);
    let r = Reading::Measured {
        session: 5.0,
        week: 95.0,
    };
    assert_eq!(Shown::from_reading(r).cached, None);
}

#[test]
fn usage_appends_the_age_with_the_separator_of_the_line() {
    let mut s = Shown::from_reading(Reading::Measured {
        session: 5.0,
        week: 95.0,
    });
    assert_eq!(msg::usage(&s, ","), "s=5%,w=95%");
    s.cached = Some(612);
    assert_eq!(msg::usage(&s, ","), "s=5%,w=95%,cached=612s");
    assert_eq!(msg::usage(&s, " "), "s=5% w=95% cached=612s");
}

#[test]
fn usage_annotates_an_exhausted_reading_and_no_other_kind() {
    let mut s = Shown::from_reading(Reading::Exhausted {
        session: Some(100.0),
        week: Some(95.0),
    });
    s.cached = Some(30);
    assert_eq!(msg::usage(&s, ","), "s=100%,w=95%,cached=30s");
    let mut failed = Shown::from_reading(Reading::Failed);
    failed.cached = Some(30);
    assert_eq!(msg::usage(&failed, ","), "failed");
    let mut excluded = Shown::from_reading(Reading::Failed);
    excluded.excluded = true;
    excluded.cached = Some(30);
    assert!(!msg::usage(&excluded, ",").contains("cached"));
    let mut running = Shown::from_reading(Reading::Failed);
    running.still_running = true;
    running.cached = Some(30);
    assert!(!msg::usage(&running, ",").contains("cached"));
}
