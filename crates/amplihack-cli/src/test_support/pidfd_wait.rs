//! Wait on and kill a process this test did not spawn, through a pidfd
//! (issue #2019, Linux only).
//!
//! A pid read from a file can be reused once its process is reaped. A pidfd
//! opened while the process is known to be alive keeps referring to that
//! process, so waiting on it and signalling through it can never reach a
//! stranger. The wait is the pidfd becoming readable (the process exited),
//! bounded only by [`CEILING`], which fails the test as hung.

use super::wait::CEILING;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// Parses a pid written by `echo $$`: ASCII digits and at most one trailing
/// newline. Refuses 0 (the process group), 1 (init), this test's own pid and
/// anything beyond `pid_t`.
pub(crate) fn parse_pid(raw: &str) -> Result<i32, String> {
    let digits = raw.strip_suffix('\n').unwrap_or(raw);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("not a pid: {raw:?}"));
    }
    let pid: i32 = digits
        .parse()
        .map_err(|e| format!("pid {digits:?} out of range: {e}"))?;
    if pid == 0 || pid == 1 {
        return Err(format!("refusing pid {pid}"));
    }
    if u32::try_from(pid).ok() == Some(std::process::id()) {
        return Err("refusing the test's own pid".to_string());
    }
    Ok(pid)
}

/// A pidfd for one process.
#[derive(Debug)]
pub(crate) struct PidFd(OwnedFd);

impl PidFd {
    /// Opens a pidfd for `pid`. `ESRCH` means there is no such process.
    pub(crate) fn open(pid: i32) -> io::Result<Self> {
        // SAFETY: pidfd_open takes a pid and flags and returns a new fd or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = i32::try_from(fd).map_err(|_| io::Error::other("pidfd out of range"))?;
        // SAFETY: `fd` was just returned by pidfd_open and is owned by nobody else.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Sends `signal` through the pidfd. `ESRCH` (already exited) is `Ok`.
    pub(crate) fn signal(&self, signal: i32) -> io::Result<()> {
        // SAFETY: pidfd_send_signal on an fd we own, with no siginfo.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.0.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if rc < 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ESRCH) {
                return Err(e);
            }
        }
        Ok(())
    }
}

/// Blocks until the process behind `fd` has exited.
pub(crate) fn wait_pid_gone(fd: &PidFd, what: &str) {
    let mut pfd = libc::pollfd {
        fd: fd.0.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ceiling_ms = i32::try_from(CEILING.as_millis()).unwrap_or(i32::MAX);
    loop {
        // SAFETY: one live pollfd.
        let rc = unsafe { libc::poll(&mut pfd, 1, ceiling_ms) };
        match rc {
            1.. => return,
            0 => panic!("hung: waiting for {what} for {CEILING:?}"),
            _ => {
                let e = io::Error::last_os_error();
                assert!(
                    e.kind() == io::ErrorKind::Interrupted,
                    "waiting for {what}: poll on the pidfd failed: {e}"
                );
            }
        }
    }
}

/// Kills the process behind the pidfd when dropped, so a failed assertion
/// cannot leave it running. A process that already exited is left alone.
pub(crate) struct KillOnDrop(pub(crate) PidFd);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Err(e) = self.0.signal(libc::SIGKILL) {
            eprintln!("killing a test's orphan through its pidfd failed: {e}");
        }
    }
}
