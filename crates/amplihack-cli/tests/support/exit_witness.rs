//! Issue #2931 (TIMING): the end of file on a pipe as the event "the process
//! group is gone".
//!
//! A test that starts a stub which leaves descendants behind used to wait for
//! the clock (`sleep` until a marker could have appeared) or read the host's
//! `/proc` to see whether the descendant had died. Both depend on the host.
//! Here the descendants hold the write end of a named pipe the test made, and
//! the read end sees end of file when the last holder has gone, however the
//! holders ended: no process table, no sleep.
//!
//! Usage: make an [`ExitWitness`] in the test's own folder, put
//! [`ExitWitness::stub_lines`] at the top of the stub script (every process the
//! stub starts afterwards inherits the pipe), run the code under test, then
//! call [`ExitWitness::wait_released`]. The ceiling it takes bounds a failure
//! only: a pass returns as soon as the pipe closes.
//!
//! The stub writes `held` into the pipe before anything else, so a stub that
//! never held the pipe cannot pass for one whose holders were reaped: a pass
//! must report `held == true`.
//!
//! The pipe is made with mode 0600 inside the folder given, which must be the
//! test's own, and its name is fixed: making a second witness in one folder
//! fails on purpose. This file uses only `std` and `libc`.

use std::ffi::CString;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long one `poll` may block before the pipe is read again. The wake-up
/// on an event is immediate; this only covers a platform that does not report
/// a closed pipe through `poll`.
const SLICE: Duration = Duration::from_millis(100);

/// The pipe and its read end, opened before the stub runs.
pub struct ExitWitness {
    fifo: PathBuf,
    read: File,
    seen: std::cell::RefCell<Vec<u8>>,
}

/// What [`ExitWitness::wait_released`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The last holder closed the pipe. `held` is whether the stub's `held`
    /// token was read first; `after` is how long this call waited.
    Released { held: bool, after: Duration },
    /// A holder was still there when the ceiling passed.
    StillHeld,
}

impl ExitWitness {
    /// Makes the pipe `<dir>/exit-witness` (mode 0600) and opens its read end
    /// without blocking, so the stub's later open for writing cannot block.
    pub fn new(dir: &Path) -> Self {
        let fifo = dir.join("exit-witness");
        let text = fifo.to_str().expect("the folder name is UTF-8");
        assert!(
            !text.contains(['\'', '\n']),
            "cannot single-quote {text:?} for /bin/sh"
        );
        let c = CString::new(fifo.as_os_str().as_bytes()).expect("no NUL in the path");
        // SAFETY: `c` is a valid NUL-terminated path for the whole call.
        let made = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
        assert_eq!(
            made,
            0,
            "mkfifo {}: {}",
            fifo.display(),
            std::io::Error::last_os_error()
        );
        // SAFETY: as above; the descriptor is checked and then owned by `File`.
        let fd = unsafe {
            libc::open(
                c.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        assert!(
            fd >= 0,
            "open {}: {}",
            fifo.display(),
            std::io::Error::last_os_error()
        );
        // SAFETY: `fd` is a fresh, open descriptor that nothing else owns.
        let read = unsafe { File::from_raw_fd(fd) };
        Self {
            fifo,
            read,
            seen: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// One line of shell for the top of the stub: open the pipe for writing on
    /// descriptor 9, which every later process of the stub inherits, and write
    /// the `held` token. It has no trailing newline or semicolon, so it can be
    /// joined with `;` or put on its own line.
    pub fn stub_lines(&self) -> String {
        let fifo = self.fifo.to_str().expect("checked in new");
        format!("exec 9>'{fifo}'; printf 'held\\n' >&9")
    }

    /// Waits until every holder of the pipe has closed it, or `ceiling` passes.
    pub fn wait_released(&self, ceiling: Duration) -> Outcome {
        let started = Instant::now();
        let mut read = &self.read;
        loop {
            let mut buffer = [0u8; 64];
            match read.read(&mut buffer) {
                Ok(0) => {
                    return Outcome::Released {
                        held: self.seen.borrow().starts_with(b"held\n"),
                        after: started.elapsed(),
                    };
                }
                Ok(n) => {
                    self.seen.borrow_mut().extend_from_slice(&buffer[..n]);
                    continue;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => panic!("read {}: {e}", self.fifo.display()),
            }
            let Some(left) = ceiling.checked_sub(started.elapsed()) else {
                return Outcome::StillHeld;
            };
            let mut poll = libc::pollfd {
                fd: self.read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let wait = left.min(SLICE).as_millis().max(1) as libc::c_int;
            // SAFETY: one valid `pollfd` that lives across the call.
            unsafe { libc::poll(&mut poll, 1, wait) };
        }
    }
}
