//! Coalescible readiness only; completion data lives in the worker queue.
use super::Event;
use crate::commands::profile::ProfileError;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

struct Shared {
    write: OwnedFd,
    pending: AtomicU8,
}
#[derive(Clone)]
pub(crate) struct WakeHandle(Arc<Shared>);
pub(crate) struct WakePipe {
    read: OwnedFd,
    handle: WakeHandle,
}
impl WakeHandle {
    fn notify(&self, flag: u8) -> io::Result<()> {
        self.0.pending.fetch_or(flag, Ordering::Release);
        loop {
            // SAFETY: live write fd, one byte; async-signal-safe for shutdown.
            let n = unsafe { libc::write(self.0.write.as_raw_fd(), b"w".as_ptr().cast(), 1) };
            if n == 1 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::WouldBlock => return Ok(()), // already readable
                io::ErrorKind::Interrupted => continue,
                _ => return Err(e),
            }
        }
    }
    pub(crate) fn completion(&self) -> io::Result<()> {
        self.notify(2)
    }
    pub(crate) fn stop(&self) -> io::Result<()> {
        self.notify(1)
    }
}
impl WakePipe {
    pub(crate) fn open() -> Result<Self, ProfileError> {
        let mut fds = [-1; 2];
        // SAFETY: a two-element writable descriptor array.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: pipe just transferred ownership of both descriptors.
        let [read, write] = unsafe { [OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])] };
        for fd in [&read, &write] {
            // SAFETY: these descriptors are private until this constructor returns.
            if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1
                || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } == -1
            {
                return Err(io::Error::last_os_error().into());
            }
        }
        Ok(Self {
            read,
            handle: WakeHandle(Arc::new(Shared {
                write,
                pending: AtomicU8::new(0),
            })),
        })
    }
    pub(crate) fn fd(&self) -> RawFd {
        self.read.as_raw_fd()
    }
    pub(crate) fn handle(&self) -> WakeHandle {
        self.handle.clone()
    }
    #[cfg(test)]
    pub(crate) fn raw_fds(&self) -> [RawFd; 2] {
        [self.fd(), self.handle.0.write.as_raw_fd()]
    }
    pub(crate) fn take(&self) -> io::Result<Vec<Event>> {
        let mut bytes = [0u8; 256];
        loop {
            // SAFETY: live read fd and writable local buffer.
            let n = unsafe { libc::read(self.fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
            if n > 0 {
                continue;
            }
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "watch readiness pipe closed",
                ));
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::WouldBlock => break,
                io::ErrorKind::Interrupted => continue,
                _ => return Err(e),
            }
        }
        let bits = self.handle.0.pending.swap(0, Ordering::AcqRel);
        let mut out = Vec::new();
        if bits & 2 != 0 {
            out.push(Event::Completion);
        }
        if bits & 1 != 0 {
            out.push(Event::Stop);
        }
        Ok(out)
    }
}
