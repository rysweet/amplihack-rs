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
//!
//! No test here measures or spends real time.

use super::usage_direct::wait_for_refreshes_on;
use crate::test_support::virtual_clock::{PanicOnSleep, VirtualClock};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Virtual time that moves only by what is slept on it. After `drain_after`
/// sleeps it sets `counter` to 0, standing in for the last refresh finishing.
fn stepping(counter: &Arc<AtomicUsize>, drain_after: Option<usize>) -> VirtualClock {
    let clock = VirtualClock::new();
    if let Some(last) = drain_after {
        let counter = Arc::clone(counter);
        clock.on_sleep(move |n, _| {
            if n == last {
                counter.store(0, Ordering::SeqCst);
            }
        });
    }
    clock
}

/// The length of each sleep on `clock`, in order.
fn polls(clock: &VirtualClock) -> Vec<Duration> {
    clock.sleeps().into_iter().map(|(_, d)| d).collect()
}

#[test]
fn a_counter_at_zero_returns_true_without_sleeping() {
    let counter = AtomicUsize::new(0);
    let clock = PanicOnSleep::new();
    assert!(wait_for_refreshes_on(
        &counter,
        Duration::from_secs(5),
        &clock
    ));
}

#[test]
fn a_counter_at_zero_returns_true_even_with_a_zero_limit() {
    let counter = AtomicUsize::new(0);
    let clock = PanicOnSleep::new();
    assert!(wait_for_refreshes_on(&counter, Duration::ZERO, &clock));
}

#[test]
fn a_counter_that_drains_returns_true() {
    let counter = Arc::new(AtomicUsize::new(2));
    let clock = stepping(&counter, Some(3));
    assert!(wait_for_refreshes_on(
        &counter,
        Duration::from_secs(5),
        &clock
    ));
    assert_eq!(
        polls(&clock).len(),
        3,
        "it stops polling once the count is 0"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[test]
fn a_counter_that_never_drains_returns_false_at_the_limit() {
    let counter = Arc::new(AtomicUsize::new(1));
    let clock = stepping(&counter, None);
    let limit = Duration::from_secs(5);
    assert!(!wait_for_refreshes_on(&counter, limit, &clock));
    let spent = clock.elapsed();
    assert!(
        spent >= limit,
        "gave up at {spent:?} of virtual time, before {limit:?}"
    );
    let last = *polls(&clock).last().expect("a held counter is waited for");
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
    let counter = Arc::new(AtomicUsize::new(1));
    let probe = stepping(&counter, None);
    let limit = Duration::from_secs(1);
    assert!(!wait_for_refreshes_on(&counter, limit, &probe));
    let polls_to_limit = polls(&probe).len();
    assert!(polls_to_limit >= 2, "the limit spans several polls");

    let counter = Arc::new(AtomicUsize::new(1));
    let clock = stepping(&counter, Some(polls_to_limit - 1));
    assert!(wait_for_refreshes_on(&counter, limit, &clock));
}

#[test]
fn every_wait_is_one_poll_on_the_injected_clock() {
    let counter = Arc::new(AtomicUsize::new(1));
    let clock = stepping(&counter, Some(4));
    assert!(wait_for_refreshes_on(
        &counter,
        Duration::from_secs(5),
        &clock
    ));
    let sleeps = polls(&clock);
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
    let clock = PanicOnSleep::new();
    assert!(!wait_for_refreshes_on(&counter, Duration::ZERO, &clock));
}

#[test]
fn a_duration_max_limit_does_not_overflow() {
    let counter = Arc::new(AtomicUsize::new(1));
    let clock = stepping(&counter, Some(2));
    assert!(wait_for_refreshes_on(&counter, Duration::MAX, &clock));
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
    let counter = Arc::new(AtomicUsize::new(1));
    let clock = stepping(&counter, None);
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
