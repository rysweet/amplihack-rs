//! Issue #2019: an injectable clock for the exclusive-lock wait.
//!
//! Contract:
//! - `amplihack_utils::clock::Clock` is `Send + Sync` and object safe, with
//!   `now() -> Instant` and `sleep(Duration)`. `SystemClock` is the only
//!   implementation shipped in production code.
//! - `secure_files::lock_exclusive_timeout_with(file, timeout, &dyn Clock)`
//!   is the `flock` retry loop with all time taken from the clock: it
//!   retries every 100 ms (or the time left, if less) and gives up with
//!   `ErrorKind::TimedOut` once the clock says `timeout` has passed. A zero
//!   timeout tries once and never sleeps.
//! - `lock_exclusive_timeout(file, timeout)` keeps its signature and
//!   behaviour; it is `lock_exclusive_timeout_with(.., &SystemClock)`.
//!
//! No test here asserts on real elapsed time: every wait is measured on a
//! fake clock whose `sleep` only moves virtual time forward.

#![cfg(unix)]

#[path = "../../amplihack-cli/tests/support/virtual_clock.rs"]
mod virtual_clock;

use amplihack_utils::clock::{Clock, SystemClock};
use amplihack_utils::secure_files::{
    ExclusiveLock, lock_exclusive_timeout, lock_exclusive_timeout_with,
};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use virtual_clock::{PanicOnSleep, VirtualClock};

const RETRY: Duration = Duration::from_millis(100);

fn open(p: &Path) -> fs::File {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(p)
        .unwrap()
}

/// A lock file in a fresh directory, already held through its own open
/// file description (so a second `open` of the same path conflicts).
fn held_lock() -> (tempfile::TempDir, std::path::PathBuf, ExclusiveLock) {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("lock");
    let held = lock_exclusive_timeout(open(&p), Duration::ZERO).expect("first lock is free");
    (t, p, held)
}

#[test]
fn clock_is_object_safe_send_and_sync() {
    fn send_sync<T: Send + Sync + ?Sized>() {}
    send_sync::<dyn Clock>();
    send_sync::<SystemClock>();
    let c: std::sync::Arc<dyn Clock> = std::sync::Arc::new(SystemClock);
    let a = c.now();
    let b = c.now();
    assert!(b >= a, "SystemClock::now is monotonic");
}

#[test]
fn a_free_lock_is_taken_at_once_without_sleeping() {
    let t = tempfile::tempdir().unwrap();
    let clock = PanicOnSleep::new();
    let lock = lock_exclusive_timeout_with(open(&t.path().join("lock")), RETRY * 50, &clock)
        .expect("a free lock is taken");
    drop(lock);
}

#[test]
fn a_held_lock_times_out_only_once_virtual_time_reaches_the_timeout() {
    let (_t, p, held) = held_lock();
    let clock = VirtualClock::new();
    let timeout = Duration::from_secs(1);
    let err = lock_exclusive_timeout_with(open(&p), timeout, &clock).expect_err("the lock is held");
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert_eq!(err.to_string(), "timed out waiting for an exclusive lock");
    let sleeps = clock.sleeps();
    assert_eq!(sleeps.len(), 10, "one retry every 100 ms: {sleeps:?}");
    assert!(sleeps.iter().all(|&(_, d)| d == RETRY), "{sleeps:?}");
    let (last_start, _) = *sleeps.last().unwrap();
    assert!(
        last_start < timeout,
        "the last wait starts before the timeout"
    );
    assert_eq!(
        clock.elapsed(),
        timeout,
        "it gives up exactly at the timeout"
    );
    drop(held);
}

#[test]
fn the_last_wait_is_cut_to_the_time_left() {
    let (_t, p, held) = held_lock();
    let clock = VirtualClock::new();
    let timeout = Duration::from_millis(250);
    let err = lock_exclusive_timeout_with(open(&p), timeout, &clock).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    let lengths: Vec<_> = clock.sleeps().into_iter().map(|(_, d)| d).collect();
    assert_eq!(
        lengths,
        [RETRY, RETRY, Duration::from_millis(50)],
        "never sleeps past the timeout"
    );
    assert_eq!(clock.elapsed(), timeout);
    drop(held);
}

#[test]
fn a_zero_timeout_tries_once_and_never_sleeps() {
    let (_t, p, held) = held_lock();
    let clock = VirtualClock::new();
    let err = lock_exclusive_timeout_with(open(&p), Duration::ZERO, &clock).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert!(clock.sleeps().is_empty(), "{:?}", clock.sleeps());
    assert_eq!(clock.elapsed(), Duration::ZERO);
    drop(held);
}

#[test]
fn a_lock_released_during_the_wait_is_taken_on_the_next_try() {
    let (_t, p, held) = held_lock();
    let clock = VirtualClock::new();
    let holder = std::sync::Arc::new(Mutex::new(Some(held)));
    let h = std::sync::Arc::clone(&holder);
    clock.on_sleep(move |n, _| {
        if n == 3 {
            h.lock().unwrap().take();
        }
    });
    let lock = lock_exclusive_timeout_with(open(&p), Duration::from_secs(5), &clock)
        .expect("taken once the holder lets go");
    assert_eq!(
        clock.sleeps().len(),
        3,
        "retried until released, then stopped"
    );
    assert_eq!(clock.elapsed(), RETRY * 3);
    drop(lock);
}

#[test]
fn a_clock_that_runs_far_past_the_timeout_does_not_panic() {
    // `waited` is compared with the timeout before `timeout - waited` is
    // computed, so a clock that jumps past the timeout cannot underflow.
    struct Jumping {
        base: Instant,
        calls: AtomicU64,
    }
    impl Clock for Jumping {
        fn now(&self) -> Instant {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                self.base
            } else {
                self.base + Duration::from_secs(3600)
            }
        }
        fn sleep(&self, _d: Duration) {}
    }
    let (_t, p, held) = held_lock();
    let clock = Jumping {
        base: Instant::now(),
        calls: AtomicU64::new(0),
    };
    let err =
        lock_exclusive_timeout_with(open(&p), Duration::from_millis(200), &clock).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    drop(held);
}

#[test]
fn the_system_clock_wrapper_keeps_its_behaviour() {
    let (_t, p, held) = held_lock();
    let err = lock_exclusive_timeout(open(&p), Duration::ZERO).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert_eq!(err.to_string(), "timed out waiting for an exclusive lock");
    drop(held);
    let lock = lock_exclusive_timeout(open(&p), Duration::ZERO).expect("free after release");
    drop(lock);
}
