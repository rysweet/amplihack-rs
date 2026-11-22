//! Issue #2909: when the rotator's watch loop wakes.
//!
//! A limit hit is an event. Whatever meets a usage, spend or credit limit
//! records it for the login it ran under, in the ledger's
//! `overflow_blocked`, and the rotator must act on that record at once
//! rather than find it on its next tick. On 21 November 2026 about
//! twenty-five runs on calypso used one login's five-hour window in about
//! twenty minutes; every one of them then met the limit and waited for a
//! six-hundred-second cycle that already had the answer.
//!
//! So the watch loop blocks in one `Waiter::wait` (`amplihack-utils`),
//! which polls an inotify watch on `ledger.json`, this process's pidfd and
//! a stop pipe together. It wakes when:
//!
//! - the ledger changes, unless this process wrote exactly those bytes
//!   ([`is_self_write`]), or the blocks in force differ from those the last
//!   cycle began with ([`Blocks`]), which catches a record written while
//!   that cycle ran and kept by its own save;
//! - the earliest recorded reset passes ([`deadline`]), or passed while the
//!   last cycle ran ([`Blocks::fell_due`]), so a login comes back the
//!   moment its block ends;
//! - SIGTERM or SIGINT arrives, through the stop pipe, so `rotate stop`
//!   keeps the ten-second latency `control.rs` requires;
//! - `--interval` passes, which remains only as the outer bound of the
//!   wait. No timer, retry or polling loop is added here; the one-second
//!   sleep loop the watch used to run is gone.
//!
//! Nothing in this module returns an error to the loop. An intake that
//! cannot be built or that fails later logs one fixed line and leaves the
//! rotator running on `--interval` alone, because a rotator that stops is
//! worse than a rotator that is slow.

use super::super::ledger::{MAX_STATE, RotateLedger, RotateState, trusted};
use super::super::log::wake_msg::{self, Reason};
use crate::commands::profile::auto_switch::kit::{now, read_bounded_nofollow};
use amplihack_utils::file_event::{SWAP_MASK, Waiter, Watch, Woke};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The blocks a cycle began with, and the resets that passed since.
#[path = "wake_blocks.rs"]
mod blocks;
pub use blocks::Blocks;

/// The longest a burst of ledger writes may make the rotator run cycles:
/// one per this many seconds, or per `--interval` when that is shorter.
const COALESCE_SECS: u64 = 5;

/// The ledger bytes this process wrote last, so a wake raised by the
/// rotator's own atomic write is not taken for a limit event. Held in
/// memory only: it carries profile names and account fields, so it is never
/// logged, printed or written anywhere, and it has no `Debug`.
static LAST_WRITTEN: Mutex<Option<Vec<u8>>> = Mutex::new(None);

/// Records the bytes `save_ledger` has just put in `ledger.json`.
///
/// Called *after* the rename succeeds, never before. An external write made
/// between the rename and this call is then read as this process's own on
/// the next wake, costing one extra cycle; inverting the order would
/// instead suppress real external changes, which is the failure that
/// matters. Do not invert it.
pub fn note_written(bytes: &[u8]) {
    if let Ok(mut last) = LAST_WRITTEN.lock() {
        *last = Some(bytes.to_vec());
    }
}

/// Whether `ledger` holds, byte for byte, what this process wrote last.
///
/// `save_ledger` renames over the watched name on nearly every cycle, so a
/// long-lived waiter wakes on its own write. Comparing content, not the
/// modification time or the length, because an atomic write is a rename and
/// changes the inode and the timestamps whatever the content.
///
/// Fails open in every direction that costs only a cycle: a missing file, a
/// file over [`MAX_STATE`], one owned by another user, or nothing recorded
/// yet all read as changed. Suppression is the only path that skips a
/// cycle, and it takes an exact match.
pub fn is_self_write(ledger: &Path) -> bool {
    let Ok(last) = LAST_WRITTEN.lock() else {
        return false;
    };
    let Some(last) = last.as_deref() else {
        return false;
    };
    matches!(
        read_bounded_nofollow(ledger, MAX_STATE),
        Ok(Some((raw, m))) if trusted(&m) && raw == last
    )
}

/// The earliest recorded reset after `now_unix`, over every block in the
/// ledger: the per-login `overflow_blocked` entries and the API-key
/// `platform_block`. A reset already passed does not contribute, so a
/// ledger `prune_blocks` has not yet reached cannot pin the wait to the
/// past.
pub fn earliest_reset(l: &RotateLedger, now_unix: i64) -> Option<i64> {
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

/// The least time between the end of one cycle and a cycle started by a
/// ledger write: five seconds, or `--interval` when that is shorter, so
/// `--interval 1` is not slowed.
///
/// Any process that may write `ledger.json` can start a cycle, and a cycle
/// probes every login. Writes inside the floor are not dropped: each one
/// shortens the wait to the floor rather than being lost, so a burst
/// collapses into one cycle instead of one cycle each.
pub fn coalesce_floor(interval: Duration) -> Duration {
    interval.min(Duration::from_secs(COALESCE_SECS))
}

/// When a cycle runs for a ledger write seen at `now`, in a wait that began
/// at `started` (the end of the previous cycle): at once, unless the last
/// cycle ended less than `floor` ago, and then when the floor has passed.
///
/// A limit recorded minutes after the last cycle is acted on with no delay
/// at all; only a write arriving straight after a cycle, typically the
/// second of a burst, waits out what is left of the floor.
pub fn event_due(started: Instant, now: Instant, floor: Duration) -> Instant {
    now.max(started + floor)
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
    pub fn arm(rs: &RotateState) -> Self {
        let mut w = Self {
            waiter: None,
            interrupt: None,
            said: Vec::new(),
            last: None,
            cycle_from: now(),
            seen: Blocks::of(&current(rs)),
        };
        match stop_pipe().and_then(|i| build(rs, &i).map(|waiter| (i, waiter))) {
            Ok((i, waiter)) => {
                w.interrupt = Some(i);
                w.waiter = Some(waiter);
            }
            Err(e) => w.say(rs, "unavailable", &wake_msg::wake_unavailable(&e)),
        }
        w
    }

    /// Waits until the next cycle is due: a ledger change or a block the
    /// last cycle did not act on, the earliest recorded reset, a stop
    /// signal, or `--interval`. Returns having changed nothing.
    pub fn wait(&mut self, rs: &RotateState, interval: u64, stop: &AtomicBool) {
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
        let floor = coalesce_floor(interval);
        let blocks = Blocks::of(&ledger);
        // A block recorded while the last cycle ran, and kept by its save.
        if self.seen.differ_at(&blocks, now_unix) {
            why = Reason::Ledger;
            until = until.min(event_due(start, start, floor));
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
                    if !is_self_write(&rs.ledger) {
                        why = Reason::Ledger;
                        until = until.min(event_due(start, Instant::now(), floor));
                    }
                }
                // A lost watch or a dropped event may have hidden a limit
                // record, so a cycle runs as for one.
                Ok(Woke::Lost(_) | Woke::Overflow) => {
                    self.rebuild(rs);
                    why = Reason::Ledger;
                    until = until.min(event_due(start, Instant::now(), floor));
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
fn stop_pipe() -> std::io::Result<UnixStream> {
    let (tx, rx) = UnixStream::pair()?;
    rx.set_nonblocking(true)?;
    tx.set_nonblocking(true)?;
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        // Each registration takes a descriptor of its own and keeps it for
        // the life of the process.
        signal_hook::low_level::pipe::register(sig, tx.try_clone()?)?;
    }
    Ok(rx)
}
