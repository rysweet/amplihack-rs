//! Fake clocks for tests of code that reads an injected
//! `amplihack_utils::clock::Clock` (issue #2019).
//!
//! There is one copy. Each test crate that needs it includes this file with
//! `#[path]`, as it includes `wait.rs`. It uses only `std` and
//! `amplihack_utils`.
//!
//! - [`VirtualClock`] starts at a base `Instant`, plus an offset that moves
//!   only through [`VirtualClock::advance`] or `sleep`. `sleep(d)` records
//!   its virtual start and `d`, moves the offset by `d`, runs the hook set
//!   by [`VirtualClock::on_sleep`], and returns at once. Use it when time is
//!   the subject, such as "the commit fails only after 5 s". A test that
//!   never advances it, and calls code that never sleeps, has time standing
//!   still.
//! - [`FrozenClock`] never moves, and its `sleep(d)` really sleeps for `d`.
//!   Use it when time is not the subject and a slow host must not run a
//!   deadline out, such as threads racing for a lock. A wait on it has no
//!   end while the event it waits for does not come, so collect results
//!   under `wait::CEILING`. It sleeps rather than yields, so a waiter uses
//!   no CPU while another thread holds what it waits for.
//! - [`PanicOnSleep`] never moves, and fails the test if anything sleeps on
//!   it. Use it to prove a path returns without waiting at all.

#![allow(dead_code)]

use amplihack_utils::clock::Clock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Run after each sleep with the number of sleeps so far (the first is 1)
/// and the virtual time the sleep ended at.
type OnSleep = Box<dyn FnMut(usize, Duration) + Send>;

/// Virtual time that moves only when a test advances it or something sleeps
/// on it. It never sleeps for real.
pub struct VirtualClock {
    base: Instant,
    offset_ns: AtomicU64,
    sleeps: Mutex<Vec<(Duration, Duration)>>,
    on_sleep: Mutex<Option<OnSleep>>,
}

impl Default for VirtualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualClock {
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            offset_ns: AtomicU64::new(0),
            sleeps: Mutex::new(Vec::new()),
            on_sleep: Mutex::new(None),
        }
    }

    /// A new clock behind an `Arc`, for a test that hands it to a thread or
    /// a fake server as well as to the code under test.
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Virtual time since the clock was made.
    pub fn elapsed(&self) -> Duration {
        Duration::from_nanos(self.offset_ns.load(Ordering::SeqCst))
    }

    /// Moves virtual time on by `d` without counting as a sleep, as when
    /// "the token endpoint took 300 ms".
    pub fn advance(&self, d: Duration) {
        let ns = u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
        self.offset_ns.fetch_add(ns, Ordering::SeqCst);
    }

    /// `(virtual start, length)` of every sleep, in order.
    pub fn sleeps(&self) -> Vec<(Duration, Duration)> {
        self.sleeps.lock().unwrap().clone()
    }

    /// Runs `f` after each sleep, once virtual time has moved, with the
    /// number of sleeps so far and the virtual time reached. A test uses it
    /// to make an event happen at a known point of virtual time, such as a
    /// lock holder letting go.
    pub fn on_sleep(&self, f: impl FnMut(usize, Duration) + Send + 'static) {
        *self.on_sleep.lock().unwrap() = Some(Box::new(f));
    }
}

impl Clock for VirtualClock {
    fn now(&self) -> Instant {
        self.base + self.elapsed()
    }

    fn sleep(&self, d: Duration) {
        let n = {
            let mut sleeps = self.sleeps.lock().unwrap();
            sleeps.push((self.elapsed(), d));
            sleeps.len()
        };
        self.advance(d);
        if let Some(f) = self.on_sleep.lock().unwrap().as_mut() {
            f(n, self.elapsed());
        }
    }
}

/// Time that never moves, so a deadline measured on it never runs out.
/// `sleep(d)` sleeps for real, so a retry loop on it waits without
/// spinning.
pub struct FrozenClock(Instant);

impl FrozenClock {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self(Instant::now()))
    }
}

impl Clock for FrozenClock {
    fn now(&self) -> Instant {
        self.0
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// Time that never moves, and a `sleep` that fails the test.
pub struct PanicOnSleep(Instant);

impl Default for PanicOnSleep {
    fn default() -> Self {
        Self::new()
    }
}

impl PanicOnSleep {
    pub fn new() -> Self {
        Self(Instant::now())
    }
}

impl Clock for PanicOnSleep {
    fn now(&self) -> Instant {
        self.0
    }

    fn sleep(&self, d: Duration) {
        panic!("sleep({d:?}) called on a path that must not wait");
    }
}
