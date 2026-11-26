//! Issue #2909: when the rotator's watch loop wakes.
//!
//! A limit hit is an event. Whatever meets a usage, spend or credit limit
//! records it for the login it ran under, in the ledger's
//! `overflow_blocked`, and the rotator must act on that record at once
//! rather than find it on its next tick. On 21 November 2026 about
//! twenty-five runs on host-b used one login's five-hour window in about
//! twenty minutes; every one of them then met the limit and waited for a
//! six-hundred-second cycle that already had the answer.
//!
//! So the watch loop blocks in one `Waiter::wait` (`amplihack-utils`),
//! which polls an inotify watch on `ledger.json`, this process's pidfd and
//! a stop pipe together. It wakes when:
//!
//! - the blocks the ledger records differ from those the last cycle began
//!   with ([`moved`], [`Blocks::differ_at`]): a step or hook recorded a
//!   limit, `profile overflow clear` removed one, or the last cycle
//!   recorded one itself. A write that leaves the blocks as they were, which
//!   is every save the rotator makes for its own records, gives the next
//!   cycle nothing new to decide on and does not wake it;
//! - the earliest recorded reset passes ([`deadline`]), or passed while the
//!   last cycle ran ([`Blocks::fell_due`]), so a login comes back the
//!   moment its block ends;
//! - SIGTERM or SIGINT arrives, through the stop pipe, so `rotate stop`
//!   keeps the ten-second latency `control.rs` requires;
//! - `--interval` passes, which remains only as the outer bound of the
//!   wait. No timer, retry or polling loop is added here; the one-second
//!   sleep loop the watch used to run is gone.
//!
//! How many cycles events can cause. Every writer of a block keeps a block
//! in force as it is: `step_block::store`, `platform_block::record` and the
//! rotator's own `overflow_record::block_with` all leave an active block
//! unchanged (pinned by `an_active_block_is_neither_extended_nor_alerted_twice`
//! and `the_first_record_creates_the_block_and_the_second_does_not`). A login
//! therefore gains at most one block per reset window, and so starts at most
//! one cycle when the block is recorded and one when it falls due. Blocks
//! written while a cycle runs are all taken by the one cycle after it, so a
//! burst coalesces without a wall-clock floor. A block the rotator records
//! itself costs one extra cycle, once: the cycle after it begins with that
//! block, and the rotator does not record it again while it is in force.
//!
//! What a woken cycle reads. [`Waker::wait`] returns why it stopped
//! waiting, and the cycle after a limit event or a reset reads the live
//! login and takes the other logins' figures from their last reading
//! (#2909, "Wanted" item 3; `cycle::event_read` says when a reading can
//! stand). The freshness gate reads the chosen target before a swap, as in
//! every cycle. The cycle after an interval wake reads every login, so a
//! quiet rotator still reads each login once per `--interval`.
//!
//! Nothing in this module returns an error to the loop. An intake that
//! cannot be built or that fails later logs one fixed line and leaves the
//! rotator running on `--interval` alone, because a rotator that stops is
//! worse than a rotator that is slow.

use super::super::ledger::{RotateLedger, RotateState};
use super::super::log::wake_msg::{self, Reason};
use crate::commands::profile::auto_switch::kit::now;
use amplihack_utils::file_event::{SWAP_MASK, Waiter, Watch, Woke};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The blocks a cycle began with, and the resets that passed since.
#[path = "wake_blocks.rs"]
mod blocks;
pub use blocks::Blocks;

/// The earliest recorded reset after `now_unix`, over every block in the
/// ledger: the per-login `overflow_blocked` entries and the API-key
/// `platform_block`. A reset already passed does not contribute, so a
/// ledger `prune_blocks` has not yet reached cannot pin the wait to the
/// past.
fn earliest_reset(l: &RotateLedger, now_unix: i64) -> Option<i64> {
    l.overflow_blocked
        .values()
        .chain(l.platform_block.iter())
        .map(|b| b.until)
        .filter(|until| *until > now_unix)
        .min()
}

/// When the next cycle runs at the latest: the earlier of `now + interval`
/// and the earliest recorded reset.
///
/// The reset only ever shortens a wait. One beyond `--interval` changes
/// nothing, and `--interval 1` still ticks at one second. The wall-clock
/// reset becomes an `Instant` by saturating subtraction from `now_unix`, so
/// a clock that jumps forward cannot lengthen the wait past the interval
/// bound and one that jumps back cannot make it negative.
pub fn deadline(now: Instant, now_unix: i64, interval: Duration, l: &RotateLedger) -> Instant {
    let bound = now + interval;
    match earliest_reset(l, now_unix) {
        Some(at) => {
            let secs = u64::try_from(at.saturating_sub(now_unix)).unwrap_or(0);
            (now + Duration::from_secs(secs)).min(bound)
        }
        None => bound,
    }
}

/// Whether a change to the ledger file runs a cycle: the blocks in force
/// at `now_unix` in `read` differ from `seen`, the blocks the last cycle
/// began with.
///
/// `read` is the ledger as loaded now: `Some` for a ledger that loaded
/// (a missing one is an empty ledger), `None` for one that could not be
/// loaded. An unloadable ledger runs a cycle, so the cycle reports why
/// rather than the rotator going quiet until `--interval`. Neither the
/// cycle's prelude nor a step's writer saves over a ledger it cannot load,
/// so this costs one cycle per change to the file, and nothing writes one.
///
/// Who wrote the file does not matter, only what it now says. The rotator's
/// own saves keep the blocks as they were and so start nothing; a block
/// recorded while a cycle measured, which that cycle's own save then kept,
/// still differs from what the cycle began with and so still starts one.
pub fn moved(seen: &Blocks, read: Option<&RotateLedger>, now_unix: i64) -> bool {
    read.is_none_or(|l| seen.differ_at(&Blocks::of(l), now_unix))
}

/// The watch loop's intake: the waiter, the stop pipe behind it, and the
/// lines it has already said.
pub struct Waker {
    waiter: Option<Waiter>,
    /// The read end of the stop pipe. The write ends are held by
    /// signal-hook for as long as the process lives.
    interrupt: Option<UnixStream>,
    /// Lines already written, keyed by kind, so a repeated failure says it
    /// once.
    said: Vec<&'static str>,
    /// Why the last wait ended, so a quiet rotator does not write one
    /// `wake: interval` line per tick. A limit event and a reset are rare
    /// and always said, because they are the evidence that the event
    /// intake is doing its work.
    last: Option<Reason>,
    /// When the cycle before this wait began, in unix seconds, for
    /// [`Blocks::fell_due`].
    cycle_from: i64,
    /// The blocks when the cycle before this wait began, for [`Blocks`].
    seen: Blocks,
}

impl Waker {
    /// Builds the intake once, before the loop, so events raised while a
    /// cycle runs are queued by the kernel rather than lost.
    ///
    /// Call this *after* the stop flag is registered: signal-hook runs
    /// actions in registration order, so the flag is set before the pipe
    /// byte arrives and a wake always finds the flag already true.
    ///
    /// Issue #2095: a watcher a test started is armed here to end with that
    /// test. The lifetime thread sets `stop` and then writes to the stop
    /// pipe, in the same order as a signal, so this wait ends at once.
    pub fn arm(rs: &RotateState, stop: &Arc<AtomicBool>) -> Self {
        let mut w = Self {
            waiter: None,
            interrupt: None,
            said: Vec::new(),
            last: None,
            cycle_from: now(),
            seen: Blocks::of(&current(rs)),
        };
        match stop_pipe().and_then(|(i, tx)| build(rs, &i).map(|waiter| (i, tx, waiter))) {
            Ok((i, tx, waiter)) => {
                w.interrupt = Some(i);
                w.waiter = Some(waiter);
                super::super::lifetime::arm(stop, Some(tx));
            }
            Err(e) => {
                // The interval wait checks `stop` itself.
                super::super::lifetime::arm(stop, None::<UnixStream>);
                w.say(rs, "unavailable", &wake_msg::wake_unavailable(&e));
            }
        }
        w
    }

    /// Waits until the next cycle is due: a change in the recorded blocks,
    /// the earliest recorded reset, a stop signal, or `--interval`. Returns
    /// why the wait ended, having changed nothing; a wait a stop signal ended
    /// returns the reason it would have logged.
    pub fn wait(&mut self, rs: &RotateState, interval: u64, stop: &AtomicBool) -> Reason {
        let interval = Duration::from_secs(interval);
        let ledger = current(rs);
        // One clock for both, so the comparison below is exact.
        let (start, now_unix) = (Instant::now(), now());
        let mut until = deadline(start, now_unix, interval, &ledger);
        // A reset is the reason only where it is the earlier of the two,
        // which is what `deadline` has just worked out.
        let mut why = if until < start + interval {
            Reason::Reset
        } else {
            Reason::Interval
        };
        let blocks = Blocks::of(&ledger);
        // A block recorded while the last cycle ran, and kept by its save.
        if self.seen.differ_at(&blocks, now_unix) {
            (until, why) = (start, Reason::Ledger);
        }
        // A reset that passed while the last cycle ran. Asked of the blocks
        // that cycle began with too, because its own save pruned the block.
        if self.seen.fell_due(self.cycle_from, now_unix)
            || blocks.fell_due(self.cycle_from, now_unix)
        {
            (until, why) = (start, Reason::Reset);
        }
        while !stop.load(Ordering::SeqCst) && Instant::now() < until {
            let woke = match self.waiter.as_mut() {
                // No intake: the interval wait, exactly as it was.
                None => {
                    let left = until.saturating_duration_since(Instant::now());
                    std::thread::sleep(Duration::from_secs(1).min(left));
                    continue;
                }
                Some(w) => w.wait(until),
            };
            match woke {
                // The pidfd is this process's own, so `ProcessGone` cannot
                // happen; were it ever returned, a cycle is the safe answer.
                Ok(Woke::Deadline | Woke::ProcessGone) => break,
                Ok(Woke::File(_)) => {
                    if self.moved(rs) {
                        why = Reason::Ledger;
                        break;
                    }
                }
                // A lost watch or a dropped event may have hidden a limit
                // record. The ledger still holds every block in force, so
                // reading it once the new watch is in place finds any such
                // record, and a write made in between raises an event on
                // the new watch.
                Ok(Woke::Lost(_) | Woke::Overflow) => {
                    self.rebuild(rs);
                    if self.moved(rs) {
                        why = Reason::Ledger;
                        break;
                    }
                }
                Err(e) => {
                    self.waiter = None;
                    self.say(rs, "lost", &wake_msg::wake_failed(&e));
                }
            }
        }
        self.said_why(rs, why, stop);
        self.cycle_from = now();
        self.seen = Blocks::of(&current(rs));
        why
    }

    /// [`moved`] for the ledger as it is now, against the blocks the last
    /// cycle began with.
    fn moved(&self, rs: &RotateState) -> bool {
        let read = rs.load_ledger().ok().map(Option::unwrap_or_default);
        moved(&self.seen, read.as_ref(), now())
    }

    /// Writes why the wait ended. A limit event and a reset are always
    /// said; `wake: interval` is latched like the `idle` line, so a quiet
    /// rotator writes it once and not once per tick. Nothing is written
    /// when the rotator is stopping: the cycle after it is abandoned.
    fn said_why(&mut self, rs: &RotateState, why: Reason, stop: &AtomicBool) {
        if stop.load(Ordering::SeqCst) || (why == Reason::Interval && self.last == Some(why)) {
            return;
        }
        self.last = Some(why);
        super::log_line(rs, why.line());
    }

    /// Puts a new watch in place of one the kernel declared lost or whose
    /// queue overflowed. Without this the rotator would quietly fall back
    /// to `--interval` for the rest of its life.
    fn rebuild(&mut self, rs: &RotateState) {
        let Some(i) = self.interrupt.as_ref() else {
            return;
        };
        match build(rs, i) {
            Ok(waiter) => {
                self.waiter = Some(waiter);
                self.say(rs, "rearmed", wake_msg::WAKE_REARMED);
            }
            Err(e) => {
                self.waiter = None;
                self.say(rs, "lost", &wake_msg::wake_failed(&e));
            }
        }
    }

    /// Writes `line`, at most once per `kind` for this process, so a watch
    /// that keeps failing says so once rather than once per wait.
    fn say(&mut self, rs: &RotateState, kind: &'static str, line: &str) {
        if self.said.contains(&kind) {
            return;
        }
        self.said.push(kind);
        super::log_line(rs, line);
    }
}

/// The ledger as it is now. One that cannot be read reads as empty: it
/// contributes no reset and no block, so the wait is the interval. Not
/// reported here: the cycle loads the same ledger and logs the refusal
/// itself, once per cycle, with its reason.
fn current(rs: &RotateState) -> RotateLedger {
    rs.load_ledger().ok().flatten().unwrap_or_default()
}

/// One watch on the rotate directory for the name `ledger.json`, with a
/// pidfd for this process and the stop pipe. Fails off Linux, or where
/// inotify refuses.
///
/// The stop pipe is required, not optional: a waiter that a signal cannot
/// end would hold `rotate stop` past its ten-second limit, so without the
/// pipe the rotator keeps the interval wait, which checks the stop flag.
fn build(rs: &RotateState, interrupt: &UnixStream) -> std::io::Result<Waiter> {
    let watches = [Watch {
        dir: rs.dir.clone(),
        name: Some("ledger.json"),
        mask: SWAP_MASK,
    }];
    Waiter::with_interrupt(&watches, std::process::id(), interrupt.as_fd())
}

/// A socket pair whose write end signal-hook writes a byte to on SIGTERM
/// and SIGINT, so a wait of up to `--interval` ends at once. Both ends are
/// non-blocking and anonymous: no file is created and nothing is named.
///
/// `Waiter::wait` retries on `EINTR`, so the signal alone would not end the
/// wait; without this, `rotate stop` would fail its ten-second limit
/// (`control.rs`) on every rotator with a long interval.
///
/// Returns the read end, and a further write end for the lifetime thread of
/// a watcher a test started (#2095).
fn stop_pipe() -> std::io::Result<(UnixStream, UnixStream)> {
    let (tx, rx) = UnixStream::pair()?;
    rx.set_nonblocking(true)?;
    tx.set_nonblocking(true)?;
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        // Each registration takes a descriptor of its own and keeps it for
        // the life of the process.
        signal_hook::low_level::pipe::register(sig, tx.try_clone()?)?;
    }
    Ok((rx, tx))
}
