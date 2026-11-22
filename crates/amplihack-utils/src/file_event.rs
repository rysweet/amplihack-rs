//! One blocking wait on file events and a process exit (issue #2085).
//!
//! A [`Waiter`] holds an inotify descriptor with one watch per directory and
//! a pidfd for one process. [`Waiter::wait`] blocks in a single `poll(2)` on
//! both until a watched name changes, a watched directory goes away, the
//! inotify queue overflows, the process exits or the deadline passes. It
//! never sleeps and never polls on an interval.
//!
//! Issue #2909: [`Waiter::with_interrupt`] adds one caller-supplied readable
//! descriptor to the same `poll(2)`. A signal handler that writes a byte to
//! it ends the wait at once, so a process waiting out a long deadline still
//! answers SIGTERM promptly. The waiter duplicates the descriptor
//! (close-on-exec) and drains it, so the caller keeps its own copy and the
//! next wait does not spin on the same byte.
//!
//! Linux only: elsewhere [`Waiter::new`] fails with `Unsupported`, and a
//! caller has nothing to wait on.

#[cfg(not(target_os = "linux"))]
use std::io;
use std::path::PathBuf;
use std::time::Instant;

/// `IN_MOVED_TO`: a file renamed into a watched directory.
const IN_MOVED_TO: u32 = 0x0000_0080;
/// `IN_CLOSE_WRITE`: a file opened for writing was closed.
const IN_CLOSE_WRITE: u32 = 0x0000_0008;

/// What wakes a wait on the swap record: an atomic rename onto it, or an
/// in-place write.
pub const SWAP_MASK: u32 = IN_MOVED_TO | IN_CLOSE_WRITE;
/// What wakes a wait on prompt markers, which are always renamed in.
pub const PROMPT_MASK: u32 = IN_MOVED_TO;

#[cfg(target_os = "linux")]
const _: () = assert!(IN_MOVED_TO == libc::IN_MOVED_TO && IN_CLOSE_WRITE == libc::IN_CLOSE_WRITE);

/// One watched directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    /// A real directory; a symlink or any other file type is refused.
    pub dir: PathBuf,
    /// The one name in `dir` that counts, compared exactly; `None` for any.
    pub name: Option<&'static str>,
    /// The inotify events that count, such as [`SWAP_MASK`].
    pub mask: u32,
}

/// Why a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Woke {
    /// A matching event in watch `i` (its index in the list given to
    /// [`Waiter::new`]).
    File(usize),
    /// The kernel dropped events: anything may have changed.
    Overflow,
    /// Watch `i` is gone: its directory was removed, moved or unmounted, or
    /// the event stream could not be read.
    Lost(usize),
    /// The watched process exited.
    ProcessGone,
    /// The deadline passed with nothing else, or the interrupt descriptor
    /// given to [`Waiter::with_interrupt`] became readable. The caller
    /// checks its own stop flag, so the two need not be told apart.
    Deadline,
}

/// One `struct inotify_event`, its name without the NUL padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvent {
    pub wd: i32,
    pub mask: u32,
    pub name: Vec<u8>,
}

/// Size of the fixed part of `struct inotify_event`: wd, mask, cookie, len.
const EVENT_HEADER: usize = 16;

/// Parses the bytes of one `read(2)` from an inotify descriptor. `None`
/// when a record is cut short: a header past the end, or a name length
/// past the end. Reads each field with `from_ne_bytes`, so the buffer may
/// have any alignment.
pub fn parse_events(buf: &[u8]) -> Option<Vec<RawEvent>> {
    let field = |at: usize| -> Option<[u8; 4]> { buf.get(at..at.checked_add(4)?)?.try_into().ok() };
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < buf.len() {
        let wd = i32::from_ne_bytes(field(at)?);
        let mask = u32::from_ne_bytes(field(at + 4)?);
        let len = usize::try_from(u32::from_ne_bytes(field(at + 12)?)).ok()?;
        let name_at = at.checked_add(EVENT_HEADER)?;
        let end = name_at.checked_add(len)?;
        let raw = buf.get(name_at..end)?;
        let name_len = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        out.push(RawEvent {
            wd,
            mask,
            name: raw[..name_len].to_vec(),
        });
        at = end;
    }
    Some(out)
}

/// The `poll(2)` timeout from `now` until `deadline`, in milliseconds,
/// rounded up so a wait never ends before the deadline. 0 at or after the
/// deadline, and at most `i32::MAX`: a negative timeout (wait forever) is
/// never produced.
pub fn timeout_ms(now: Instant, deadline: Instant) -> i32 {
    let left = deadline.saturating_duration_since(now);
    let ms = left.as_nanos().div_ceil(1_000_000);
    i32::try_from(ms).unwrap_or(i32::MAX)
}

#[cfg(target_os = "linux")]
pub use linux::Waiter;

#[cfg(target_os = "linux")]
mod linux {
    use super::{RawEvent, Watch, Woke, parse_events, timeout_ms};
    use std::collections::VecDeque;
    use std::ffi::CString;
    use std::io;
    use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::time::Instant;

    /// Events that mean a watch is gone.
    const GONE: u32 =
        libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_IGNORED | libc::IN_UNMOUNT;
    /// Read buffer: one record is at most 16 + NAME_MAX + 1 bytes.
    const BUF_BYTES: usize = 4096;
    /// The most reads one drain of the interrupt descriptor makes.
    const MAX_INTERRUPT_READS: usize = 1024;

    #[repr(C, align(8))]
    struct EventBuf([u8; BUF_BYTES]);

    /// Inotify watches and a pidfd, waited on together.
    #[derive(Debug)]
    pub struct Waiter {
        inotify: OwnedFd,
        pidfd: OwnedFd,
        /// `(wd, name, mask)` per watch, in the caller's order.
        watches: Vec<(i32, Option<&'static str>, u32)>,
        /// Wakes read but not returned yet, most urgent first.
        pending: VecDeque<Woke>,
        /// Issue #2909: this waiter's own copy of the caller's interrupt
        /// descriptor, dropped once the far end closes.
        interrupt: Option<OwnedFd>,
        /// The interrupt became readable and no wake has reported it yet.
        interrupted: bool,
    }

    fn check(rc: libc::c_int) -> io::Result<libc::c_int> {
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(rc)
        }
    }

    impl Waiter {
        /// Adds a watch per entry (a real directory, never through a
        /// symlink, ending on `IN_DELETE_SELF` or `IN_MOVE_SELF`) and opens
        /// a pidfd for `pid`. Every failure is an error; both descriptors
        /// are close-on-exec.
        pub fn new(watches: &[Watch], pid: u32) -> io::Result<Self> {
            Self::build(watches, pid, None)
        }

        /// [`Waiter::new`] with `interrupt` polled beside the watches and
        /// the pidfd (issue #2909). The descriptor is duplicated with
        /// `F_DUPFD_CLOEXEC`, so this waiter owns its copy for as long as it
        /// lives and a child of this process never inherits it. The caller
        /// keeps the original and may go on writing to or reading from it.
        pub fn with_interrupt(
            watches: &[Watch],
            pid: u32,
            interrupt: BorrowedFd<'_>,
        ) -> io::Result<Self> {
            // SAFETY: fcntl with F_DUPFD_CLOEXEC on a borrowed descriptor
            // returns a new descriptor or -1; it reads no memory.
            let raw =
                check(unsafe { libc::fcntl(interrupt.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) })?;
            // SAFETY: `raw` is a new descriptor that nothing else owns.
            let owned = unsafe { OwnedFd::from_raw_fd(raw) };
            Self::build(watches, pid, Some(owned))
        }

        fn build(watches: &[Watch], pid: u32, interrupt: Option<OwnedFd>) -> io::Result<Self> {
            // SAFETY: inotify_init1 takes flags only.
            let raw = check(unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) })?;
            // SAFETY: `raw` is a new descriptor that nothing else owns.
            let inotify = unsafe { OwnedFd::from_raw_fd(raw) };
            let mut added = Vec::with_capacity(watches.len());
            for w in watches {
                let path = CString::new(w.dir.as_os_str().as_bytes())
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
                let mask = w.mask
                    | libc::IN_ONLYDIR
                    | libc::IN_DONT_FOLLOW
                    | libc::IN_DELETE_SELF
                    | libc::IN_MOVE_SELF;
                // SAFETY: a valid descriptor and a NUL-terminated path.
                let wd = check(unsafe {
                    libc::inotify_add_watch(inotify.as_raw_fd(), path.as_ptr(), mask)
                })?;
                if added.iter().any(|(seen, _, _)| *seen == wd) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "two watches on one directory",
                    ));
                }
                added.push((wd, w.name, w.mask));
            }
            let pid = libc::pid_t::try_from(pid)
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad pid"))?;
            // SAFETY: pidfd_open(pid, 0); the result is a descriptor or -1.
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = libc::c_int::try_from(fd)
                .map_err(|_| io::Error::other("pidfd_open returned a bad descriptor"))?;
            // SAFETY: `fd` is a new pidfd (close-on-exec) nothing else owns.
            let pidfd = unsafe { OwnedFd::from_raw_fd(fd) };
            Ok(Self {
                inotify,
                pidfd,
                watches: added,
                pending: VecDeque::new(),
                interrupt,
                interrupted: false,
            })
        }

        /// Whether the process still runs, without blocking.
        pub fn process_alive(&self) -> bool {
            let mut fds = [libc::pollfd {
                fd: self.pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            }];
            loop {
                // SAFETY: one valid pollfd; timeout 0 never blocks.
                let rc = unsafe { libc::poll(fds.as_mut_ptr(), 1, 0) };
                if rc < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return rc == 0;
            }
        }

        /// Blocks until something happens or `deadline` passes. Events for
        /// other names are read and dropped without returning.
        ///
        /// A readable interrupt ([`Waiter::with_interrupt`]) ends the wait
        /// with [`Woke::Deadline`]. File events found in the same `poll(2)`
        /// are returned first and the interrupt on the next call, so neither
        /// is lost.
        pub fn wait(&mut self, deadline: Instant) -> io::Result<Woke> {
            loop {
                if let Some(w) = self.pending.pop_front() {
                    return Ok(w);
                }
                if std::mem::take(&mut self.interrupted) {
                    return Ok(Woke::Deadline);
                }
                let poll = |fd: i32| libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let mut fds = vec![poll(self.pidfd.as_raw_fd()), poll(self.inotify.as_raw_fd())];
                if let Some(i) = &self.interrupt {
                    fds.push(poll(i.as_raw_fd()));
                }
                let n = fds.len();
                let timeout = timeout_ms(Instant::now(), deadline);
                // SAFETY: `n` valid pollfds; the timeout is 0..=i32::MAX.
                let rc = unsafe { libc::poll(fds.as_mut_ptr(), n as libc::nfds_t, timeout) };
                if rc < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e);
                }
                if rc == 0 {
                    return Ok(Woke::Deadline);
                }
                let bad = libc::POLLERR | libc::POLLNVAL;
                if fds[0].revents & bad != 0 || fds[1].revents & bad != 0 {
                    return Err(io::Error::other("poll reported an invalid descriptor"));
                }
                if n > 2 {
                    self.take_interrupt(fds[2].revents);
                }
                if fds[0].revents != 0 {
                    return Ok(Woke::ProcessGone);
                }
                if fds[1].revents != 0 {
                    self.drain()?;
                }
            }
        }

        /// Acts on the interrupt descriptor's `revents`: readable means the
        /// caller signalled, so the bytes are read away and the next wake
        /// reports it. A hang-up or an error on it means the writer is gone
        /// for good, so the descriptor is dropped rather than polled for
        /// ever, and the waiter carries on with its watches alone.
        ///
        /// The two are tested in turn, not as alternatives: a socket whose
        /// peer has closed reports `POLLIN | POLLHUP` and keeps reporting it
        /// once drained, so treating readable as the only case would make
        /// every later wait return at once. What is there is read and
        /// reported once, and then the descriptor goes.
        fn take_interrupt(&mut self, revents: i16) {
            if revents & libc::POLLIN != 0 {
                self.drain_interrupt();
                self.interrupted = true;
            }
            if revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                self.interrupt = None;
            }
        }

        /// Reads the interrupt descriptor empty without blocking: each read
        /// is made only after a zero-timeout `poll(2)` says one byte is
        /// there, so a blocking descriptor is safe. Bounded, so a writer
        /// faster than this loop cannot hold it here.
        fn drain_interrupt(&mut self) {
            let Some(fd) = self.interrupt.as_ref().map(AsRawFd::as_raw_fd) else {
                return;
            };
            let mut buf = [0u8; 64];
            for _ in 0..MAX_INTERRUPT_READS {
                let mut one = [libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                }];
                // SAFETY: one valid pollfd; timeout 0 never blocks.
                let rc = unsafe { libc::poll(one.as_mut_ptr(), 1, 0) };
                if rc < 0 {
                    if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return;
                }
                if rc == 0 || one[0].revents & libc::POLLIN == 0 {
                    return;
                }
                // SAFETY: `buf` is 64 writable bytes, and `poll` has just
                // said a read of this descriptor will not block.
                let got = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                if got < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if got <= 0 {
                    return;
                }
            }
        }

        /// Reads every queued inotify event and queues the wakes.
        fn drain(&mut self) -> io::Result<()> {
            let mut buf = EventBuf([0; BUF_BYTES]);
            let mut lost = Vec::new();
            let mut overflow = false;
            let mut files = Vec::new();
            loop {
                // SAFETY: `buf` is BUF_BYTES of writable memory.
                let n = unsafe {
                    libc::read(
                        self.inotify.as_raw_fd(),
                        buf.0.as_mut_ptr().cast(),
                        BUF_BYTES,
                    )
                };
                if n < 0 {
                    let e = io::Error::last_os_error();
                    match e.kind() {
                        io::ErrorKind::Interrupted => continue,
                        io::ErrorKind::WouldBlock => break,
                        _ => return Err(e),
                    }
                }
                if n == 0 {
                    break;
                }
                let n = usize::try_from(n).unwrap_or(0).min(BUF_BYTES);
                let Some(events) = parse_events(&buf.0[..n]) else {
                    // S9: a malformed record ends the wait without a resume.
                    lost.push(0);
                    break;
                };
                for e in events {
                    self.classify(&e, &mut lost, &mut overflow, &mut files);
                }
            }
            lost.sort_unstable();
            files.sort_unstable();
            let wakes = lost
                .into_iter()
                .map(Woke::Lost)
                .chain(overflow.then_some(Woke::Overflow))
                .chain(files.into_iter().map(Woke::File));
            for w in wakes {
                if !self.pending.contains(&w) {
                    self.pending.push_back(w);
                }
            }
            Ok(())
        }

        fn classify(
            &self,
            e: &RawEvent,
            lost: &mut Vec<usize>,
            overflow: &mut bool,
            files: &mut Vec<usize>,
        ) {
            if e.mask & libc::IN_Q_OVERFLOW != 0 {
                *overflow = true;
                return;
            }
            let Some(i) = self.watches.iter().position(|(wd, _, _)| *wd == e.wd) else {
                return;
            };
            let (_, name, mask) = self.watches[i];
            if e.mask & GONE != 0 {
                lost.push(i);
            } else if e.mask & mask != 0
                && !e.name.is_empty()
                && name.is_none_or(|n| e.name == n.as_bytes())
            {
                files.push(i);
            }
        }
    }
}

/// Off Linux there is no inotify or pidfd: nothing can be waited on.
#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
pub struct Waiter {
    _private: (),
}

#[cfg(not(target_os = "linux"))]
impl Waiter {
    /// Always `Unsupported`.
    pub fn new(_watches: &[Watch], _pid: u32) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "file events need Linux",
        ))
    }

    /// Always `Unsupported`, as [`Waiter::new`] is.
    #[cfg(unix)]
    pub fn with_interrupt(
        _watches: &[Watch],
        _pid: u32,
        _interrupt: std::os::fd::BorrowedFd<'_>,
    ) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "file events need Linux",
        ))
    }

    /// Never reached: no `Waiter` can be built.
    pub fn process_alive(&self) -> bool {
        false
    }

    /// Never reached: no `Waiter` can be built.
    pub fn wait(&mut self, _deadline: Instant) -> io::Result<Woke> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "file events need Linux",
        ))
    }
}
