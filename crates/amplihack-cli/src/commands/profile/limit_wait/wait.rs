//! The wait itself (issue #1618), over a `WaitDeps` so tests drive it with
//! their own clock. Each pass checks, in order: a stop signal, the end of
//! the session that started the step, the deadline, the login fingerprint,
//! the rotator's capacity and, on a capacity wait, another login's capacity
//! (every 30 s), on a capacity wait with no deadline whether a rotator is
//! still running, and the progress line (every 60 s), then sleeps at most one
//! second. The login baseline is the first value two probes in a row agree
//! on, and a change is a different value that two probes in a row agree on,
//! so a torn or failed read of `.claude.json`, first or later, never ends the
//! wait.

use super::events::{Event, line};
use super::plan::{Bound, Plan, PlanKind};
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
    /// A reading taken after `start` shows another stored login has capacity.
    fn other_capacity(&mut self, start: i64) -> bool;
    /// A rotator process is still running. Asked only on a
    /// capacity wait with no deadline, which nothing else could end once the
    /// rotator is gone (issue #2022).
    fn rotator_running(&mut self) -> bool;
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
    /// A capacity wait's `max_wait` deadline passed; nothing lifted the limit.
    CapacityExpired,
    /// A reading showed that another stored login has capacity.
    OtherCapacity,
    /// A capacity wait with no deadline saw no running rotator at two polls
    /// in a row, so nothing could record capacity or swap the login.
    RotatorStopped,
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
            End::CapacityExpired => "capacity expired",
            End::OtherCapacity => "login capacity",
            End::RotatorStopped => "rotator stopped",
            End::SessionEnded => "session ended",
            End::Signal(_) => "signal",
        }
    }

    fn at_deadline(bound: Bound, kind: PlanKind) -> Self {
        match (bound, kind) {
            (Bound::StepTimeout, _) => End::StepTimeout,
            // On a capacity wait the reset is the failed login's own: it has
            // recovered, so the step is retried.
            (Bound::ResetMargin, _) => End::ResetReached,
            (Bound::MaxWait, PlanKind::Reset) => End::MaxReached,
            (Bound::MaxWait, PlanKind::Capacity) => End::CapacityExpired,
            // An unbounded wait has no deadline to reach; mapped for
            // completeness, never panicking.
            (Bound::Unbounded, _) => End::CapacityExpired,
        }
    }
}

/// Waits until `plan.until`, a login change, rotator capacity, on a capacity
/// wait another login's capacity, the end of the step's session or a stop
/// signal. Writes only `progress` lines; `start`, `end` and `resume` are the
/// caller's.
pub(crate) fn wait<D: WaitDeps>(plan: &Plan, deps: &mut D) -> End {
    let start = deps.now();
    // The previous probe, and the value two probes in a row agreed on.
    let mut last = deps.fingerprint();
    let mut baseline: Option<Option<D::Print>> = None;
    let mut next_poll = start.saturating_add(POLL_SECS);
    let mut next_progress = start.saturating_add(PROGRESS_SECS);
    // Only an unbounded capacity wait depends on the rotator to end; a miss
    // must be seen at two polls in a row, so a rotator restart is not a stop.
    let watch_rotator = plan.kind == PlanKind::Capacity && plan.until.is_none();
    let mut rotator_missed = false;
    loop {
        if let Some(sig) = deps.signal() {
            return End::Signal(sig);
        }
        if deps.session_ended() {
            return End::SessionEnded;
        }
        let now = deps.now();
        if plan.until.is_some_and(|until| now >= until) {
            return End::at_deadline(plan.bound, plan.kind);
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
            if plan.kind == PlanKind::Capacity && deps.other_capacity(start) {
                return End::OtherCapacity;
            }
            if watch_rotator {
                let missed = !deps.rotator_running();
                if missed && rotator_missed {
                    return End::RotatorStopped;
                }
                rotator_missed = missed;
            }
            next_poll = now.saturating_add(POLL_SECS);
        }
        if now >= next_progress {
            let remaining = plan
                .until
                .map(|until| u64::try_from(until.saturating_sub(now)).unwrap_or(0));
            deps.log(&line(&Event::Progress { remaining }, &chrono::Local));
            next_progress = now.saturating_add(PROGRESS_SECS);
        }
        // At most one second, so a signal is noticed promptly; an unbounded
        // wait sleeps the full second.
        let left = plan
            .until
            .map_or(1, |until| until.saturating_sub(now).min(1));
        deps.sleep(Duration::from_secs(u64::try_from(left).unwrap_or(1)));
    }
}
