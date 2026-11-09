//! The clock `codex-resume` waits on: one sleep until the earliest reset.
//! Tests use [`TestClock`], which records the deadline and moves time.

/// Time, and one wait until a deadline.
pub(super) trait Clock {
    /// Unix epoch seconds.
    fn now(&self) -> i64;
    /// Returns at `deadline` (epoch seconds), at once when it has passed.
    fn sleep_until(&self, deadline: i64);
}

/// The system clock: a single `sleep`, only when the deadline is ahead.
pub(super) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        crate::commands::profile::auto_switch::kit::now()
    }

    fn sleep_until(&self, deadline: i64) {
        let left = deadline.saturating_sub(self.now());
        if left > 0 {
            std::thread::sleep(std::time::Duration::from_secs(left.unsigned_abs()));
        }
    }
}

/// A clock for tests: `sleep_until` records the deadline and moves time to
/// it, never backwards.
#[cfg(test)]
pub(super) struct TestClock {
    now: std::cell::Cell<i64>,
    sleeps: std::cell::RefCell<Vec<i64>>,
}

#[cfg(test)]
impl TestClock {
    pub(super) fn new(now: i64) -> Self {
        Self {
            now: std::cell::Cell::new(now),
            sleeps: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Every deadline slept until, in order.
    pub(super) fn sleeps(&self) -> Vec<i64> {
        self.sleeps.borrow().clone()
    }
}

#[cfg(test)]
impl Clock for TestClock {
    fn now(&self) -> i64 {
        self.now.get()
    }

    fn sleep_until(&self, deadline: i64) {
        self.sleeps.borrow_mut().push(deadline);
        if deadline > self.now.get() {
            self.now.set(deadline);
        }
    }
}
