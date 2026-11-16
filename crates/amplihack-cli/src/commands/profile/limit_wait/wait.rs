//! The wait itself (issue #1618), over a `WaitDeps` so tests drive it with
//! their own clock. Each pass checks, in order: a stop signal, the end of
//! the session that started the step, the deadline, the login fingerprint,
//! the rotator's capacity and another login's capacity (both every 30 s),
//! and the progress line (every 60 s), then sleeps at most one second.
//! Another login's capacity ends a wait of any kind, not only a capacity
//! wait (issue #2694): a step whose own reset is hours away still moves to
//! a login with room as soon as one is seen.
//!
//! The one exception is a login-change wait (issue #2731), where both
//! capacity sources are skipped: it is waiting for an organisation spend
//! limit to be replaced by another organisation's credit, and no capacity
//! reading reports credit. The plan says so with
//! [`Plan::capacity_may_end`].
//!
//! The login baseline is the first value two probes in a row agree on, and
//! a change is a different value that two probes in a row agree on, so a
//! torn or failed read of `.claude.json`, first or later, never ends the
//! wait.
//!
//! That rule costs the fingerprint its first poll interval: a swap landing
//! before two probes have agreed is absorbed into the baseline, because to
//! the fingerprint it is indistinguishable from a torn first read. Both are
//! "the first reading differs from the stable one that follows", and no
//! comparison of opaque values can tell them apart. For a `$HOME/.claude`
//! step the wait therefore also watches [`WaitDeps::login_changed`], the
//! named login key, which is anchored on a reading taken *before the
//! attempt was launched* and so has no baseline to settle. That is the
//! check that ends a login-change wait when the rotator swaps in the
//! wait's first seconds (issue #2731) — the one case where the fingerprint
//! is the only other exit.
//!
//! It is read last of the four, after both capacity sources, so it renames
//! no end that already had a name: a swapping rotator cycle moves the key
//! and reports capacity in the same write, and such a wait still ends
//! `rotator capacity` as it always has.

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
    /// The login in `$HOME/.claude` is no longer the one this step's
    /// attempt ran on (issue #2731); always false for a step whose login
    /// the rotator never changes.
    ///
    /// Unlike [`WaitDeps::fingerprint`] this needs no baseline: it is
    /// anchored on a login key read before the attempt was launched, so a
    /// swap landing at any point after that is seen, including inside the
    /// first poll interval. It compares names the rotator itself wrote and
    /// fails closed, so a reading that cannot be made is not a change.
    fn login_changed(&mut self) -> bool;
    /// A stop signal received, if any.
    fn signal(&mut self) -> Option<i32>;
    /// The process that started the step is gone, so nobody is left to
    /// use the retry's result.
    fn session_ended(&mut self) -> bool;
    /// The rotator recorded a cycle with capacity after `start`.
    fn rotator_capacity(&mut self, start: i64) -> bool;
    /// A reading taken after `start` shows another stored login has capacity.
    fn other_capacity(&mut self, start: i64) -> bool;
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
        }
    }
}

/// Waits until `plan.until`, a login change, rotator capacity, another
/// login's capacity, the end of the step's session or a stop signal. Writes
/// only `progress` lines; `start`, `end` and `resume` are the caller's.
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
            // Both capacity sources, or neither (#2731). They are read on
            // a reset wait as well as a capacity wait (#2694): whether the
            // step may move at all is `other_capacity`'s own decision, and
            // it is false for a step with no auto-switch candidate. A
            // login-change wait reads neither, because neither reports what
            // it is waiting for; the fingerprint above and the login key
            // below are what end it.
            if plan.capacity_may_end() {
                if deps.rotator_capacity(start) {
                    return End::RotatorCapacity;
                }
                if deps.other_capacity(start) {
                    return End::OtherCapacity;
                }
            }
            // The named login key last, so it changes no wait's reported
            // reason. A rotator cycle that swaps moves this key and sets
            // `rotator_capacity` in the same write, and that end is the
            // one such a wait has always reported; checking the key first
            // would relabel it `login changed`. What the key adds is the
            // case no capacity source speaks for: a login-change wait,
            // where both are suppressed and the fingerprint's baseline has
            // not settled yet (issue #2731).
            if deps.login_changed() {
                return End::LoginChanged;
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
