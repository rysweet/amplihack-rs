//! What a usage-limit wait blocks on (issue #2911).
//!
//! One blocking call per wake, on everything a wait can end on:
//!
//! - a write to a file the wait's checks read ([`Watch::files`]): inotify on
//!   Linux, kqueue on macOS;
//! - a stop signal (SIGTERM, SIGHUP, SIGINT), through the socket the step's
//!   signal handlers write a byte to (`attempt::Signals`);
//! - the exit of the process that started the step: a pidfd on Linux,
//!   `EVFILT_PROC` on macOS;
//! - the time the wait asks for, which is its deadline or its next progress
//!   line. The progress wake reads no file.
//!
//! A directory that does not exist yet is watched through the nearest one
//! above it that does, for any name, so a rotator or a login that appears
//! after the wait began is still seen. The watches are made again whenever
//! the directories they need change.
//!
//! A file can be rewritten many times a second (`$HOME/.claude.json` is
//! written by every Claude session on the host), so writes that arrive
//! within [`READ_SPACING`] of the last read are gathered into one read. The
//! spacing starts only from a write and only delays the read that write
//! asked for, blocking on events while it lasts; nothing is read without a
//! write, and nothing is slept.
//!
//! Where no file event can be had (another platform, or a host out of
//! inotify instances), the wait says so once, on a `no_file_events` line and
//! in a sentence beside it, and reads nothing more: it cannot see a login
//! gain room or change, so it ends on its reset, a stop signal or the end of
//! its session. It does not stand in a timer for the events it lacks.

use super::events::{Event, log};
use super::fingerprint::Watch;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The least time between two reads that writes asked for.
pub(super) const READ_SPACING: Duration = Duration::from_secs(1);

/// How far up a missing directory's ancestors are looked for one to watch.
const MAX_ANCESTORS: usize = 8;

/// One directory to watch, and the one name in it that counts (`None` for
/// any name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Watched {
    dir: PathBuf,
    name: Option<&'static str>,
}

/// What woke one platform wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Woke {
    /// A watched file was written, created, removed or renamed.
    Files,
    /// A watched directory went away, or events were lost: make the watches
    /// again and read everything.
    Rewatch,
    /// The process that started the step exited.
    ProcessGone,
    /// The time came, or a stop signal arrived; the wait checks both.
    Time,
}

/// The wait's event source, for as long as one wait lasts.
pub(super) struct Wake<'a> {
    /// The read end of the socket the stop-signal handlers write to, owned
    /// by the step's `Signals`.
    interrupt: &'a UnixStream,
    files: Vec<(PathBuf, &'static str)>,
    parent: libc::pid_t,
    parent_gone: bool,
    events: Option<platform::Events>,
    /// When the last read that a write asked for was allowed.
    last_read: Option<Instant>,
    /// Whether the `no_file_events` line has been written.
    noted: bool,
}

impl<'a> Wake<'a> {
    /// The event source for a wait on `watch`'s files, for a step started
    /// by `parent`, woken by a stop signal through `interrupt`.
    pub(super) fn new(watch: &Watch, parent: libc::pid_t, interrupt: &'a UnixStream) -> Self {
        let mut wake = Self {
            interrupt,
            files: watch.files(),
            parent,
            parent_gone: false,
            events: None,
            last_read: None,
            noted: false,
        };
        wake.rewatch();
        wake
    }

    /// Whether the process that started the step has exited.
    pub(super) fn parent_gone(&self) -> bool {
        self.parent_gone
    }

    /// Blocks for at most `d` (real time). True when a watched file may have
    /// changed and the wait should read its files again. `spacing` is
    /// [`READ_SPACING`] on the wait's clock.
    pub(super) fn block(&mut self, d: Duration, spacing: Duration) -> bool {
        let began = Instant::now();
        let Some(events) = self.events.as_mut() else {
            return self.block_without_events(d);
        };
        match events.wait(d) {
            Ok(Woke::Files) => {
                if self.watched_now() != *self.events_dirs() {
                    self.rewatch();
                }
                self.settle(spacing, d.saturating_sub(began.elapsed()));
                true
            }
            Ok(Woke::Rewatch) => {
                self.rewatch();
                true
            }
            Ok(Woke::ProcessGone) => {
                self.parent_gone = true;
                false
            }
            Ok(Woke::Time) => {
                self.drain_interrupt();
                false
            }
            // The event source failed. It is not made again here, since a
            // source that fails each time it is made would turn this into a
            // loop of reads; the files are read once, for any write it may
            // have missed, and the wait goes on without file events.
            Err(_) => {
                self.events = None;
                self.note("wait_failed");
                true
            }
        }
    }

    fn events_dirs(&self) -> &[Watched] {
        self.events.as_ref().map_or(&[], platform::Events::dirs)
    }

    fn watched_now(&self) -> Vec<Watched> {
        watched(&self.files)
    }

    /// Gathers writes that arrive within `spacing` of the last read into
    /// this one, waiting no longer than `left`. A stop signal or the step's
    /// session ending cuts it short; the wait checks both first.
    fn settle(&mut self, spacing: Duration, left: Duration) {
        if let (Some(last), Some(events)) = (self.last_read, self.events.as_mut()) {
            let until = Instant::now() + spacing.saturating_sub(last.elapsed()).min(left);
            loop {
                let rest = until.saturating_duration_since(Instant::now());
                if rest.is_zero() {
                    break;
                }
                match events.wait(rest) {
                    Ok(Woke::Files | Woke::Rewatch) => {}
                    Ok(Woke::ProcessGone) => {
                        self.parent_gone = true;
                        break;
                    }
                    Ok(Woke::Time) | Err(_) => break,
                }
            }
        }
        self.last_read = Some(Instant::now());
    }

    /// Makes the watches again for the directories the files need now.
    fn rewatch(&mut self) {
        self.events = None;
        // SAFETY: getppid(2) has no preconditions. A step whose session has
        // already ended stops at its first check, so nothing is watched.
        if unsafe { libc::getppid() } != self.parent {
            return;
        }
        let dirs = watched(&self.files);
        match platform::Events::new(&dirs, &self.files, self.parent, self.interrupt) {
            Ok(events) => self.events = Some(events),
            Err(e) if e.kind() == io::ErrorKind::Unsupported => self.note("unsupported"),
            Err(_) => self.note("watch_failed"),
        }
    }

    /// Writes the `no_file_events` line, and the sentence beside it, once
    /// per wait.
    fn note(&mut self, why: &'static str) {
        if !std::mem::replace(&mut self.noted, true) {
            log(&Event::NoFileEvents { why });
            eprintln!("{NO_FILE_EVENTS}");
        }
    }

    /// With no file events: blocks until `d` passes or a stop signal comes.
    /// Never asks for the files to be read, since no write can have been
    /// seen; the wait checks the time, the signal and its session itself.
    fn block_without_events(&mut self, d: Duration) -> bool {
        let mut fds = [libc::pollfd {
            fd: self.interrupt.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        let ms = i32::try_from(d.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: one valid pollfd; the timeout is 0..=i32::MAX.
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), 1, ms) };
        if rc > 0 {
            self.drain_interrupt();
        }
        false
    }

    /// Reads away the bytes the signal handlers wrote, so the next wake does
    /// not return on the same signal.
    fn drain_interrupt(&mut self) {
        let mut sock = self.interrupt;
        let mut buf = [0u8; 64];
        while matches!(sock.read(&mut buf), Ok(n) if n > 0) {}
    }
}

/// What a wait with no file events prints beside its `no_file_events` line:
/// static text, read by a person deciding whether to act.
const NO_FILE_EVENTS: &str = "amplihack: this wait cannot watch its files, so it cannot see \
                              another login gain room or the login change; it ends at its \
                              reset, on a stop signal or when its session ends, and a wait \
                              whose limit stated no reset waits for one of the last two. See \
                              docs/reference/usage-limit-waits.md";

/// The directories to watch for `files`, each once.
fn watched(files: &[(PathBuf, &'static str)]) -> Vec<Watched> {
    let mut out: Vec<Watched> = Vec::new();
    for (dir, name) in files {
        let Some(w) = nearest(dir, name) else {
            continue;
        };
        match out.iter_mut().find(|o| o.dir == w.dir) {
            Some(o) if o.name != w.name => o.name = None,
            Some(_) => {}
            None => out.push(w),
        }
    }
    out
}

/// What to watch for `name` in `dir`: `dir` itself when it is a directory,
/// else the nearest directory above it, for any name, since what appears
/// there is the way down to `dir`. Paths are resolved, so a symlinked
/// directory is watched where it points.
fn nearest(dir: &Path, name: &'static str) -> Option<Watched> {
    let real = |p: &Path| std::fs::canonicalize(p).ok().filter(|r| r.is_dir());
    if let Some(dir) = real(dir) {
        return Some(Watched {
            dir,
            name: Some(name),
        });
    }
    dir.ancestors()
        .skip(1)
        .take(MAX_ANCESTORS)
        .find_map(real)
        .map(|dir| Watched { dir, name: None })
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{Watched, Woke};
    use amplihack_utils::file_event::{self, Waiter};
    use std::io;
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// What counts as a write to a watched name: an in-place write closed,
    /// a rename in or out, a create and a removal.
    const MASK: u32 = libc::IN_CLOSE_WRITE
        | libc::IN_MOVED_TO
        | libc::IN_MOVED_FROM
        | libc::IN_CREATE
        | libc::IN_DELETE;

    pub(super) struct Events {
        waiter: Waiter,
        dirs: Vec<Watched>,
    }

    impl Events {
        pub(super) fn new(
            dirs: &[Watched],
            _files: &[(PathBuf, &'static str)],
            parent: libc::pid_t,
            interrupt: &UnixStream,
        ) -> io::Result<Self> {
            let pid = u32::try_from(parent)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad pid"))?;
            let watches: Vec<file_event::Watch> = dirs
                .iter()
                .map(|w| file_event::Watch {
                    dir: w.dir.clone(),
                    name: w.name,
                    mask: MASK,
                })
                .collect();
            let waiter = Waiter::with_interrupt(&watches, pid, interrupt.as_fd())?;
            Ok(Self {
                waiter,
                dirs: dirs.to_vec(),
            })
        }

        pub(super) fn dirs(&self) -> &[Watched] {
            &self.dirs
        }

        pub(super) fn wait(&mut self, d: Duration) -> io::Result<Woke> {
            // A deadline past what an `Instant` can hold is a day away: the
            // wait asks again for its progress line long before that.
            let deadline = Instant::now()
                .checked_add(d)
                .unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
            Ok(match self.waiter.wait(deadline)? {
                file_event::Woke::File(_) | file_event::Woke::Overflow => Woke::Files,
                file_event::Woke::Lost(_) => Woke::Rewatch,
                file_event::Woke::ProcessGone => Woke::ProcessGone,
                file_event::Woke::Deadline => Woke::Time,
            })
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{Watched, Woke};
    use std::ffi::CString;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// Vnode events that count: a write or growth, and the ways a watched
    /// file or directory stops being the one at its path.
    const NOTES: u32 = libc::NOTE_WRITE
        | libc::NOTE_EXTEND
        | libc::NOTE_DELETE
        | libc::NOTE_RENAME
        | libc::NOTE_REVOKE;
    /// The vnode events after which a watch must be made again.
    const GONE: u32 = libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE;

    pub(super) struct Events {
        kq: OwnedFd,
        /// The watched directories and files, held open for their watches.
        _held: Vec<OwnedFd>,
        dirs: Vec<Watched>,
    }

    fn check(rc: libc::c_int) -> io::Result<libc::c_int> {
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(rc)
        }
    }

    /// Opens `path` for events only (`O_EVTONLY`), never through a final
    /// symlink.
    fn open_events(path: &Path, dir: bool) -> io::Result<OwnedFd> {
        let c = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
        let mut flags = libc::O_EVTONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        if dir {
            flags |= libc::O_DIRECTORY;
        }
        // SAFETY: a NUL-terminated path and plain flags.
        let fd = check(unsafe { libc::open(c.as_ptr(), flags) })?;
        // SAFETY: `fd` is a new descriptor nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn add(kq: RawFd, ident: usize, filter: i16, fflags: u32) -> io::Result<()> {
        let change = libc::kevent {
            ident,
            filter,
            flags: libc::EV_ADD | libc::EV_CLEAR,
            fflags,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: one valid change, no event list, no timeout.
        check(unsafe { libc::kevent(kq, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) })
            .map(drop)
    }

    impl Events {
        pub(super) fn new(
            dirs: &[Watched],
            files: &[(PathBuf, &'static str)],
            parent: libc::pid_t,
            interrupt: &UnixStream,
        ) -> io::Result<Self> {
            // SAFETY: kqueue(2) takes no arguments.
            let raw = check(unsafe { libc::kqueue() })?;
            // SAFETY: `raw` is a new descriptor nothing else owns.
            let kq = unsafe { OwnedFd::from_raw_fd(raw) };
            // SAFETY: fcntl on our own descriptor.
            check(unsafe { libc::fcntl(kq.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) })?;
            let mut held = Vec::new();
            for w in dirs {
                let fd = open_events(&w.dir, true)?;
                add(
                    kq.as_raw_fd(),
                    fd.as_raw_fd() as usize,
                    libc::EVFILT_VNODE,
                    NOTES,
                )?;
                held.push(fd);
            }
            // A file written in place changes no directory entry, so each
            // file that is there now is watched as well. One that is
            // missing is seen by its directory's watch when it appears.
            for (dir, name) in files {
                if let Ok(fd) = open_events(&dir.join(name), false) {
                    add(
                        kq.as_raw_fd(),
                        fd.as_raw_fd() as usize,
                        libc::EVFILT_VNODE,
                        NOTES | libc::NOTE_ATTRIB,
                    )?;
                    held.push(fd);
                }
            }
            let pid = usize::try_from(parent)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad pid"))?;
            add(kq.as_raw_fd(), pid, libc::EVFILT_PROC, libc::NOTE_EXIT)?;
            add(
                kq.as_raw_fd(),
                interrupt.as_raw_fd() as usize,
                libc::EVFILT_READ,
                0,
            )?;
            Ok(Self {
                kq,
                _held: held,
                dirs: dirs.to_vec(),
            })
        }

        pub(super) fn dirs(&self) -> &[Watched] {
            &self.dirs
        }

        pub(super) fn wait(&mut self, d: Duration) -> io::Result<Woke> {
            let ts = libc::timespec {
                tv_sec: libc::time_t::try_from(d.as_secs()).unwrap_or(libc::time_t::MAX),
                tv_nsec: libc::c_long::from(d.subsec_nanos()),
            };
            // SAFETY: an all-zero `kevent` is a valid value to be overwritten.
            let mut out: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
            // SAFETY: no changes, room for 16 events, a valid timeout.
            let n = unsafe {
                libc::kevent(
                    self.kq.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    out.as_mut_ptr(),
                    out.len() as libc::c_int,
                    &ts,
                )
            };
            if n < 0 {
                let e = io::Error::last_os_error();
                return if e.kind() == io::ErrorKind::Interrupted {
                    Ok(Woke::Time)
                } else {
                    Err(e)
                };
            }
            let got = &out[..usize::try_from(n).unwrap_or(0)];
            if got.iter().any(|e| e.filter == libc::EVFILT_PROC) {
                return Ok(Woke::ProcessGone);
            }
            let vnode = |e: &&libc::kevent| e.filter == libc::EVFILT_VNODE;
            if got.iter().filter(vnode).any(|e| e.fflags & GONE != 0) {
                return Ok(Woke::Rewatch);
            }
            if got.iter().any(|e| vnode(&e)) {
                return Ok(Woke::Files);
            }
            Ok(Woke::Time)
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::{Watched, Woke};
    use std::io;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::time::Duration;

    /// No file events on this platform.
    pub(super) struct Events {
        _never: std::convert::Infallible,
    }

    impl Events {
        pub(super) fn new(
            _dirs: &[Watched],
            _files: &[(PathBuf, &'static str)],
            _parent: libc::pid_t,
            _interrupt: &UnixStream,
        ) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no file events on this platform",
            ))
        }

        pub(super) fn dirs(&self) -> &[Watched] {
            match self._never {}
        }

        pub(super) fn wait(&mut self, _d: Duration) -> io::Result<Woke> {
            match self._never {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Watched, nearest, watched};
    use std::fs;

    #[test]
    fn a_missing_directory_is_watched_through_the_nearest_one_above_it() {
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let deep = root.join("a").join("b");
        assert_eq!(
            nearest(&deep, "ledger.json"),
            Some(Watched {
                dir: root.clone(),
                name: None
            })
        );
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(
            nearest(&deep, "ledger.json"),
            Some(Watched {
                dir: deep,
                name: Some("ledger.json")
            })
        );
    }

    #[test]
    fn two_names_in_one_directory_are_one_watch_for_any_name() {
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let files = vec![(root.clone(), "a.json"), (root.clone(), "b.json")];
        assert_eq!(
            watched(&files),
            vec![Watched {
                dir: root.clone(),
                name: None
            }]
        );
        let one = vec![(root.clone(), "a.json"), (root.clone(), "a.json")];
        assert_eq!(
            watched(&one),
            vec![Watched {
                dir: root,
                name: Some("a.json")
            }]
        );
    }
}
