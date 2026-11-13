//! Issue #2029: the inotify backend (Linux).
//!
//! One inotify watch per directory, added with `IN_ONLYDIR |
//! IN_DONT_FOLLOW` so a symlink at the path is never followed, after an
//! `O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC` open that checks the directory is
//! this user's. The inotify descriptor and the stop pipe are polled
//! together, so a blocked rotator still sees SIGTERM.
//!
//! `IN_Q_OVERFLOW` is reported as [`Event::Overflow`] rather than folded
//! into a change: the reader has missed events, so the next pass reads
//! every state file again.

use super::{DirFail, Event, Events, Skipped, WakePipe, stop_last};
use crate::commands::profile::ProfileError;
use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "tests_issue_2029_inotify.rs"]
mod issue_2029_inotify;

/// Watch data mutations, not writable closes: merely releasing the shared
/// state lock closes an O_RDWR descriptor and must never wake another read.
const MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_MODIFY
    | libc::IN_ATTRIB
    | libc::IN_ONLYDIR
    | libc::IN_DONT_FOLLOW;

/// One read of the inotify descriptor. Large enough for a burst of renames
/// under one directory; anything beyond it is read by the next call.
const BUF: usize = 16 * 1024;

pub(super) struct Backend {
    fd: OwnedFd,
    dirs: BTreeMap<PathBuf, (libc::c_int, u64, u64)>,
    wake: WakePipe,
    skipped: Skipped,
}

impl Backend {
    pub(super) fn open(dirs: &[PathBuf]) -> Result<Self, ProfileError> {
        // SAFETY: inotify_init1(2) takes flags and returns a descriptor.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(ProfileError::RotateDaemon(format!(
                "watch: inotify could not be opened ({})",
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: the descriptor was just created and is not owned elsewhere.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let wake = WakePipe::open()?;
        let mut backend = Self {
            fd,
            dirs: BTreeMap::new(),
            wake,
            skipped: Skipped::default(),
        };
        backend.watch(dirs)?;
        Ok(backend)
    }

    /// Checks `path` is a directory this user owns, without following a
    /// symlink at it. The descriptor is closed again: inotify takes a path.
    fn check_dir(path: &Path) -> Result<OwnedFd, DirFail> {
        use std::os::unix::ffi::OsStrExt;
        let raw = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| DirFail::fault(path, "the path holds a NUL byte"))?;
        // SAFETY: `raw` is a NUL-terminated path for the life of the call.
        let fd = unsafe {
            libc::open(
                raw.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        };
        if fd < 0 {
            return Err(DirFail::io(path, &std::io::Error::last_os_error()));
        }
        // SAFETY: the descriptor was just opened and is not owned elsewhere.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: `stat` is plain data, so an all-zero value is valid;
        // fstat(2) overwrites it before anything reads it.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `st` is a live `stat` and `fd` is open.
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
            return Err(DirFail::io(path, &std::io::Error::last_os_error()));
        }
        // SAFETY: geteuid(2) cannot fail and has no preconditions.
        let user = unsafe { libc::geteuid() };
        // Root-owned parents such as /srv and sticky /tmp must remain
        // subscribable across replacement of a user's configured root.
        // Observation readers independently enforce ownership of their state.
        let system_parent =
            st.st_uid == 0 && (st.st_mode & 0o022 == 0 || st.st_mode & libc::S_ISVTX != 0);
        if st.st_uid != user && !system_parent {
            return Err(DirFail::fault(
                path,
                "not a user-owned or protected system directory",
            ));
        }
        Ok(fd)
    }

    fn add(&mut self, dir: &Path) -> Result<(), DirFail> {
        let directory = Self::check_dir(dir)?;
        let raw = std::ffi::CString::new(format!("/proc/self/fd/{}", directory.as_raw_fd()))
            .map_err(|_| DirFail::fault(dir, "the path holds a NUL byte"))?;
        // SAFETY: `raw` is a NUL-terminated path for the life of the call.
        let wd = unsafe {
            libc::inotify_add_watch(
                self.fd.as_raw_fd(),
                raw.as_ptr(),
                MASK & !libc::IN_DONT_FOLLOW,
            )
        };
        if wd < 0 {
            let e = std::io::Error::last_os_error();
            // A silent partial watch set presents as a rotator that runs
            // and never rotates, so the limit is named rather than hidden.
            if e.raw_os_error() == Some(libc::ENOSPC) {
                return Err(DirFail::fault(
                    dir,
                    "the inotify watch limit was reached; raise fs.inotify.max_user_watches",
                ));
            }
            return Err(DirFail::io(dir, &e));
        }
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::File::from(directory)
            .metadata()
            .map_err(|e| DirFail::io(dir, &e))?;
        self.dirs.insert(dir.to_path_buf(), (wd, m.dev(), m.ino()));
        Ok(())
    }

    /// Reads whatever inotify has ready. `Ok(None)` when it has nothing.
    fn read_events(&mut self) -> Result<Option<Vec<Event>>, ProfileError> {
        let mut buf = vec![0_u8; BUF];
        // SAFETY: reading into a live buffer from a descriptor we own.
        let n = unsafe {
            libc::read(
                self.fd.as_raw_fd(),
                buf.as_mut_ptr().cast::<libc::c_void>(),
                buf.len(),
            )
        };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ) {
                return Ok(None);
            }
            return Err(ProfileError::RotateDaemon(format!(
                "watch: inotify could not be read ({e})"
            )));
        }
        if n == 0 {
            return Ok(None);
        }
        let header = std::mem::size_of::<libc::inotify_event>();
        let mut at = 0_usize;
        let (mut changed, mut overflow) = (false, false);
        while at + header <= n as usize {
            // SAFETY: `at` is within the bytes read and leaves room for a
            // whole header, and inotify writes whole records.
            let ev = unsafe {
                buf.as_ptr()
                    .add(at)
                    .cast::<libc::inotify_event>()
                    .read_unaligned()
            };
            at += header + ev.len as usize;
            if ev.mask & libc::IN_IGNORED != 0 {
                // Removing subscriptions during repair queues IN_IGNORED.
                // Unknown old descriptors must not trigger another repair.
                if !self.dirs.values().any(|(wd, _, _)| *wd == ev.wd) {
                    continue;
                }
                self.dirs.retain(|_, (wd, _, _)| *wd != ev.wd);
            }
            if ev.mask
                & (libc::IN_Q_OVERFLOW
                    | libc::IN_IGNORED
                    | libc::IN_DELETE_SELF
                    | libc::IN_MOVE_SELF)
                != 0
            {
                // Invalidations have the same repair contract as overflow:
                // resynchronise, re-arm, then reread across registration.
                overflow = true;
            }
            if ev.mask & libc::IN_Q_OVERFLOW == 0 {
                changed = true;
            }
        }
        if overflow {
            for (wd, _, _) in self.dirs.values() {
                self.remove_watch(*wd)?;
            }
            self.dirs.clear();
        }
        let mut out = Vec::new();
        if overflow {
            out.push(Event::Overflow);
        }
        if changed {
            out.push(Event::Changed);
        }
        Ok((!out.is_empty()).then_some(out))
    }

    fn remove_watch(&self, wd: libc::c_int) -> Result<(), ProfileError> {
        // SAFETY: wd was registered on this inotify descriptor.
        if unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), wd) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINVAL) {
                return Err(ProfileError::RotateDaemon(format!(
                    "watch: subscription repair failed ({error})"
                )));
            }
        }
        Ok(())
    }

    /// Polls the inotify descriptor and the stop pipe together.
    /// Non-blocking drains use a zero timeout; waits have no deadline.
    fn poll(&self, blocking: bool) -> Result<bool, ProfileError> {
        let mut fds = [
            libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.wake.fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let timeout = if blocking { -1 } else { 0 };
        // SAFETY: `fds` is a live array of two descriptors we own.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                return Ok(true);
            }
            return Err(ProfileError::RotateDaemon(format!(
                "watch: the event source could not be polled ({e})"
            )));
        }
        if fds
            .iter()
            .any(|fd| fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0)
        {
            return Err(ProfileError::RotateDaemon(
                "watch: native event source closed or failed".into(),
            ));
        }
        Ok(n > 0)
    }

    /// Everything ready now, with `Stop` last.
    fn collect(&mut self, blocking: bool) -> Result<Vec<Event>, ProfileError> {
        if !self.poll(blocking)? {
            return Ok(Vec::new());
        }
        let mut out = self.read_events()?.unwrap_or_default();
        out.extend(self.wake.take()?);
        Ok(stop_last(out))
    }
}

impl Events for Backend {
    fn watch(&mut self, dirs: &[PathBuf]) -> Result<(), ProfileError> {
        let wanted: std::collections::BTreeSet<&PathBuf> = dirs.iter().collect();
        let gone: Vec<PathBuf> = self
            .dirs
            .keys()
            .filter(|p| !wanted.contains(p))
            .cloned()
            .collect();
        for dir in gone {
            if let Some((wd, _, _)) = self.dirs.remove(&dir) {
                self.remove_watch(wd)?;
            }
        }
        for dir in dirs {
            use std::os::unix::fs::MetadataExt;
            if let Some((_, dev, ino)) = self.dirs.get(dir)
                && std::fs::symlink_metadata(dir)
                    .is_ok_and(|m| m.is_dir() && m.dev() == *dev && m.ino() == *ino)
            {
                continue;
            }
            if let Some((wd, _, _)) = self.dirs.remove(dir) {
                self.remove_watch(wd)?;
            }
            let added = self.add(dir);
            match added {
                Ok(()) => self.skipped.clear(dir),
                // Not there, or a symlink at the path: reported and left
                // out, never a reason to end the rotator.
                Err(f) if f.skip => self.skipped.note(dir, &f.err),
                Err(f) => return Err(f.err),
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn watched(&self) -> Vec<PathBuf> {
        self.dirs.keys().cloned().collect()
    }

    fn wake_handle(&self) -> super::WakeHandle {
        self.wake.handle()
    }
    #[cfg(test)]
    fn raw_fds_for_test(&self) -> Vec<std::os::fd::RawFd> {
        std::iter::once(self.fd.as_raw_fd())
            .chain(self.wake.raw_fds())
            .collect()
    }

    fn wait(&mut self) -> Result<Vec<Event>, ProfileError> {
        loop {
            let batch = self.collect(true)?;
            if !batch.is_empty() {
                return Ok(batch);
            }
        }
    }

    fn drain(&mut self) -> Result<Vec<Event>, ProfileError> {
        self.collect(false)
    }
}
