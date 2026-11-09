//! Issue #2019: `wait_for_refreshes_on` waits on an injected clock.
//!
//! Contract:
//!
//! ```ignore
//! pub(crate) fn wait_for_refreshes_on(
//!     counter: &AtomicUsize,
//!     limit: Duration,
//!     clock: &dyn Clock,
//! ) -> bool
//! ```
//!
//! - `true` once `counter` is 0; a counter already at 0 returns at once
//!   without calling `clock.sleep`.
//! - `false` once `limit` has passed on `clock` (not before), with the
//!   counter still above 0.
//! - Between checks it sleeps `POLL` on `clock`, never on the real clock.
//! - `wait_for_refreshes(limit) -> bool` keeps its signature and calls it with
//!   the process-wide `REFRESHING`, `SystemClock` and `POLL`.
//!
//! No test here measures or spends real time.

use super::usage_direct::{wait_for_refreshes, wait_for_refreshes_on};
use amplihack_utils::clock::Clock;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// A clock whose `sleep` fails the test: proves a path never waits.
struct PanicOnSleep(Instant);

impl Clock for PanicOnSleep {
    fn now(&self) -> Instant {
        self.0
    }
    fn sleep(&self, d: Duration) {
        panic!("sleep({d:?}) called on a path that must not wait");
    }
}

/// Virtual time that moves only by what is slept on it. After `drain_after`
/// sleeps it sets `counter` to 0, standing in for the last refresh finishing.
struct Stepping<'a> {
    base: Instant,
    offset_ns: AtomicU64,
    sleeps: Mutex<Vec<Duration>>,
    counter: &'a AtomicUsize,
    drain_after: Option<usize>,
}

impl<'a> Stepping<'a> {
    fn new(counter: &'a AtomicUsize, drain_after: Option<usize>) -> Self {
        Self {
            base: Instant::now(),
            offset_ns: AtomicU64::new(0),
            sleeps: Mutex::new(Vec::new()),
            counter,
            drain_after,
        }
    }
    fn elapsed(&self) -> Duration {
        Duration::from_nanos(self.offset_ns.load(Ordering::SeqCst))
    }
    fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().unwrap().clone()
    }
}

impl Clock for Stepping<'_> {
    fn now(&self) -> Instant {
        self.base + self.elapsed()
    }
    fn sleep(&self, d: Duration) {
        let n = {
            let mut s = self.sleeps.lock().unwrap();
            s.push(d);
            s.len()
        };
        self.offset_ns
            .fetch_add(u64::try_from(d.as_nanos()).unwrap(), Ordering::SeqCst);
        if self.drain_after == Some(n) {
            self.counter.store(0, Ordering::SeqCst);
        }
    }
}

#[test]
fn a_counter_at_zero_returns_true_without_sleeping() {
    let counter = AtomicUsize::new(0);
    let clock = PanicOnSleep(Instant::now());
    assert!(wait_for_refreshes_on(
        &counter,
        Duration::from_secs(5),
        &clock
    ));
}

#[test]
fn a_counter_at_zero_returns_true_even_with_a_zero_limit() {
    let counter = AtomicUsize::new(0);
    let clock = PanicOnSleep(Instant::now());
    assert!(wait_for_refreshes_on(&counter, Duration::ZERO, &clock));
}

#[test]
fn a_counter_that_drains_returns_true() {
    let counter = AtomicUsize::new(2);
    let clock = Stepping::new(&counter, Some(3));
    assert!(wait_for_refreshes_on(
        &counter,
        Duration::from_secs(5),
        &clock
    ));
    assert_eq!(
        clock.sleeps().len(),
        3,
        "it stops polling once the count is 0"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[test]
fn a_counter_that_never_drains_returns_false_at_the_limit() {
    let counter = AtomicUsize::new(1);
    let clock = Stepping::new(&counter, None);
    let limit = Duration::from_secs(5);
    assert!(!wait_for_refreshes_on(&counter, limit, &clock));
    let spent = clock.elapsed();
    assert!(
        spent >= limit,
        "gave up at {spent:?} of virtual time, before {limit:?}"
    );
    let last = *clock.sleeps().last().expect("a held counter is waited for");
    assert!(
        spent < limit + last,
        "waited {spent:?} of virtual time, more than one poll past {limit:?}"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "the counter is only read"
    );
}

#[test]
fn a_counter_that_drains_just_before_the_limit_still_returns_true() {
    // Drains on the poll that brings virtual time to just under the limit:
    // the waiter must not give up early.
    let counter = AtomicUsize::new(1);
    let probe = Stepping::new(&counter, None);
    let limit = Duration::from_secs(1);
    assert!(!wait_for_refreshes_on(&counter, limit, &probe));
    let polls_to_limit = probe.sleeps().len();
    assert!(polls_to_limit >= 2, "the limit spans several polls");

    let counter = AtomicUsize::new(1);
    let clock = Stepping::new(&counter, Some(polls_to_limit - 1));
    assert!(wait_for_refreshes_on(&counter, limit, &clock));
}

#[test]
fn every_wait_is_one_poll_on_the_injected_clock() {
    let counter = AtomicUsize::new(1);
    let clock = Stepping::new(&counter, Some(4));
    assert!(wait_for_refreshes_on(
        &counter,
        Duration::from_secs(5),
        &clock
    ));
    let sleeps = clock.sleeps();
    assert!(!sleeps.is_empty());
    let first = sleeps[0];
    assert!(first > Duration::ZERO, "a zero poll would spin");
    assert!(
        sleeps.iter().all(|d| *d == first),
        "every poll has the same length: {sleeps:?}"
    );
    // The production poll is unchanged (usage_direct::POLL).
    assert_eq!(first, Duration::from_millis(20));
}

#[test]
fn a_zero_limit_with_a_held_counter_returns_false_without_sleeping() {
    let counter = AtomicUsize::new(1);
    let clock = PanicOnSleep(Instant::now());
    assert!(!wait_for_refreshes_on(&counter, Duration::ZERO, &clock));
}

#[test]
fn a_duration_max_limit_does_not_overflow() {
    let counter = AtomicUsize::new(1);
    let clock = Stepping::new(&counter, Some(2));
    assert!(wait_for_refreshes_on(&counter, Duration::MAX, &clock));
}

#[test]
fn wait_for_refreshes_keeps_its_signature() {
    let _: fn(Duration) -> bool = wait_for_refreshes;
}

/// The wrapper reads the process-wide counter through the injected-clock
/// function, not a copy of the loop.
#[test]
fn wait_for_refreshes_delegates_to_wait_for_refreshes_on_with_the_global_counter() {
    let src = include_str!("usage_direct.rs");
    let start = src
        .find("pub(super) fn wait_for_refreshes(")
        .or_else(|| src.find("pub(crate) fn wait_for_refreshes("))
        .expect("wait_for_refreshes is defined");
    let body = &src[start..];
    let end = body.find("\n}\n").expect("end of wait_for_refreshes");
    let body = &body[..end];
    assert!(
        body.contains("wait_for_refreshes_on(&REFRESHING"),
        "wait_for_refreshes must call wait_for_refreshes_on(&REFRESHING, ..):\n{body}"
    );
    assert!(body.contains("SystemClock"), "{body}");
    assert!(
        !body.contains("Instant::now") && !body.contains("elapsed()"),
        "the wrapper must not keep its own wall-clock loop:\n{body}"
    );
}

#[test]
fn a_timed_out_wait_removes_no_lock_it_does_not_own() {
    // S3.2: when the stop gives up, the refresh lock left in place may by
    // then belong to someone else, here a Claude that took the lock again
    // (a new inode) after ours was gone. The timed-out wait must not touch
    // it, and the late release of our own lock must leave it alone too.
    use super::auto_switch::kit::{REFRESH_LOCK, release_refresh_lock_at, take_refresh_lock_at};
    use std::os::unix::fs::MetadataExt;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let lock = dir.join(REFRESH_LOCK);
    let ours = take_refresh_lock_at(dir).expect("take the refresh lock");
    // The foreign lock is made while ours still exists, so the filesystem
    // cannot hand it our inode, then moved into place.
    let foreign = dir.join("foreign-lock");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::remove_dir(&lock).unwrap();
    std::fs::rename(&foreign, &lock).unwrap();
    let theirs = std::fs::symlink_metadata(&lock).unwrap().ino();
    assert_ne!(ours, theirs, "the foreign lock must be a different inode");

    // A refresh still counted, on a clock that only moves when slept on, so
    // the wait times out in virtual time.
    let counter = AtomicUsize::new(1);
    let clock = Stepping::new(&counter, None);
    let limit = Duration::from_secs(5);
    assert!(!wait_for_refreshes_on(&counter, limit, &clock));
    assert!(clock.elapsed() >= limit, "the timeout is on virtual time");
    assert_eq!(
        std::fs::symlink_metadata(&lock).map(|m| m.ino()).ok(),
        Some(theirs),
        "the timed-out wait removed a lock it did not own"
    );

    assert!(!release_refresh_lock_at(dir, ours));
    assert_eq!(
        std::fs::symlink_metadata(&lock).map(|m| m.ino()).ok(),
        Some(theirs),
        "releasing our inode removed someone else's lock"
    );
}
