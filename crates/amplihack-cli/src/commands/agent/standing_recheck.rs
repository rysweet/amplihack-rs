//! What a standing session's brake checks before it refuses (issue #2912):
//! whether the room it saw at the exit was still there after the exit.
//!
//! The rotator writes its swap record only when it swaps, so at the moment
//! of an exit the record is usually older than the run. It says that the
//! login it swapped to had room then, and nothing about whether the run has
//! spent it since. Read on its own, such a record makes a run that went into
//! a usage limit look like a run that crashed while a login had room, and
//! the brake would stop the chain on a limit and leave the session to the
//! supervisor.
//!
//! So before a `no-progress` refusal the wrapper asks the rotator, when one
//! runs. It blocks on the rotate directory until the rotator has measured
//! after the exit, and acts on what that measurement found:
//!
//! - a swap since the run began onto a login with room, or a swap or
//!   `profile switch` after the exit: the capacity has been replaced, and
//!   the session is relaunched;
//! - the live login with room and no swap: the exit was not a limit, so the
//!   refusal stands, now on a reading taken after the exit;
//! - no capacity (the rotator's no-room alert): the run met a limit. The
//!   wrapper waits until a login has room and then relaunches.
//!
//! When there is no one to ask (no rotator runs, a login is pinned with
//! `profile switch`, the rotate directory cannot be watched) or the rotator
//! stops before it answers, the refusal stands on what was read at the
//! exit, and its alert says so. A reading already on disk that was taken
//! after the exit is acted on as above even then, except that no capacity
//! with no rotator to find room is a `gone` refusal, not a wait.
//!
//! A cycle's measurement time is when its reading of the live login was
//! taken, which for a passive (status line) reading can be earlier than the
//! cycle. Such a reading from before the exit is not an answer; the rotator
//! reads the login directly once its passive reading is older than its
//! passive limit or one interval, so the wait is a few cycles at most.
//!
//! The wait ends on a write the rotator makes or on its exit (a pidfd on
//! the rotator), with [`MAX_WAIT`] as the outer bound. It has no interval
//! and no probe of its own.

use super::Evidence;
use super::relaunch::{DefaultSignals, Lost, watchable};
use amplihack_utils::file_event::{SWAP_MASK, Waiter, Watch, Woke};
use amplihack_utils::rotator_status::swap_record::{SwapRecord, read_swap, rotate_dir};
use amplihack_utils::rotator_status::{
    CycleStatus, Ledger, MAX_WAIT, read_ledger, running_rotator,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// What the rotator's answer means for a refusal the brake would make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recheck {
    /// The capacity the run had has been replaced: relaunch.
    Appeared,
    /// The refusal stands, on this evidence.
    Refuse(Evidence),
    /// SIGHUP or SIGTERM before the wait blocked: the pane or the host is
    /// going away.
    Stopped,
    /// The live login read as spent after the exit, and the wait for room
    /// ended without one, for the reason given.
    Gone(Lost),
}

/// What one reading of the rotator's two files says. See [`answer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// A swap onto a login with room since the run began, or a cycle that
    /// measured after the exit and swapped, or a `profile switch` made
    /// after it: the capacity has been replaced.
    Replaced,
    /// A cycle that measured after the exit stayed on the live login, which
    /// had room.
    Room,
    /// A cycle that measured after the exit found no capacity.
    Spent,
    /// No reading will come: the rotator is held by a `profile switch` pin,
    /// or its cycle after the exit could not read the live login.
    Unanswered,
    /// No cycle has measured since the exit yet.
    Pending,
}

/// What the swap record and the ledger's last cycle say about the run that
/// began at `since` and exited at `exited`, both in Unix seconds. Pure.
///
/// A measurement counts only when it began strictly after the exit second.
/// One taken in that second may predate the exit, and reading it as later
/// would let a reading from before a limit refuse the session; the cost of
/// the strict rule is one more rotator cycle of waiting.
pub fn answer(
    swap: Option<&SwapRecord>,
    cycle: Option<&CycleStatus>,
    since: i64,
    exited: i64,
) -> Answer {
    if swap.is_some_and(|r| r.swapped_at >= since && r.has_room()) {
        return Answer::Replaced;
    }
    let Some(c) = cycle else {
        return Answer::Pending;
    };
    let after = c.measured_at.is_some_and(|m| m > exited);
    let outcome = c.outcome.as_deref();
    match outcome {
        Some("swapped" | "switched") if after && c.has_capacity() => Answer::Replaced,
        Some("stayed") if after && c.has_capacity() => Answer::Room,
        _ if after && (c.alert || outcome == Some("alerted")) => Answer::Spent,
        // A pin: held cycles measure nothing, and a `switched` record is
        // kept by them, so neither will be followed by a reading.
        Some("held" | "switched") => Answer::Unanswered,
        Some("stayed-unread") if after => Answer::Unanswered,
        _ => Answer::Pending,
    }
}

/// Asks the rotator for `root` whether the run that began at `since` and
/// exited at `exited` met a limit, and blocks until it has answered; see the
/// module documentation. `on_wait` is called with the waiting line's reason
/// each time the session is about to be held for a new one:
/// `room-unconfirmed` while the rotator has not yet measured, `no-room`
/// once it has found no capacity.
///
/// While it blocks, `SIGHUP`, `SIGTERM` and `SIGINT` have their default
/// action, as in the wait for room at the exit.
pub fn recheck(
    root: &Path,
    since: i64,
    exited: i64,
    stopped: &AtomicBool,
    mut on_wait: impl FnMut(&'static str),
) -> Recheck {
    let look = || {
        let now = crate::commands::agent::registry::now();
        let swap = read_swap(root, now);
        let cycle = match read_ledger(root, now) {
            Ledger::Read(c) => Ok(Some(c)),
            Ledger::Missing => Ok(None),
            Ledger::Unreadable => Err(()),
        };
        let evidence = if swap.is_some() {
            Evidence::Record
        } else {
            Evidence::Unchecked
        };
        let found = match cycle {
            Ok(c) => answer(swap.as_ref(), c.as_ref(), since, exited),
            Err(()) => Answer::Unanswered,
        };
        (found, evidence)
    };
    let (first, evidence) = look();
    // When the rotator cannot be asked for another reading, the one already
    // on disk still counts if it was taken after the exit, as it would in
    // the wait: room refuses on that reading, and no capacity is a wait
    // for room that ended, for `why`, without one.
    let unasked = |why: Lost| match first {
        Answer::Replaced => Recheck::Appeared,
        Answer::Room => Recheck::Refuse(Evidence::Reading),
        Answer::Spent => Recheck::Gone(why),
        Answer::Unanswered | Answer::Pending => Recheck::Refuse(evidence),
    };
    if first == Answer::Replaced {
        return Recheck::Appeared;
    }
    let dir = rotate_dir(root);
    let Some(rotator) = running_rotator(root) else {
        return unasked(Lost::Rotator);
    };
    if !watchable(&dir) {
        return unasked(Lost::Unwatchable);
    }
    let watches = [Watch {
        dir,
        name: None,
        mask: SWAP_MASK,
    }];
    // The pidfd is the rotator's, so a rotator that exits ends the wait.
    let Ok(mut waiter) = Waiter::new(&watches, rotator) else {
        return unasked(Lost::Unarmed);
    };
    let Some(deadline) = Instant::now().checked_add(MAX_WAIT) else {
        return unasked(Lost::Deadline);
    };
    let _default_signals = DefaultSignals::install();
    if stopped.load(Ordering::SeqCst) {
        return Recheck::Stopped;
    }
    let mut spent = false;
    let mut said = None;
    loop {
        // Read again on every wake, and once the watch is armed, so a write
        // made before the watch existed is not missed.
        let (found, evidence) = look();
        match found {
            Answer::Replaced => return Recheck::Appeared,
            Answer::Room if spent => return Recheck::Appeared,
            Answer::Room => return Recheck::Refuse(Evidence::Reading),
            Answer::Spent => spent = true,
            Answer::Unanswered if !spent => return Recheck::Refuse(evidence),
            Answer::Unanswered | Answer::Pending => {}
        }
        let reason = if spent { "no-room" } else { "room-unconfirmed" };
        if said != Some(reason) {
            on_wait(reason);
            said = Some(reason);
        }
        let ended = |why| {
            if spent {
                Recheck::Gone(why)
            } else {
                Recheck::Refuse(evidence)
            }
        };
        match waiter.wait(deadline) {
            Ok(Woke::File(_) | Woke::Overflow) => {}
            Ok(Woke::Deadline) => return ended(Lost::Deadline),
            Ok(Woke::ProcessGone) => return ended(Lost::Rotator),
            Ok(Woke::Lost(_)) | Err(_) => return ended(Lost::Watch),
        }
    }
}
