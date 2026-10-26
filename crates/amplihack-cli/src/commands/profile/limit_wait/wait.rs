//! The wait itself (issue #1618), over a `WaitDeps` so tests drive it with
//! their own clock. Each pass checks, in order: a stop signal, the end of
//! the session that started the step, the deadline, the login fingerprint
//! and the rotator's capacity (every 30 s), and the progress line (every
//! 60 s), then sleeps at most one second. The login baseline is the first
//! value two probes in a row agree on, and a change is a different value
//! that two probes in a row agree on, so a torn or failed read of
//! `.claude.json`, first or later, never ends the wait.

use super::events::{Event, line};
use super::plan::{Bound, Plan};
use std::time::Duration;

/// How often the login fingerprint is taken again.
const POLL_SECS: i64 = 30;
/// How often a `progress` line is written.
const PROGRESS_SECS: i64 = 60;

/// What the wait needs from the world.
pub(crate) trait WaitDeps {
    /// The login fingerprint; only compared, never shown.
    type Print: Eq;
    /// Unix seconds.
    fn now(&mut self) -> i64;
    fn sleep(&mut self, d: Duration);
    fn fingerprint(&mut self) -> Option<Self::Print>;
    /// A stop signal received, if any.
    fn signal(&mut self) -> Option<i32>;
    /// The process that started the step is gone, so nobody is left to
    /// use the retry's result.
    fn session_ended(&mut self) -> bool;
    /// The rotator recorded a cycle with capacity after `start`.
    fn rotator_capacity(&mut self, start: i64) -> bool;
    fn log(&mut self, line: &str);
}

/// Why the wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    ResetReached,
    MaxReached,
    StepTimeout,
    LoginChanged,
    RotatorCapacity,
    SessionEnded,
    Signal(i32),
}

impl End {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            End::ResetReached => "reset reached",
            End::MaxReached => "max reached",
            End::StepTimeout => "step timeout",
            End::LoginChanged => "login changed",
            End::RotatorCapacity => "rotator capacity",
            End::SessionEnded => "session ended",
            End::Signal(_) => "signal",
        }
    }

    fn at_deadline(bound: Bound) -> Self {
        match bound {
            Bound::ResetMargin => End::ResetReached,
            Bound::MaxWait => End::MaxReached,
            Bound::StepTimeout => End::StepTimeout,
        }
    }
}

/// Waits until `plan.until`, a login change, rotator capacity, the end of
/// the step's session or a stop signal. Writes only `progress` lines;
/// `start`, `end` and `resume` are the caller's.
pub(crate) fn wait<D: WaitDeps>(plan: &Plan, deps: &mut D) -> End {
    let start = deps.now();
    // The previous probe, and the value two probes in a row agreed on.
    let mut last = deps.fingerprint();
    let mut baseline: Option<Option<D::Print>> = None;
    let mut next_poll = start.saturating_add(POLL_SECS);
    let mut next_progress = start.saturating_add(PROGRESS_SECS);
    loop {
        if let Some(sig) = deps.signal() {
            return End::Signal(sig);
        }
        if deps.session_ended() {
            return End::SessionEnded;
        }
        let now = deps.now();
        if now >= plan.until {
            return End::at_deadline(plan.bound);
        }
        if now >= next_poll {
            let probe = deps.fingerprint();
            match baseline.take() {
                None if probe == last => baseline = Some(std::mem::replace(&mut last, probe)),
                None => last = probe,
                Some(base) => {
                    if probe != base && probe == last {
                        return End::LoginChanged;
                    }
                    baseline = Some(base);
                    last = probe;
                }
            }
            if deps.rotator_capacity(start) {
                return End::RotatorCapacity;
            }
            next_poll = now.saturating_add(POLL_SECS);
        }
        if now >= next_progress {
            let remaining = u64::try_from(plan.until.saturating_sub(now)).unwrap_or(0);
            deps.log(&line(&Event::Progress { remaining }, &chrono::Local));
            next_progress = now.saturating_add(PROGRESS_SECS);
        }
        let left = plan.until.saturating_sub(now);
        deps.sleep(Duration::from_secs(u64::try_from(left.min(1)).unwrap_or(1)));
    }
}
