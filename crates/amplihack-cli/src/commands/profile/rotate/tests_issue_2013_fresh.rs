//! Issue #2013 (D5, D6, D7, D12): fresh readings and how readings are shown.
//!
//! Contract:
//! - `fresh::max_age(&Settings) -> i64`: `min(300, interval)` for an
//!   interval above 0, else 300. `MAX_READING_AGE_SECS` stays 300.
//! - `fresh::stale(read_at, live_read_at, damped_at, now, max_age)`: no
//!   reading time, older than `max_age`, or later than `now` is stale. All
//!   arithmetic saturates.
//! - `Ctx.clock: fn() -> i64`, defaulting to the system clock; the gate,
//!   `decide_measured` and the alert limit read the time only through it.
//! - `Probed.failure: Option<String>` (error kind and status only) and
//!   `Probed.started_at: Option<i64>` (when a still-running or carried read
//!   started).
//! - `log::Shown::at(&Probed, measured_at, now, max_age)` and
//!   `log::msg::reading(&Shown) -> String`, the D7 formats:
//!   `s=41% w=97% age=35s`, `s=41% w=97% stale age=7320s (not used)`,
//!   `probe failed: <reason>` and `probe running since <n>s (not used)`.
//!   The cycle line shows `<name>:s=..%,w=..%,age=..s`, `<name>:stale`,
//!   `<name>:failed` or `<name>:running`, each of the last three followed
//!   by one `reading: <name> ...` line.
//! - `log::failure_text(&str) -> String`: control and unprintable characters
//!   removed, cut to 128 characters.
//! - D6: a live reading older than `max_age` gives `Decision::Stay` with the
//!   fresh line `stay: live reading stale age=<n>s (not used)`; the cycle
//!   records `stayed-unread` and does not count a failed live probe.

use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::decide::{Decision, Probed};
use super::fresh::{self, MAX_READING_AGE_SECS, stale};
use super::log::{Shown, failure_text, msg};
use super::observe::{Measurement, Snap, decide_measured};
use super::settings::Settings;
use super::tests_issue_2013_common::{
    clock, ctx_at, failed, lines, rd, running, set_now, settings,
};
use super::tests_revalidate_cause::{add_stored, ctx_with, probes_for, spent};
use super::tests_swap::{env, ino_at, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use std::sync::Mutex;
use std::time::Duration;

const T: i64 = 1_794_200_000;

fn with_interval(interval: u64) -> Settings {
    Settings {
        interval,
        ..settings()
    }
}

// --- max_age and stale -------------------------------------------------------------

#[test]
fn the_age_limit_is_one_interval_capped_at_300_seconds() {
    assert_eq!(MAX_READING_AGE_SECS, 300, "the cap is never raised");
    assert_eq!(fresh::max_age(&with_interval(120)), 120);
    assert_eq!(fresh::max_age(&with_interval(300)), 300);
    assert_eq!(fresh::max_age(&with_interval(600)), 300);
    assert_eq!(fresh::max_age(&with_interval(0)), 300);
}

#[test]
fn a_reading_at_the_limit_is_fresh_and_one_second_older_is_stale() {
    assert!(!stale(Some(T - 120), None, None, T, 120));
    assert!(stale(Some(T - 121), None, None, T, 120));
    assert!(!stale(Some(T - 300), None, None, T, 300));
    assert!(stale(Some(T - 301), None, None, T, 300));
}

#[test]
fn a_reading_time_later_than_now_is_stale() {
    assert!(stale(Some(T + 1), None, None, T, 300));
    assert!(stale(Some(i64::MAX), None, None, i64::MIN, 300));
}

#[test]
fn no_reading_time_is_stale_and_extreme_times_do_not_overflow() {
    assert!(stale(None, None, None, T, 300));
    assert!(stale(Some(i64::MIN), None, None, i64::MAX, 300));
}

#[test]
fn the_earlier_live_and_damping_rules_still_apply() {
    assert!(stale(Some(T - 10), Some(T - 5), None, T, 300));
    assert!(stale(Some(T - 10), None, Some(T - 10), T, 300));
    assert!(!stale(Some(T - 10), Some(T - 10), Some(T - 11), T, 300));
}

#[test]
fn the_default_clock_is_the_system_clock() {
    let e = env();
    let c = ctx_with(&e, unused);
    let before = now();
    let got = (c.clock)();
    let after = now();
    assert!(
        before <= got && got <= after,
        "{before} <= {got} <= {after}"
    );
}

fn unused(_: &Ctx, _: &Snap) -> Result<Measurement, ProfileError> {
    panic!("not measured")
}

// --- D7 display ---------------------------------------------------------------------

fn shown(p: &Probed) -> Shown {
    Shown::at(p, T, T, 300)
}

#[test]
fn a_fresh_reading_shows_its_age() {
    assert_eq!(
        msg::reading(&shown(&rd(41.0, 97.0, Some(T - 35)))),
        "s=41% w=97% age=35s"
    );
    assert_eq!(
        msg::reading(&shown(&rd(41.0, 97.0, None))),
        "s=41% w=97% age=0s"
    );
}

#[test]
fn a_stale_reading_says_it_is_not_used() {
    assert_eq!(
        msg::reading(&shown(&rd(41.0, 97.0, Some(T - 7320)))),
        "s=41% w=97% stale age=7320s (not used)"
    );
}

#[test]
fn a_failed_probe_shows_its_reason_and_no_percentages() {
    let got = msg::reading(&shown(&failed("http 429")));
    assert_eq!(got, "probe failed: http 429");
    assert!(!got.contains('%'), "{got}");
}

#[test]
fn a_failed_probe_never_shows_the_figures_of_an_earlier_reading() {
    // A failed probe that still carries windows from somewhere.
    let mut p = rd(41.0, 97.0, Some(T - 30));
    p.reading = super::decide::Reading::Failed;
    p.unknown = true;
    p.failure = Some("timeout".into());
    let got = msg::reading(&shown(&p));
    assert_eq!(got, "probe failed: timeout");
}

#[test]
fn a_running_read_shows_how_long_it_has_run() {
    assert_eq!(
        msg::reading(&shown(&running(T - 130))),
        "probe running since 130s (not used)"
    );
}

#[test]
fn the_cycle_line_shows_age_and_names_stale_failed_and_running_logins() {
    let live = shown(&rd(41.0, 97.0, Some(T - 35)));
    let others = vec![
        (n("aold"), shown(&rd(10.0, 10.0, Some(T - 7320)))),
        (n("bfail"), shown(&failed("timeout"))),
        (n("crun"), shown(&running(T - 130))),
        (n("dok"), shown(&rd(5.0, 11.0, None))),
    ];
    assert_eq!(
        msg::cycle(Some(&n("work")), &live, &others, None),
        "cycle live=work work:s=41%,w=97%,age=35s aold:stale bfail:failed crun:running dok:s=5%,w=11%,age=0s"
    );
}

#[test]
fn both_readings_of_a_swap_line_show_their_age() {
    let live = shown(&rd(100.0, 72.0, Some(T - 2)));
    let target = shown(&rd(0.0, 43.0, Some(T - 2)));
    assert_eq!(
        msg::swap(
            &n("work"),
            &n("spare"),
            &live,
            &target,
            super::decide::Reason::Threshold
        ),
        "swap work->spare live s=100% w=72% age=2s target s=0% w=43% age=2s reason=threshold"
    );
    let failed_live = shown(&failed("http 503"));
    assert!(
        msg::swap(
            &n("work"),
            &n("spare"),
            &failed_live,
            &target,
            super::decide::Reason::FailedProbe
        )
        .starts_with("swap work->spare live probe failed: http 503 target s=0% w=43% age=2s")
    );
}

#[test]
fn failure_text_strips_control_characters_and_is_cut_to_128_characters() {
    assert_eq!(failure_text("http\u{1b}[31m 429\n"), "http[31m 429");
    assert_eq!(failure_text("bidi\u{202e}x"), "bidix");
    let long = "x".repeat(500);
    assert_eq!(failure_text(&long).chars().count(), 128);
}

// --- D6: a stale live reading in a cycle ---------------------------------------------------

fn measure_live_stale(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(snap, &[("spare", rd(0.0, 10.0, None))]);
    // Values first read more than two hours ago, as at 22:49Z on 6 Nov.
    let m = decide_measured(ctx, snap, rd(17.0, 98.0, Some(at - 7320)), probes, at)?;
    assert_eq!(
        m.decision,
        Decision::Stay,
        "no decision from a stale live reading"
    );
    assert_eq!(m.failed, 0, "a stale reading is not a failed probe");
    Ok(m)
}

#[cfg(target_os = "linux")]
#[test]
fn d6_a_live_reading_two_hours_old_is_never_used_and_the_cycle_stays() {
    let e = env();
    let live = ino_at(&e.live());
    let mut c = ctx_at(&e, measure_live_stale, settings(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::StayedUnread,
        "{:?}",
        lines(&e)
    );
    assert_eq!(ino_at(&e.live()), live, "nothing moved");
    let log = lines(&e);
    assert!(
        log.iter()
            .any(|l| l == "stay: live reading stale age=7320s (not used)"),
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|l| l == "reading: work s=17% w=98% stale age=7320s (not used)"),
        "{log:?}"
    );
    let cycle = log.iter().find(|l| l.starts_with("cycle ")).unwrap();
    assert!(cycle.starts_with("cycle live=work work:stale "), "{cycle}");
    assert!(!log.iter().any(|l| l.starts_with("swap ")), "{log:?}");
}

fn measure_live_failed(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(snap, &[("spare", rd(0.0, 10.0, None))]);
    decide_measured(ctx, snap, failed("http 429"), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn d6_a_failed_live_probe_shows_as_failed_with_no_percentages() {
    let e = env();
    let mut c = ctx_at(&e, measure_live_failed, settings(), now());
    run_cycle(&mut c, Mode::Watch).unwrap();
    let log = lines(&e);
    let cycle = log.iter().find(|l| l.starts_with("cycle ")).unwrap();
    assert!(cycle.starts_with("cycle live=work work:failed "), "{cycle}");
    assert!(
        log.iter()
            .any(|l| l == "reading: work probe failed: http 429"),
        "{log:?}"
    );
}

// --- D5: a candidate older than one interval is read again -------------------------------

static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn reread_full(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    CALLS.lock().unwrap().push(name.to_string());
    super::tests_issue_2013_common::exh(43.0, 100.0)
}

fn measure_spare_200s(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(
        snap,
        &[
            ("spare", rd(0.0, 10.0, Some(at - 200))),
            ("third", rd(0.0, 60.0, None)),
        ],
    );
    decide_measured(ctx, snap, spent(), probes, at)
}

/// With `--interval 120` a passive reading 200 s old is stale, though it is
/// under the 300-second cap: the target is read again through the fake
/// re-read, and the decision is made on the new figures.
#[cfg(target_os = "linux")]
#[test]
fn d5_with_interval_120_a_candidate_reading_200_seconds_old_is_read_again() {
    let e = env();
    let third = add_stored(&e, "third");
    let mut c = ctx_at(&e, measure_spare_200s, with_interval(120), now());
    c.reread = Some(reread_full);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        lines(&e)
    );
    assert_eq!(*CALLS.lock().unwrap(), ["spare"]);
    assert_eq!(ino_at(&e.live()), Some(third), "third, read now, is chosen");
}

fn measure_live_future(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(snap, &[("spare", rd(0.0, 10.0, None))]);
    let m = decide_measured(ctx, snap, rd(90.0, 50.0, Some(at + 600)), probes, at)?;
    assert_eq!(
        m.decision,
        Decision::Stay,
        "a reading from the future is stale"
    );
    Ok(m)
}

#[cfg(target_os = "linux")]
#[test]
fn d5_a_live_reading_time_after_now_is_stale() {
    let e = env();
    let mut c = ctx_at(&e, measure_live_future, settings(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::StayedUnread
    );
}

/// D12: the decision reads the injected clock, not the system clock. With
/// the clock set an hour ahead, a reading taken "now" by the system clock
/// is an hour old.
#[test]
fn d12_decide_measured_reads_the_injected_clock() {
    let e = env();
    let mut c = ctx_with(&e, unused);
    set_now(now() + 3600);
    c.clock = clock;
    let snap = super::observe::snapshot(
        &c,
        &crate::commands::profile::auto_switch::kit::State::new(c.store.root()).unwrap(),
    )
    .unwrap();
    let probes = probes_for(&snap, &[("spare", rd(0.0, 10.0, None))]);
    let m = decide_measured(&c, &snap, rd(90.0, 50.0, Some(now())), probes, clock()).unwrap();
    assert_eq!(m.decision, Decision::Stay);
    assert!(
        m.fresh_lines
            .iter()
            .any(|l| l.starts_with("stay: live reading stale age=")),
        "{:?}",
        m.fresh_lines
    );
}
