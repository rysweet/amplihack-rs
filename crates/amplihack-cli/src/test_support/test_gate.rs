//! A gate that holds one side of a test at a known point (issue #2019).
//!
//! A fake server's handler calls [`Gate::arrive_and_wait`]; the test calls
//! [`Gate::wait_arrived`] to know the handler is parked there, asserts what
//! must hold while it is parked, then calls [`Gate::open`]. No step sleeps,
//! and the only time bound is [`CEILING`], which ends a hung test with a
//! message containing "hung" and never decides a pass. A gate used out of
//! order panics with a message containing "gate".

use super::wait::CEILING;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

#[derive(Default)]
struct State {
    arrived: bool,
    open: bool,
}

#[derive(Default)]
pub(crate) struct Gate {
    state: Mutex<State>,
    changed: Condvar,
}

impl Gate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Marks the caller as arrived and blocks until the gate is opened. A
    /// gate that is already open lets the caller straight through.
    pub(crate) fn arrive_and_wait(&self) {
        let mut state = self.lock();
        state.arrived = true;
        self.changed.notify_all();
        let (state, timeout) = self
            .changed
            .wait_timeout_while(state, CEILING, |s| !s.open)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(state);
        assert!(
            !timeout.timed_out(),
            "hung: the gate was never opened within {CEILING:?}"
        );
    }

    /// Blocks until a caller of [`Gate::arrive_and_wait`] has arrived.
    pub(crate) fn wait_arrived(&self, what: &str) {
        let state = self.lock();
        assert!(
            !state.open,
            "gate misuse: waiting for {what} after the gate was opened"
        );
        let (state, timeout) = self
            .changed
            .wait_timeout_while(state, CEILING, |s| !s.arrived)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(state);
        assert!(
            !timeout.timed_out(),
            "hung: waiting for {what} to reach the gate for {CEILING:?}"
        );
    }

    /// Opens the gate and releases every caller parked at it.
    pub(crate) fn open(&self) {
        let mut state = self.lock();
        assert!(!state.open, "gate misuse: the gate was opened twice");
        state.open = true;
        self.changed.notify_all();
    }
}
