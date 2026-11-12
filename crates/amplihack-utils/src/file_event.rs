//! One blocking wait on file events and a process exit (issue #2085).
//!
//! A [`Waiter`] holds an inotify descriptor with one watch per directory and
//! a pidfd for one process. [`Waiter::wait`] blocks in a single `poll(2)` on
//! both until a watched name changes, a watched directory goes away, the
//! inotify queue overflows, the process exits or the deadline passes. It
//! never sleeps and never polls on an interval.
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
    /// The deadline passed with nothing else.
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
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::time::Instant;

    /// Events that mean a watch is gone.
    const GONE: u32 =
        libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_IGNORED | libc::IN_UNMOUNT;
    /// Read buffer: one record is at most 16 + NAME_MAX + 1 bytes.
    const BUF_BYTES: usize = 4096;

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
        pub fn wait(&mut self, deadline: Instant) -> io::Result<Woke> {
            loop {
                if let Some(w) = self.pending.pop_front() {
                    return Ok(w);
                }
                let mut fds = [
                    libc::pollfd {
                        fd: self.pidfd.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: self.inotify.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                let timeout = timeout_ms(Instant::now(), deadline);
                // SAFETY: two valid pollfds; the timeout is 0..=i32::MAX.
                let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
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
                if fds[0].revents != 0 {
                    return Ok(Woke::ProcessGone);
                }
                if fds[1].revents != 0 {
                    self.drain()?;
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
