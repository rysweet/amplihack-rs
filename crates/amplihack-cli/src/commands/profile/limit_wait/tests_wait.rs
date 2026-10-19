//! `wait` and `line` (issue #1618), driven by a fake `WaitDeps` with its own
//! virtual clock: the order of checks in each pass (signal, deadline, login
//! fingerprint, progress), sleep slices of at most one second, the 30-second
//! fingerprint poll, the 60-second progress line, and the exact log lines.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_wait;`.

use super::plan::{Bound, Plan};
use super::wait::{End, WaitDeps, wait};
use super::{Event, Skip, line};
use crate::commands::recipe::run::limit_patterns::Harness;
use std::time::Duration;

/// 2026-10-18 08:00:00 -07:00.
const T0: i64 = 1_792_335_600;
const PREFIX: &str = "amplihack.recipe.usage_limit_wait ";

/// Login fingerprint schedule: from virtual second `.0` (relative to T0) on,
/// the probe returns `.1`.
type Schedule = Vec<(i64, Option<u8>)>;

struct Fake {
    ms: i64,
    prints: Schedule,
    /// Virtual second from which a signal is pending.
    signal_from: Option<(i64, i32)>,
    signal_seen_at_ms: Option<i64>,
    sleeps: Vec<Duration>,
    sleeps_after_signal: usize,
    probes: Vec<i64>,
    logs: Vec<String>,
}

impl Fake {
    fn new() -> Self {
        Self {
            ms: 0,
            prints: vec![(0, Some(1))],
            signal_from: None,
            signal_seen_at_ms: None,
            sleeps: Vec::new(),
            sleeps_after_signal: 0,
            probes: Vec::new(),
            logs: Vec::new(),
        }
    }

    fn elapsed(&self) -> i64 {
        self.ms.div_euclid(1000)
    }

    fn progress(&self) -> Vec<&str> {
        self.logs
            .iter()
            .map(String::as_str)
            .filter(|l| l.contains(" progress "))
            .collect()
    }
}

impl WaitDeps for Fake {
    type Print = u8;

    fn now(&mut self) -> i64 {
        T0 + self.elapsed()
    }

    fn sleep(&mut self, d: Duration) {
        assert!(
            d <= Duration::from_secs(1),
            "slice of {d:?} is over one second"
        );
        assert!(!d.is_zero(), "a zero sleep would spin");
        assert!(self.sleeps.len() < 200_000, "the wait never ended");
        if self.signal_seen_at_ms.is_some() {
            self.sleeps_after_signal += 1;
        }
        self.sleeps.push(d);
        self.ms += d.as_millis() as i64;
    }

    fn fingerprint(&mut self) -> Option<u8> {
        let t = self.elapsed();
        self.probes.push(t);
        self.prints
            .iter()
            .rev()
            .find(|(from, _)| *from <= t)
            .and_then(|(_, p)| *p)
    }

    fn signal(&mut self) -> Option<i32> {
        let (from, sig) = self.signal_from?;
        if self.ms >= from * 1000 {
            self.signal_seen_at_ms.get_or_insert(self.ms);
            Some(sig)
        } else {
            None
        }
    }

    fn log(&mut self, line: &str) {
        self.logs.push(line.to_string());
    }
}

fn plan_in(secs: i64, bound: Bound) -> Plan {
    Plan {
        until: T0 + secs,
        bound,
        reset: None,
    }
}

#[test]
fn a_zero_length_wait_ends_at_once_without_sleeping() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&plan_in(0, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
    assert!(f.sleeps.is_empty(), "{:?}", f.sleeps);
    assert!(f.progress().is_empty());
}

#[test]
fn each_bound_maps_to_its_end_reason() {
    for (bound, want) in [
        (Bound::ResetMargin, End::ResetReached),
        (Bound::Default, End::ResetReached),
        (Bound::MaxWait, End::MaxReached),
        (Bound::StepTimeout, End::StepTimeout),
    ] {
        let mut f = Fake::new();
        assert_eq!(wait(&plan_in(75, bound), &mut f), want, "{bound:?}");
        assert!(
            f.elapsed() >= 75,
            "{bound:?} ended early at {}",
            f.elapsed()
        );
        assert!(f.elapsed() <= 76, "{bound:?} overslept to {}", f.elapsed());
        let mut f = Fake::new();
        assert_eq!(wait(&plan_in(0, bound), &mut f), want, "{bound:?} zero");
    }
}

#[test]
fn sleeps_are_slices_of_at_most_one_second() {
    let mut f = Fake::new();
    wait(&plan_in(300, Bound::Default), &mut f);
    assert!(!f.sleeps.is_empty());
    // The assertions in `Fake::sleep` cover each slice; the total is the wait.
    let total: Duration = f.sleeps.iter().sum();
    assert!(total >= Duration::from_secs(300) && total < Duration::from_secs(302));
}

#[test]
fn a_progress_line_is_written_every_sixty_seconds() {
    let mut f = Fake::new();
    wait(&plan_in(150, Bound::ResetMargin), &mut f);
    assert_eq!(
        f.progress(),
        vec![
            "amplihack.recipe.usage_limit_wait progress remaining=90s",
            "amplihack.recipe.usage_limit_wait progress remaining=30s",
        ]
    );
    // `wait` writes only progress lines; start, end and resume are the
    // caller's.
    assert_eq!(f.logs.len(), 2, "{:?}", f.logs);
}

#[test]
fn the_fingerprint_is_taken_at_the_start_and_every_thirty_seconds() {
    let mut f = Fake::new();
    wait(&plan_in(100, Bound::Default), &mut f);
    let mut distinct = f.probes.clone();
    distinct.dedup();
    assert_eq!(distinct, vec![0, 30, 60, 90]);
}

#[test]
fn a_changed_login_ends_the_wait_at_the_next_poll() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    assert_eq!(
        wait(&plan_in(1800, Bound::Default), &mut f),
        End::LoginChanged
    );
    assert_eq!(f.elapsed(), 60);
}

#[test]
fn a_part_that_appears_or_disappears_counts_as_a_change() {
    for prints in [
        vec![(0, None), (10, Some(1))],
        vec![(0, Some(1)), (10, None)],
    ] {
        let mut f = Fake::new();
        f.prints = prints.clone();
        assert_eq!(
            wait(&plan_in(1800, Bound::Default), &mut f),
            End::LoginChanged,
            "{prints:?}"
        );
        assert_eq!(f.elapsed(), 30);
    }
}

#[test]
fn an_absent_fingerprint_that_stays_absent_is_no_change() {
    let mut f = Fake::new();
    f.prints = vec![(0, None)];
    assert_eq!(
        wait(&plan_in(200, Bound::Default), &mut f),
        End::ResetReached
    );
}

#[test]
fn a_signal_ends_the_wait_within_one_slice() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let mut f = Fake::new();
        f.signal_from = Some((10, sig));
        assert_eq!(
            wait(&plan_in(1800, Bound::Default), &mut f),
            End::Signal(sig)
        );
        assert_eq!(f.sleeps_after_signal, 0, "slept after seeing the signal");
        assert!(f.elapsed() <= 11, "noticed at {}", f.elapsed());
    }
}

#[test]
fn a_pending_signal_wins_over_a_passed_deadline_and_a_changed_login() {
    let mut f = Fake::new();
    f.signal_from = Some((0, libc::SIGTERM));
    assert_eq!(
        wait(&plan_in(0, Bound::ResetMargin), &mut f),
        End::Signal(libc::SIGTERM)
    );
    let mut f = Fake::new();
    f.signal_from = Some((30, libc::SIGINT));
    f.prints = vec![(0, Some(1)), (20, Some(2))];
    assert_eq!(
        wait(&plan_in(1800, Bound::Default), &mut f),
        End::Signal(libc::SIGINT)
    );
}

#[test]
fn a_passed_deadline_wins_over_a_changed_login() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    assert_eq!(wait(&plan_in(60, Bound::MaxWait), &mut f), End::MaxReached);
}

#[test]
fn end_reasons_match_the_log_table() {
    assert_eq!(End::ResetReached.as_str(), "reset reached");
    assert_eq!(End::MaxReached.as_str(), "max reached");
    assert_eq!(End::StepTimeout.as_str(), "step timeout");
    assert_eq!(End::LoginChanged.as_str(), "login changed");
    assert_eq!(End::Signal(libc::SIGHUP).as_str(), "signal");
}

// --- line ----------------------------------------------------------------

const LA: chrono_tz::Tz = chrono_tz::America::Los_Angeles;

fn start(reset: Option<i64>, until: i64, bound: Bound) -> Event {
    Event::Start {
        harness: Harness::Claude,
        reset,
        until,
        bound,
    }
}

#[test]
fn start_line_matches_the_documented_example() {
    assert_eq!(
        line(
            &start(Some(1_792_337_400), 1_792_337_520, Bound::ResetMargin),
            &LA
        ),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit harness=claude \
         reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 08:32:00 -07:00 bound=reset+margin"
    );
}

#[test]
fn start_line_says_unparsed_when_no_reset_was_read() {
    assert_eq!(
        line(&start(None, T0 + 1800, Bound::Default), &LA),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit harness=claude \
         reset=unparsed until=2026-10-18 08:30:00 -07:00 bound=default"
    );
}

#[test]
fn times_use_the_zone_offset_with_a_colon() {
    let l = line(
        &start(None, T0, Bound::StepTimeout),
        &chrono_tz::Asia::Kolkata,
    );
    assert!(
        l.ends_with("until=2026-10-18 20:30:00 +05:30 bound=step_timeout"),
        "{l}"
    );
}

#[test]
fn every_other_event_has_its_documented_fields() {
    let cases = [
        (
            Event::Progress { remaining: 1740 },
            "progress remaining=1740s",
        ),
        (Event::End(End::ResetReached), "end reason=reset reached"),
        (Event::End(End::LoginChanged), "end reason=login changed"),
        (Event::End(End::MaxReached), "end reason=max reached"),
        (Event::End(End::StepTimeout), "end reason=step timeout"),
        (Event::End(End::Signal(libc::SIGTERM)), "end reason=signal"),
        (Event::Resume { attempt: 2 }, "resume attempt=2"),
        (Event::Skipped(Skip::Disabled), "skipped reason=disabled"),
        (
            Event::Skipped(Skip::AlreadyWaited),
            "skipped reason=already_waited",
        ),
        (
            Event::Skipped(Skip::NotSupervised),
            "skipped reason=not_supervised",
        ),
    ];
    for (event, rest) in cases {
        assert_eq!(line(&event, &LA), format!("{PREFIX}{rest}"));
    }
}

#[test]
fn every_line_is_one_printable_ascii_line_even_for_extreme_values() {
    let events = [
        start(Some(i64::MAX), i64::MIN, Bound::ResetMargin),
        start(Some(i64::MIN), i64::MAX, Bound::MaxWait),
        start(Some(0), 0, Bound::Default),
        Event::Progress {
            remaining: u64::MAX,
        },
        Event::Resume { attempt: u32::MAX },
        Event::End(End::Signal(i32::MIN)),
    ];
    for e in &events {
        let l = line(e, &LA);
        assert!(l.starts_with(PREFIX), "{l}");
        assert!(
            l.bytes().all(|b| (0x20..0x7f).contains(&b)),
            "non-printable byte in {l:?}"
        );
    }
}
