//! The wait itself (issue #1618), over a `WaitDeps` so tests drive it with
//! their own clock.
//!
//! The wait blocks until something it could end on may have happened
//! ([`WaitDeps::block`]): a write to one of the files its checks read, a
//! stop signal, the exit of the process that started the step, or the
//! wait's deadline. It reads its files once when it begins, and after that
//! only after such a write, never on an interval, and it launches nothing. Issue #2911 asks for a wait woken by
//! limit events, and each check below reads the file that records one: the
//! rotator's ledger and swap record, the auto-switch readings and ledger,
//! `.claude.json` and the credentials file (`wake`).
//!
//! After a write it checks, in order: the rotator's capacity, the login
//! fingerprint, another login's capacity, then the named login key. One
//! rotator write both swaps the login and records the cycle's room, and such
//! a wait ends `rotator capacity`, the reason auto-drive and operators read.
//! Another login's capacity ends a wait of any kind, not only a capacity wait
//! (issue #2694): a step whose own reset is hours away still moves to a login
//! with room as soon as one is seen, unless its own login changed in the
//! same write. A stop signal, the end of the step's session and the deadline
//! are checked on every wake, before the files.
//!
//! The one exception is a login-change wait (issue #2731), where both
//! capacity sources are skipped: it is waiting for an organisation spend
//! limit to be replaced by another organisation's credit, and no capacity
//! reading reports credit. The plan says so with
//! [`Plan::capacity_may_end`].
//!
//! # The login fingerprint
//!
//! The baseline is the fingerprint read when the wait starts. A change is a
//! later reading that differs from it. A reading that could not be made,
//! such as a `.claude.json` read while another process is half-way through
//! writing it, is not a reading: it neither changes the baseline nor ends
//! the wait, and the write that completes the file wakes the wait again. A
//! baseline that could not be read is taken from the first reading that
//! can.
//!
//! For a `$HOME/.claude` step the wait also watches
//! [`WaitDeps::login_changed`], the named login key, which is anchored on a
//! reading taken *before the attempt was launched*, so a swap the
//! fingerprint's baseline absorbed is seen too. It is read last, so it
//! renames no end that already had a name.
//!
//! # The progress line
//!
//! A `progress` line is written every [`PROGRESS_SECS`] while the wait
//! lasts, so the wait wakes then too. That wake reads no file, checks no
//! account and launches nothing: it writes the line and blocks again. It is
//! there because a supervisor that watches a child for output (the idle
//! watchdog, `AMPLIHACK_IDLE_TIMEOUT_SECS`) would otherwise end a step that
//! is waiting hours for a reset, and because the line is how an operator
//! sees that the step is waiting rather than hung.
//!
//! Where no file event can be had ([`WaitDeps::block`] says so), the wake
//! for the progress line is the only one left, and the wait reads its files
//! then; the wait's own line says so once (`wake`).

use super::events::{Event, line};
use super::plan::{Bound, Plan, PlanKind};

/// How often a `progress` line is written.
pub(crate) const PROGRESS_SECS: i64 = 60;

/// What the wait needs from the world.
pub(crate) trait WaitDeps {
    /// The login fingerprint; only compared, never shown.
    type Print: Eq;
    /// Unix seconds.
    fn now(&mut self) -> i64;
    /// Blocks until a file the checks read may have changed, a stop signal
    /// arrives, the step's session ends, or `until` (unix seconds) comes,
    /// whichever is first. Returns whether the files should be read again:
    /// true after a write to one of them, false when only the time came or a
    /// signal or the session's end woke it, which the wait checks itself.
    fn block(&mut self, until: i64) -> bool;
    /// The login fingerprint, or `None` when it could not be read.
    fn fingerprint(&mut self) -> Option<Self::Print>;
    /// The login in `$HOME/.claude` is no longer the one this step's
    /// attempt ran on (issue #2731); always false for a step whose login
    /// the rotator never changes.
    ///
    /// Unlike [`WaitDeps::fingerprint`] this needs no baseline: it is
    /// anchored on a login key read before the attempt was launched. It
    /// compares names the rotator itself wrote and fails closed, so a
    /// reading that cannot be made is not a change.
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
    /// Whether a login change that has been seen may end this wait: the
    /// login now live has room (issue #2857).
    ///
    /// Asked only once a change has been seen, and last in the conjunction,
    /// so a wait that sees no change makes no extra reading. A change the
    /// answer holds back is not forgotten: the wait keeps its baseline and
    /// ends on the change at the first wake where the answer is yes, which
    /// is the rotator's next write to its ledger.
    ///
    /// It has no default body on purpose. An implementor that forgot it
    /// would fail open, which is the one direction this must not fail in,
    /// so such an implementor does not compile.
    fn change_may_end(&mut self) -> bool;
    fn log(&mut self, line: &str);
}

/// Why the wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    ResetReached,
    /// A login-change wait's outer bound passed; the login did not change.
    MaxReached,
    LoginChanged,
    RotatorCapacity,
    /// The outer bound of a capacity wait with no reset ahead passed;
    /// nothing lifted the limit.
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
            End::LoginChanged => "login changed",
            End::RotatorCapacity => "rotator capacity",
            End::CapacityExpired => "capacity expired",
            End::OtherCapacity => "login capacity",
            End::SessionEnded => "session ended",
            End::Signal(_) => "signal",
        }
    }

    /// Whether this end claimed the retry it earns should work, so a limit
    /// on that retry contradicts it (issue #2857).
    ///
    /// A reset that arrived, the rotator's capacity, a login change and
    /// another login's room each say *try now*. The rest (a maximum, a
    /// capacity wait's deadline, the session's end and a stop signal) are
    /// the wait running out, and claim nothing, so a limit after one of
    /// them is no contradiction.
    pub(crate) fn said_try_now(self) -> bool {
        matches!(
            self,
            End::ResetReached | End::RotatorCapacity | End::LoginChanged | End::OtherCapacity
        )
    }

    fn at_deadline(bound: Bound, kind: PlanKind) -> Self {
        match (bound, kind) {
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
    let mut base = deps.fingerprint();
    let mut next_progress = start.saturating_add(PROGRESS_SECS);
    // The files are read once as the wait begins, for anything written
    // between the limit and the watches being made, and then only after a
    // write.
    let mut read = true;
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
        if read && let Some(end) = read_files(plan, start, &mut base, deps) {
            return end;
        }
        if now >= next_progress {
            let remaining = u64::try_from(plan.until.saturating_sub(now)).unwrap_or(0);
            deps.log(&line(&Event::Progress { remaining }, &chrono::Local));
            next_progress = now.saturating_add(PROGRESS_SECS);
        }
        read = deps.block(plan.until.min(next_progress));
    }
}

/// The checks made after a write to one of the wait's files, in their
/// order; `Some` with the end they found.
fn read_files<D: WaitDeps>(
    plan: &Plan,
    start: i64,
    base: &mut Option<D::Print>,
    deps: &mut D,
) -> Option<End> {
    // Both capacity sources, or neither (#2731). They are read on a reset
    // wait as well as a capacity wait (#2694): whether the step may move at
    // all is `other_capacity`'s own decision, and it is false for a step
    // with no auto-switch candidate. A login-change wait reads neither,
    // because neither reports what it is waiting for.
    let capacity = plan.capacity_may_end();
    if capacity && deps.rotator_capacity(start) {
        return Some(End::RotatorCapacity);
    }
    // Issue #2857: the fingerprint says the login moved, not that the one
    // it moved to can be used. The baseline is kept when the room gate says
    // no, so the change still ends the wait at the first wake where the
    // login now live has room.
    if let Some(probe) = deps.fingerprint() {
        match base {
            None => *base = Some(probe),
            Some(b) => {
                if probe != *b && deps.change_may_end() {
                    return Some(End::LoginChanged);
                }
            }
        }
    }
    if capacity && deps.other_capacity(start) {
        return Some(End::OtherCapacity);
    }
    // The named login key last, so it changes no wait's reported reason,
    // and gated on room as the fingerprint is (issue #2857). The key is read
    // first, so a wait that sees no change asks nothing about room.
    if deps.login_changed() && deps.change_may_end() {
        return Some(End::LoginChanged);
    }
    None
}
