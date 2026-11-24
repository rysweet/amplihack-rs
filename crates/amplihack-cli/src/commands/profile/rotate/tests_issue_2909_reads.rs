//! Issue #2909, "Wanted" item 3: what an event cycle takes from the readings
//! a watch keeps (`cycle::event_read::Stored`). Pure values: no probe, no
//! lock, no daemon. The daemon-level behaviour (an event cycle reads the
//! live login and the chosen target only; an interval cycle reads every
//! login) is in `tests/issue_2909_limit_event.rs`.
//!
//! Contract:
//! - `Stored::keep(live, probes, measured_at)` keeps each probe that has a
//!   reading time (`fresh::read_at`), the live probe under `live`. A probe
//!   with none leaves what was kept for its login as it was.
//! - `Stored::usable(name, now, max_age)` gives the kept reading, marked
//!   `carried` with `observed_at` its reading time, only while it is no more
//!   than `max_age` old and not dated after `now`, reports a window, and
//!   every window it reports resets after `now`.
//! - `Stored::split(candidates, now, max_age)` splits into the logins to
//!   read and the usable readings, each in the candidates' order.
//! - A usable reading has no reading time in the cycle that takes it, so the
//!   freshness gate counts it stale and reads the login before a swap to it.
//! - `Reason::event`: a limit event and a reset narrow the next cycle; the
//!   interval does not.

use super::super::super::decide::{Probed, Reading, Win, Windows};
use super::super::super::fresh::{read_at, stale};
use super::super::super::log::wake_msg::{self, Reason};
use super::Stored;
use crate::commands::profile::name::ProfileName;

const NOW: i64 = 1_793_389_376;
const HOUR: i64 = 3600;
const INTERVAL: i64 = 600;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

/// A probe read directly in its cycle (no `observed_at`), with both
/// windows resetting at `resets`.
fn direct(session: f64, week: f64, resets: Option<i64>) -> Probed {
    let reading = if session >= 100.0 || week >= 100.0 {
        Reading::Exhausted {
            session: Some(session),
            week: Some(week),
        }
    } else {
        Reading::Measured { session, week }
    };
    Probed {
        reading,
        windows: Windows {
            session: Some(Win {
                used: session,
                resets_at: resets,
            }),
            week: Some(Win {
                used: week,
                resets_at: resets,
            }),
            no_session_limit: false,
        },
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at: None,
        carried: false,
        passive: false,
    }
}

/// `p` kept for `name` by a cycle that measured from `measured_at`.
fn kept(name: &str, p: Probed, measured_at: i64) -> Stored {
    let mut s = Stored::default();
    s.keep(
        None,
        Some(&(Probed::failed(), vec![(n(name), p)])),
        measured_at,
    );
    s
}

#[test]
fn a_direct_reading_is_kept_at_the_time_its_cycle_measured_from() {
    let mut s = Stored::default();
    let live = direct(100.0, 40.0, Some(NOW + HOUR));
    let spare = direct(10.0, 20.0, Some(NOW + HOUR));
    let probes = (live.clone(), vec![(n("spare"), spare.clone())]);
    s.keep(Some(&n("work")), Some(&probes), NOW - 60);
    let got = s.usable(&n("spare"), NOW, INTERVAL).expect("spare stands");
    assert_eq!(got.reading, spare.reading);
    assert_eq!(got.windows, spare.windows);
    assert!(got.carried, "a stored reading is an earlier cycle's");
    assert_eq!(got.observed_at, Some(NOW - 60), "dated by its real age");
    // The live probe is kept under the live login's name: once it is moved
    // off, it is a candidate whose full reading still stands.
    let work = s.usable(&n("work"), NOW, INTERVAL).expect("work stands");
    assert_eq!(work.reading, live.reading);
}

#[test]
fn a_passive_reading_is_kept_at_its_own_time_and_stays_passive() {
    let mut p = direct(10.0, 20.0, Some(NOW + HOUR));
    p.observed_at = Some(NOW - 200);
    p.passive = true;
    let s = kept("spare", p, NOW - 60);
    let got = s.usable(&n("spare"), NOW, INTERVAL).unwrap();
    assert_eq!(got.observed_at, Some(NOW - 200));
    assert!(got.passive, "the source is not rewritten");
}

#[test]
fn a_probe_with_no_reading_time_leaves_the_kept_reading_as_it_was() {
    let mut s = kept("spare", direct(10.0, 20.0, Some(NOW + HOUR)), NOW - 60);
    let mut carried = direct(90.0, 90.0, Some(NOW + HOUR));
    carried.carried = true;
    let mut running = Probed::failed();
    running.still_running = true;
    let mut unknown = direct(90.0, 90.0, Some(NOW + HOUR));
    unknown.unknown = true;
    for p in [Probed::failed(), carried, running, unknown] {
        s.keep(None, Some(&(Probed::failed(), vec![(n("spare"), p)])), NOW);
    }
    let got = s.usable(&n("spare"), NOW, INTERVAL).unwrap();
    assert_eq!(got.observed_at, Some(NOW - 60));
    assert_eq!(
        got.reading,
        Reading::Measured {
            session: 10.0,
            week: 20.0
        }
    );
    // Nothing kept, and a failed read keeps nothing.
    let s = kept("extra", Probed::failed(), NOW);
    assert!(s.usable(&n("extra"), NOW, INTERVAL).is_none());
    assert!(s.usable(&n("never-read"), NOW, INTERVAL).is_none());
}

#[test]
fn a_reading_older_than_the_interval_or_dated_after_now_does_not_stand() {
    let p = || direct(10.0, 20.0, Some(NOW + HOUR));
    let at_bound = kept("spare", p(), NOW - INTERVAL);
    assert!(at_bound.usable(&n("spare"), NOW, INTERVAL).is_some());
    let too_old = kept("spare", p(), NOW - INTERVAL - 1);
    assert!(too_old.usable(&n("spare"), NOW, INTERVAL).is_none());
    let future = kept("spare", p(), NOW + 5);
    assert!(future.usable(&n("spare"), NOW, INTERVAL).is_none());
}

#[test]
fn a_window_that_has_reset_or_has_no_known_reset_is_read_again() {
    // Reset exactly now: the window may have room again.
    let reset_now = kept("spare", direct(100.0, 20.0, Some(NOW)), NOW - 60);
    assert!(reset_now.usable(&n("spare"), NOW, INTERVAL).is_none());
    // No reset Claude printed: nothing says when the figure stops holding.
    let unknown = kept("spare", direct(10.0, 20.0, None), NOW - 60);
    assert!(unknown.usable(&n("spare"), NOW, INTERVAL).is_none());
    // The session still open but the week reset: read again.
    let mut p = direct(10.0, 100.0, Some(NOW + HOUR));
    p.windows.week = Some(Win {
        used: 100.0,
        resets_at: Some(NOW - 1),
    });
    let week_reset = kept("spare", p, NOW - 60);
    assert!(week_reset.usable(&n("spare"), NOW, INTERVAL).is_none());
    // A weekly-only login reports no session window; its week decides.
    let mut weekly = direct(0.0, 30.0, Some(NOW + HOUR));
    weekly.windows.session = None;
    weekly.windows.no_session_limit = true;
    let weekly = kept("spare", weekly, NOW - 60);
    assert!(weekly.usable(&n("spare"), NOW, INTERVAL).is_some());
}

#[test]
fn an_exhausted_reading_stands_until_its_reset_and_not_after() {
    let full = || direct(100.0, 40.0, Some(NOW + 30));
    let s = kept("work", full(), NOW - 60);
    let got = s.usable(&n("work"), NOW, INTERVAL).unwrap();
    assert!(matches!(got.reading, Reading::Exhausted { .. }));
    assert!(s.usable(&n("work"), NOW + 30, INTERVAL).is_none());
}

#[test]
fn a_reading_with_no_window_never_stands() {
    // The API key's synthetic reading: never probed, and no window.
    let s = kept("key", Probed::platform(), NOW - 60);
    assert!(s.usable(&n("key"), NOW, INTERVAL).is_none());
}

#[test]
fn split_keeps_the_candidates_order() {
    let mut s = Stored::default();
    let probes = (
        Probed::failed(),
        vec![
            (n("a"), direct(10.0, 10.0, Some(NOW + HOUR))),
            (n("c"), direct(30.0, 30.0, Some(NOW + HOUR))),
        ],
    );
    s.keep(None, Some(&probes), NOW - 60);
    let (read, stored) = s.split(&[n("a"), n("b"), n("c")], NOW, INTERVAL);
    assert_eq!(read, vec![n("b")]);
    let names: Vec<&str> = stored.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["a", "c"]);
}

#[test]
fn a_stored_reading_is_never_a_swap_target_without_a_read() {
    let s = kept("spare", direct(10.0, 20.0, Some(NOW + HOUR)), NOW - 1);
    let p = s.usable(&n("spare"), NOW, INTERVAL).unwrap();
    // No reading time in the cycle that takes it, however recent it is, so
    // the freshness gate reads the login again before a swap to it.
    assert_eq!(read_at(&p, NOW), None);
    assert!(stale(read_at(&p, NOW), Some(NOW), None, NOW));
}

#[test]
fn only_a_limit_event_or_a_reset_narrows_the_next_cycle() {
    assert!(Reason::Ledger.event());
    assert!(Reason::Reset.event());
    assert!(!Reason::Interval.event());
}

#[test]
fn the_stored_line_is_fixed_text_with_two_counts() {
    assert_eq!(
        wake_msg::stored(1, 3),
        "wake: read 1 of 3 logins, the others from their last reading"
    );
}
