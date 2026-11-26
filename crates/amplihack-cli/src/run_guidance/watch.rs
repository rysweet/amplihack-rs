//! Waiting on wake FIFOs and a child's exit at once, with no timer
//! (issue #3129, D4).
//!
//! `recipe guidance wait` blocks in [`wait_readable`] on the read ends of its
//! [`super::wake::Waiter`]s and on a [`ChildWatch`]. A `ChildWatch` is a
//! thread that blocks in `waitid(P_PID, pid, WEXITED | WNOWAIT)` and then
//! closes a pipe the main thread polls. `WNOWAIT` leaves the child
//! unreaped, so the main thread stays its only reaper and its PID cannot be
//! reused before the main thread has decided what to do: a `SIGTERM` sent
//! after a wake can only reach the child or its zombie.
//!
//! `waitid` with `WNOWAIT` exists on every target this crate builds for
//! (Linux and macOS), so no liveness-pipe fallback is needed.
//!
//! The pipe comes from [`std::io::pipe`], which opens both ends with
//! `O_CLOEXEC`, so the command holds neither. The only `unsafe` code is in
//! the three small libc wrappers below.

use std::io::{self, ErrorKind, PipeReader, PipeWriter, Write};
use std::os::fd::{AsRawFd, RawFd};

/// Blocks, with no timeout, until at least one of `fds` is readable or hung
/// up, and says which. A call interrupted by a signal returns all `false`,
/// and the caller waits again.
///
/// # Errors
///
/// `poll` failing, or a descriptor that is not open (`POLLNVAL`).
pub(crate) fn wait_readable(fds: &[RawFd]) -> io::Result<Vec<bool>> {
    let mut polled: Vec<libc::pollfd> = fds
        .iter()
        .map(|&fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    let count = libc::nfds_t::try_from(polled.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "too many descriptors to poll"))?;
    // SAFETY: `polled` is a valid, writable array of `count` pollfd structs
    // for the whole call; a timeout of -1 means no timeout.
    let rc = unsafe { libc::poll(polled.as_mut_ptr(), count, -1) };
    if rc < 0 {
        let e = io::Error::last_os_error();
        return if e.kind() == ErrorKind::Interrupted {
            Ok(vec![false; fds.len()])
        } else {
            Err(e)
        };
    }
    polled
        .iter()
        .map(|p| {
            if p.revents & libc::POLLNVAL != 0 {
                Err(io::Error::other("a descriptor given to poll is not open"))
            } else {
                Ok(p.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0)
            }
        })
        .collect()
}

/// Sends `SIGTERM` to process `pid` alone, never to a process group.
///
/// # Errors
///
/// A PID that is not a positive `pid_t` (0 and negative values would name a
/// group), or `kill` failing.
pub(crate) fn terminate(pid: u32) -> io::Result<()> {
    let pid = libc::pid_t::try_from(pid)
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "not a single process id"))?;
    // SAFETY: `pid` is positive, so this names one process; kill has no
    // memory arguments.
    if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Blocks until child `pid` has exited, leaving it unreaped.
fn exited_unreaped(pid: libc::id_t) -> io::Result<()> {
    loop {
        // SAFETY: an all-zero siginfo_t is a valid out-parameter for waitid.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is valid for writes for the whole call.
        let rc =
            unsafe { libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) };
        if rc == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        // A signal handled while blocked: block again; this is not a retry
        // in time.
        if e.kind() != ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// A thread that reports, through a pipe, that a child has exited, without
/// reaping it.
#[derive(Debug)]
pub(crate) struct ChildWatch {
    read: PipeReader,
}

impl ChildWatch {
    /// Starts watching child `pid` of this process. The read end becomes
    /// readable once `waitid` returns, whether the child exited or `waitid`
    /// failed; the caller then reaps with a non-blocking wait and learns
    /// which.
    pub(crate) fn start(pid: u32) -> io::Result<Self> {
        let id = libc::id_t::try_from(pid)
            .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "not a process id"))?;
        let (read, write) = std::io::pipe()?;
        std::thread::Builder::new()
            .name("guidance-wait-child".to_string())
            .spawn(move || report_exit(id, write))?;
        Ok(Self { read })
    }

    /// The read end, for [`wait_readable`].
    pub(crate) fn as_raw_fd(&self) -> RawFd {
        self.read.as_raw_fd()
    }
}

/// The watch thread's body: one byte, then the write end closes.
fn report_exit(pid: libc::id_t, mut write: PipeWriter) {
    let _ = exited_unreaped(pid);
    // The byte makes the read end readable on every platform; closing the
    // write end when `write` drops makes it hung up as well.
    let _ = write.write_all(&[1]);
}
