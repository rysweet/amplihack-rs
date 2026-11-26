//! The room gate of a standing session's relaunch (issue #2912).
//!
//! A standing session whose process exits is started again, but a session
//! that stopped because every login was spent must not be started into the
//! same wall. The wrapper therefore reads the rotator's swap record at the
//! moment of exit and, when no login has room, blocks on the swap file
//! until one does. The wait is one `poll(2)` on an inotify descriptor
//! through [`amplihack_utils::file_event::Waiter`], bounded only by the
//! existing [`MAX_WAIT`]: there is no interval and no probe of its own.
//!
//! A user with no rotator is never delayed: a missing record, one the
//! reader refuses and one that cannot be parsed all mean "no rotator", and
//! the session is started again at once.
//!
//! The record is written only when the rotator swaps, so a record that
//! shows room may be older than the run and blind to a limit the run has
//! just met. The session is relaunched on it all the same. `claude
//! --continue` sends no prompt, so the relaunched session waits at its
//! prompt instead of meeting the limit again.

use amplihack_utils::file_event::{SWAP_MASK, Waiter, Watch, Woke};
use amplihack_utils::rotator_status::MAX_WAIT;
use amplihack_utils::rotator_status::swap_record::{SWAP_FILE, read_swap, rotate_dir};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// How the wait for a login with room ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Room {
    /// A login had room already, or there is no rotator to wait for.
    Now,
    /// No login had room when the process exited; one appeared afterwards,
    /// either on the swap event or on the read taken once the watch was
    /// armed, and the relaunch follows it.
    Waited,
    /// The wrapper had SIGHUP or SIGTERM before the wait could block: the
    /// pane or the host is going away.
    Stopped,
    /// The wait ended with no room and no signal, for the reason given.
    Gone(Lost),
}

/// Why a wait for room ended without one. Each leaves a standing session
/// stopped that its owner wants running, so the wrapper puts it on the
/// session's record for the supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lost {
    /// The rotator's directory is missing, a symbolic link, not this
    /// user's, or writable by others, so no event could ever arrive.
    Unwatchable,
    /// The watch on the swap record could not be set up.
    Unarmed,
    /// The watch was lost, or the wait itself failed.
    Watch,
    /// [`MAX_WAIT`] passed with no login with room.
    Deadline,
}

/// Blocks until a login with room appears under `root`, or `stopped` is
/// already set, or [`MAX_WAIT`] passes, or the swap record cannot be
/// watched; [`Room`] says which.
///
/// `Room::Now` without any wait when a login has room, and also when there
/// is no record, the reader refuses it, or it cannot be parsed: an attacker
/// who can only delete or damage the file gets the behaviour of a host with
/// no rotator, never a session held back.
///
/// While it blocks, `SIGHUP`, `SIGTERM` and `SIGINT` are put back to their
/// default disposition, so closing the pane or pressing Ctrl-C ends the
/// wrapper rather than leaving it waiting for up to seven days behind a
/// signal handler. `stopped` is read again after that change and before
/// the block, so a `SIGHUP` or `SIGTERM` that arrived in between is not
/// lost.
///
/// `on_wait` is called once, after the watch is armed and the record has
/// been read for the last time, when the session really is about to be
/// held: the wrapper writes its waiting line there, so the line appears
/// only for a session that waits, and never after the wait has ended.
pub fn wait_for_room_until(root: &Path, stopped: &AtomicBool, on_wait: impl FnOnce()) -> Room {
    if room_now(root) {
        return Room::Now;
    }
    let dir = rotate_dir(root);
    if !watchable(&dir) {
        // Nothing to wait on: the directory the rotator writes in is gone
        // or is not ours, so no event can ever arrive.
        return Room::Gone(Lost::Unwatchable);
    }
    let watches = [Watch {
        dir,
        name: Some(SWAP_FILE),
        mask: SWAP_MASK,
    }];
    // The pidfd is this process: it names something that certainly exists
    // and never exits while the wait runs, so the wait ends only on a file
    // event or the deadline.
    let mut waiter = match Waiter::new(&watches, std::process::id()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("warning: standing session: cannot watch the swap record: {e}");
            return Room::Gone(Lost::Unarmed);
        }
    };
    let Some(deadline) = Instant::now().checked_add(MAX_WAIT) else {
        return Room::Gone(Lost::Deadline);
    };
    let _default_signals = DefaultSignals::install();
    if stopped.load(Ordering::SeqCst) {
        return Room::Stopped;
    }
    // Read once more now the watch is armed. A record written between the
    // read at the top and `Waiter::new` reaches neither, and the session
    // would then be held for up to seven days by room it already had.
    // Everything written from here on wakes the block below.
    if room_now(root) {
        return Room::Waited;
    }
    on_wait();
    loop {
        match waiter.wait(deadline) {
            // An overflow may have dropped the write of a new record, so
            // the record is read again rather than trusted to come later.
            Ok(Woke::File(_) | Woke::Overflow) => {
                if room_now(root) {
                    return Room::Waited;
                }
            }
            Ok(Woke::Deadline) => return Room::Gone(Lost::Deadline),
            // The pidfd is this process, so this cannot happen while the
            // wait runs; were it to, the wait has nothing left to end it.
            Ok(Woke::ProcessGone) => return Room::Gone(Lost::Watch),
            Ok(Woke::Lost(_)) => {
                eprintln!("warning: standing session: the watch on the swap record was lost");
                return Room::Gone(Lost::Watch);
            }
            Err(e) => {
                eprintln!("warning: standing session: the wait for room failed: {e}");
                return Room::Gone(Lost::Watch);
            }
        }
    }
}

/// Whether a session may start now: a login with room, or no rotator whose
/// record this process can read and understand.
///
/// Room is [`SwapRecord::has_room`], the rule the `limit-resume` hook's
/// `judge_at` uses. The rest of `judge_at` is about a session that is still
/// running (which record was on disk when the hook started, and whether the
/// swap came after the stop); a process that has already exited has no such
/// history, so only the room half applies here.
///
/// [`SwapRecord::has_room`]: amplihack_utils::rotator_status::swap_record::SwapRecord::has_room
fn room_now(root: &Path) -> bool {
    read_swap(root, crate::commands::agent::registry::now()).is_none_or(|r| r.has_room())
}

/// A real directory of this user that no one else may write to; the same
/// rule the hook applies before it watches the rotate directory.
fn watchable(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir).is_ok_and(|m| {
        !m.file_type().is_symlink()
            && m.is_dir()
            && m.uid() == amplihack_utils::secure_files::euid()
            && m.mode() & 0o022 == 0
    })
}

/// `SIGHUP`, `SIGTERM` and `SIGINT` back to `SIG_DFL` while this is alive,
/// and back to whatever was installed before when it is dropped.
///
/// The wrapper watches `SIGHUP` and `SIGTERM` with a flag so it can tell a
/// pane going away from a session ending, and every launch adds a flag for
/// all three so it can stop its child. Those handlers keep the process
/// alive, which is right while Claude Code runs and wrong while the wrapper
/// is blocked for up to seven days waiting for room: a closed pane, or the
/// owner's Ctrl-C, must end it.
struct DefaultSignals {
    saved: Vec<(libc::c_int, libc::sigaction)>,
}

impl DefaultSignals {
    fn install() -> Self {
        let mut saved = Vec::new();
        for sig in [libc::SIGHUP, libc::SIGTERM, libc::SIGINT] {
            // SAFETY: `sigaction` with a valid signal number and two owned,
            // correctly sized `struct sigaction` values. `SIG_DFL` is a
            // valid handler for each of them.
            unsafe {
                let mut old: libc::sigaction = std::mem::zeroed();
                let mut new: libc::sigaction = std::mem::zeroed();
                new.sa_sigaction = libc::SIG_DFL;
                if libc::sigemptyset(&mut new.sa_mask) == 0
                    && libc::sigaction(sig, &new, &mut old) == 0
                {
                    saved.push((sig, old));
                }
            }
        }
        Self { saved }
    }
}

impl Drop for DefaultSignals {
    fn drop(&mut self) {
        for (sig, old) in &self.saved {
            // SAFETY: `old` is the disposition this process had, read by
            // `sigaction` above and unchanged since.
            unsafe {
                libc::sigaction(*sig, old, std::ptr::null_mut());
            }
        }
    }
}
