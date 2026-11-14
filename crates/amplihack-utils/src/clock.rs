//! A clock that can be injected where code waits on time (issue #2019).
//!
//! Production code uses [`SystemClock`]. Tests pass their own [`Clock`] so a
//! timeout can be checked on virtual time instead of real elapsed time, which
//! a loaded host can stretch without limit. No fake ships here and nothing
//! (no env var, global or config setting) selects a clock at run time.

use std::time::{Duration, Instant};

/// A source of the current time and of sleeps.
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> Instant;
    /// Blocks the calling thread for `d`.
    fn sleep(&self, d: Duration);
}

/// The real clock: [`Instant::now`] and [`std::thread::sleep`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}
