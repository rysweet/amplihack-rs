//! Issue #2029: the kqueue backend (macOS and the BSDs).
//!
//! Directory subscriptions, plus existing login/control file subscriptions
//! for in-place changes. Directories use `O_EVTONLY | O_CLOEXEC |
//! O_NOFOLLOW | O_DIRECTORY` and checked for ownership after the open, so a
//! symlink is never followed and the check-then-open window is closed. The
//! stop pipe is registered on the same queue, so one `kevent` call waits on
//! the directories and the signal together.

use super::{Event, Events, WakePipe, stop_last};
use crate::commands::profile::ProfileError;
use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

/// The most events one `kevent` call collects. A batch larger than this is
/// read by the next call; nothing is lost.
const BATCH: usize = 64;

/// What a directory watch reports: anything that changes the directory or
/// its entries.
const VNODE_FLAGS: u32 = libc::NOTE_WRITE
    | libc::NOTE_DELETE
    | libc::NOTE_RENAME
    | libc::NOTE_EXTEND
    | libc::NOTE_ATTRIB
    | libc::NOTE_LINK
    | libc::NOTE_REVOKE;

use super::{DirFail, Skipped, dir_err as watch_err};

pub(super) struct Backend {
    kq: OwnedFd,
    dirs: BTreeMap<PathBuf, OwnedFd>,
    files: BTreeMap<PathBuf, OwnedFd>,
    wake: WakePipe,
    skipped: Skipped,
}

impl Backend {
    pub(super) fn open(dirs: &[PathBuf]) -> Result<Self, ProfileError> {
        // SAFETY: kqueue(2) takes no arguments and returns a descriptor.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return Err(ProfileError::RotateDaemon(format!(
                "watch: the event queue could not be opened ({})",
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: the descriptor was just created and is not owned elsewhere.
        let kq = unsafe { OwnedFd::from_raw_fd(kq) };
        // SAFETY: setting close-on-exec on a descriptor this function owns.
        if unsafe { libc::fcntl(kq.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(ProfileError::RotateDaemon(format!(
                "watch: the event queue could not be made close-on-exec ({})",
                std::io::Error::last_os_error()
            )));
        }
        let wake = WakePipe::open()?;
        let mut backend = Self {
            kq,
            dirs: BTreeMap::new(),
            files: BTreeMap::new(),
            wake,
            skipped: Skipped::default(),
        };
        backend.register_wake()?;
        backend.watch(dirs)?;
        Ok(backend)
    }

    fn register_wake(&self) -> Result<(), ProfileError> {
        let ev = kevent(
            self.wake.fd() as libc::uintptr_t,
            libc::EVFILT_READ,
            libc::EV_ADD | libc::EV_CLEAR,
            0,
        );
        self.change(&[ev]).map_err(|e| {
            ProfileError::RotateDaemon(format!("watch: the stop wake was not registered ({e})"))
        })
    }

    /// Applies `changes` to the queue, taking no events back.
    fn change(&self, changes: &[libc::kevent]) -> Result<(), std::io::Error> {
        // SAFETY: `changes` is a valid slice and no event list is taken.
        let n = unsafe {
            libc::kevent(
                self.kq.as_raw_fd(),
                changes.as_ptr(),
                changes.len() as libc::c_int,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Opens a node this user owns without following a symlink. File
    /// subscriptions use O_EVTONLY and never read credential contents.
    fn open_node(path: &Path, directory: bool) -> Result<OwnedFd, DirFail> {
        use std::os::unix::ffi::OsStrExt;
        let raw = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| DirFail::fault(path, "the path holds a NUL byte"))?;
        #[cfg(target_vendor = "apple")]
        let base = libc::O_EVTONLY;
        #[cfg(not(target_vendor = "apple"))]
        let base = libc::O_RDONLY;
        let flags = base
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if directory {
                libc::O_DIRECTORY
            } else {
                libc::O_NONBLOCK
            };
        // SAFETY: `raw` is a NUL-terminated path for the life of the call.
        let fd = unsafe { libc::open(raw.as_ptr(), flags) };
        if fd < 0 {
            return Err(DirFail::io(path, &std::io::Error::last_os_error()));
        }
        // SAFETY: the descriptor was just opened and is not owned elsewhere.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // The ownership check is on the descriptor, not the path, so a
        // directory replaced between the check and the open is not watched.
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
        let system_parent = directory
            && st.st_uid == 0
            && (st.st_mode & 0o022 == 0 || st.st_mode & libc::S_ISVTX != 0);
        if st.st_uid != user && !system_parent {
            return Err(DirFail::fault(
                path,
                "not a user-owned or protected system directory",
            ));
        }
        if !directory && st.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(DirFail::fault(path, "not a regular control or login file"));
        }
        Ok(fd)
    }

    fn watch_files(&mut self, dirs: &[PathBuf]) -> Result<(), ProfileError> {
        // kqueue directory vnodes report entry changes, not writes to an
        // existing child. Subscribe to mutable login/control files as well.
        // O_EVTONLY never reads credential contents.
        let wanted: std::collections::BTreeSet<_> = dirs
            .iter()
            .flat_map(|dir| [".credentials.json", ".auto-switch", "pin"].map(|name| dir.join(name)))
            .collect();
        self.files.retain(|p, _| wanted.contains(p));
        for path in wanted {
            if let Some(fd) = self.files.get(&path) {
                use std::os::unix::fs::MetadataExt;
                // SAFETY: stat is plain data; fstat initialises it below.
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                // SAFETY: fd is live and st is writable plain data.
                if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } == 0
                    && std::fs::symlink_metadata(&path).is_ok_and(|m| {
                        m.is_file() && m.dev() == st.st_dev as u64 && m.ino() == st.st_ino
                    })
                {
                    continue;
                }
            }
            self.files.remove(&path);
            let fd = match Self::open_node(&path, false) {
                Ok(fd) => fd,
                Err(f) if f.skip => continue,
                Err(f) => return Err(f.err),
            };
            let ev = kevent(
                fd.as_raw_fd() as libc::uintptr_t,
                libc::EVFILT_VNODE,
                libc::EV_ADD | libc::EV_CLEAR,
                VNODE_FLAGS,
            );
            self.change(&[ev]).map_err(|e| watch_err(&path, e))?;
            self.files.insert(path, fd);
        }
        Ok(())
    }
}

/// One change or event record.
fn kevent(ident: libc::uintptr_t, filter: i16, flags: u16, fflags: u32) -> libc::kevent {
    libc::kevent {
        ident,
        filter,
        flags,
        fflags,
        data: 0,
        udata: std::ptr::null_mut(),
    }
}

impl Backend {
    /// One readiness call. Non-blocking drains may return no events.
    fn collect(&mut self, blocking: bool) -> Result<Option<Vec<Event>>, ProfileError> {
        let ts = (!blocking).then_some(libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        });
        // SAFETY: `kevent` is plain data, so an all-zero array is valid;
        // kevent(2) fills the records it reports.
        let mut list: [libc::kevent; BATCH] = unsafe { std::mem::zeroed() };
        // SAFETY: `list` is a live array of `BATCH` records, and `ts` lives
        // for the call.
        let n = unsafe {
            libc::kevent(
                self.kq.as_raw_fd(),
                std::ptr::null(),
                0,
                list.as_mut_ptr(),
                BATCH as libc::c_int,
                ts.as_ref().map_or(std::ptr::null(), |t| t as *const _),
            )
        };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                return Ok(Some(Vec::new()));
            }
            return Err(ProfileError::RotateDaemon(format!(
                "watch: the event queue failed ({e})"
            )));
        }
        if n == 0 {
            return Ok(None);
        }
        let mut out = Vec::new();
        let mut changed = false;
        for ev in &list[..n as usize] {
            if ev.filter == libc::EVFILT_READ && ev.ident == self.wake.fd() as libc::uintptr_t {
                out.extend(self.wake.take()?);
            } else {
                if ev.flags & libc::EV_ERROR != 0 {
                    return Err(ProfileError::RotateDaemon(format!(
                        "watch: native subscription failed ({})",
                        std::io::Error::from_raw_os_error(ev.data as i32)
                    )));
                }
                if ev.fflags & (libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE) != 0 {
                    self.dirs
                        .retain(|_, fd| fd.as_raw_fd() as libc::uintptr_t != ev.ident);
                    self.files
                        .retain(|_, fd| fd.as_raw_fd() as libc::uintptr_t != ev.ident);
                    if !out.contains(&Event::Overflow) {
                        out.push(Event::Overflow);
                    }
                }
                changed = true;
            }
        }
        if changed {
            out.insert(0, Event::Changed);
        }
        Ok(Some(stop_last(out)))
    }
}

impl Events for Backend {
    fn watch(&mut self, dirs: &[PathBuf]) -> Result<(), ProfileError> {
        let wanted: std::collections::BTreeSet<&PathBuf> = dirs.iter().collect();
        // Dropping the descriptor removes its registration, so a directory
        // no longer named needs nothing else.
        self.dirs.retain(|p, _| wanted.contains(p));
        for dir in dirs {
            if let Some(fd) = self.dirs.get(dir) {
                use std::os::unix::fs::MetadataExt;
                // Compare identity only: never read credentials for change detection.
                // SAFETY: stat is plain data; fstat fills it through a live fd.
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                let same = unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } == 0
                    && std::fs::symlink_metadata(dir).is_ok_and(|m| {
                        m.is_dir() && m.dev() == st.st_dev as u64 && m.ino() == st.st_ino
                    });
                if same {
                    continue;
                }
            }
            let fd = match Self::open_node(dir, true) {
                Ok(fd) => fd,
                // Not there, or a symlink at the path: reported and left
                // out, never a reason to end the rotator.
                Err(f) if f.skip => {
                    self.dirs.remove(dir);
                    self.skipped.note(dir, &f.err);
                    continue;
                }
                Err(f) => return Err(f.err),
            };
            let ev = kevent(
                fd.as_raw_fd() as libc::uintptr_t,
                libc::EVFILT_VNODE,
                libc::EV_ADD | libc::EV_CLEAR,
                VNODE_FLAGS,
            );
            self.change(&[ev]).map_err(|e| watch_err(dir, e))?;
            self.dirs.insert(dir.clone(), fd);
            self.skipped.clear(dir);
        }
        self.watch_files(dirs)
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
        std::iter::once(self.kq.as_raw_fd())
            .chain(self.dirs.values().map(AsRawFd::as_raw_fd))
            .chain(self.files.values().map(AsRawFd::as_raw_fd))
            .chain(self.wake.raw_fds())
            .collect()
    }

    fn wait(&mut self) -> Result<Vec<Event>, ProfileError> {
        loop {
            match self.collect(true)? {
                Some(batch) if !batch.is_empty() => return Ok(batch),
                // Interrupted, or woken with nothing to report: block again.
                Some(_) => {}
                None => {}
            }
        }
    }

    fn drain(&mut self) -> Result<Vec<Event>, ProfileError> {
        Ok(self.collect(false)?.unwrap_or_default())
    }
}
