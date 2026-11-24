//! The guidance wake (issue #3129, D4 and D-11).
//!
//! A process that waits for guidance arms a [`Waiter`]: a FIFO in
//! `guidance-wake/` ([`WAKE_DIR`]), a 0700 folder beside a `guidance/`
//! folder. After [`super::append_entry`] publishes and logs an entry it
//! calls [`notify`], which writes one byte to every armed FIFO there.
//!
//! A byte means "read the entries again", never "there is news": the waiter
//! decides from what it reads. Bytes merge (a FIFO that already holds unread
//! bytes may refuse more, which is fine) and may be spurious.
//!
//! Naming. A waiter makes its FIFO under `.tmp-<pid>-<n>`, a name [`notify`]
//! never touches, opens the read end and a write end of its own, and only
//! then renames it to `<pid>-<n>.fifo`. A FIFO under a final name therefore
//! always has a reader, so a final name whose open for writing fails with
//! `ENXIO` (no reader) belongs to a waiter that died without removing it,
//! and [`notify`] removes it. Because the waiter holds its own write end, its
//! read end never reports end of file: `poll` sees it readable only while a
//! byte is waiting.
//!
//! [`notify`] opens each final name with `O_NOFOLLOW|O_NONBLOCK`, then checks
//! the open descriptor (a FIFO owned by this user) before it writes. It never
//! checks a path and opens it afterwards.
//!
//! Every descriptor here is opened with `O_CLOEXEC` (the standard library
//! adds it to every open), so a command the waiter starts holds none of them.
//! Nothing here sleeps, polls or sets a timer.

use super::GuideError;
use super::fs_safety::{self, DirState};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The folder of wake FIFOs, beside `guidance/`.
pub(crate) const WAKE_DIR: &str = "guidance-wake";

/// How many names [`Waiter::arm`] tries. A search over distinct names, not a
/// retry in time: a name is taken only by a FIFO a dead process with this
/// PID left behind.
const MAX_NAME_TRIES: u64 = 1000;

/// The `<n>` of the next FIFO name in this process.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Whether `name` is the final name of a waiter's FIFO, `^\d+-\d+\.fifo$`.
fn is_fifo_name(name: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    name.strip_suffix(".fifo")
        .and_then(|stem| stem.split_once('-'))
        .is_some_and(|(pid, n)| digits(pid) && digits(n))
}

/// `mkfifo(path, 0600)`.
fn mkfifo(path: &Path) -> io::Result<()> {
    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "the path holds a NUL byte"))?;
    // SAFETY: `c` is a valid NUL-terminated string that outlives the call;
    // mkfifo only reads it.
    if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Opens `path` without following a link, without blocking and without
/// making it the controlling terminal. Never creates anything.
fn open_nofollow(path: &Path, write: bool) -> io::Result<File> {
    OpenOptions::new()
        .read(!write)
        .write(write)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_NOCTTY)
        .open(path)
}

/// Why the open descriptor `file` is not a FIFO owned by this user, if so.
fn not_our_fifo(file: &File) -> io::Result<Option<String>> {
    let meta = file.metadata()?;
    let me = fs_safety::euid();
    Ok(if !meta.file_type().is_fifo() {
        Some("it is not a FIFO".to_string())
    } else if meta.uid() != me {
        Some(format!("it is owned by uid {}, not uid {me}", meta.uid()))
    } else {
        None
    })
}

/// One armed wake FIFO. Dropping it removes the FIFO, then closes it.
#[derive(Debug)]
pub(crate) struct Waiter {
    path: PathBuf,
    read: File,
    /// Never written: held so the read end never reports end of file.
    _write: File,
}

impl Waiter {
    /// Arms a FIFO in `<parent>/guidance-wake/`, creating that folder (0700)
    /// when it is missing.
    ///
    /// # Errors
    ///
    /// [`GuideError::NotFound`] when `parent` is missing,
    /// [`GuideError::Unsafe`] when it or the wake folder fails the trust
    /// rule, [`GuideError::Io`] otherwise. Nothing is left behind on error.
    pub(crate) fn arm(parent: &Path) -> Result<Self, GuideError> {
        if !fs_safety::check_dir(parent)? {
            return Err(GuideError::NotFound(format!(
                "{} does not exist",
                parent.display()
            )));
        }
        let dir = parent.join(WAKE_DIR);
        fs_safety::ensure_private_dir(&dir)?;
        let pid = std::process::id();
        for _ in 0..MAX_NAME_TRIES {
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let tmp = dir.join(format!(".tmp-{pid}-{n}"));
            match mkfifo(&tmp) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(GuideError::Io(format!(
                        "cannot make a wake FIFO in {}: {e}",
                        dir.display()
                    )));
                }
            }
            let path = dir.join(format!("{pid}-{n}.fifo"));
            return Self::open_and_publish(&tmp, path).map_err(|e| {
                let _ = fs::remove_file(&tmp);
                GuideError::Io(format!("cannot arm a wake FIFO in {}: {e}", dir.display()))
            });
        }
        Err(GuideError::Io(format!(
            "cannot find a free wake FIFO name in {}",
            dir.display()
        )))
    }

    /// Opens both ends of the FIFO at `tmp`, checks it, then renames it to
    /// `path`, so the final name never lacks a reader.
    fn open_and_publish(tmp: &Path, path: PathBuf) -> io::Result<Self> {
        let read = open_nofollow(tmp, false)?;
        if let Some(reason) = not_our_fifo(&read)? {
            return Err(io::Error::other(reason));
        }
        let write = open_nofollow(tmp, true)?;
        fs::rename(tmp, &path)?;
        Ok(Self {
            path,
            read,
            _write: write,
        })
    }

    /// The FIFO's final path. Only the tests need it: nothing outside this
    /// module opens or removes a waiter's FIFO by name.
    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The read end, for a poller.
    pub(crate) fn as_raw_fd(&self) -> RawFd {
        self.read.as_raw_fd()
    }

    /// Reads every waiting wake byte without blocking and returns how many
    /// there were (0 when none).
    pub(crate) fn take(&mut self) -> usize {
        let mut buf = [0_u8; 64];
        let mut total = 0;
        loop {
            match self.read.read(&mut buf) {
                // End of file cannot happen while `_write` is open.
                Ok(0) => return total,
                Ok(n) => total += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                // `WouldBlock`: drained. Any other error leaves nothing to read.
                Err(_) => return total,
            }
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        // Before the descriptors close, so the name never lacks a reader. A
        // FIFO already gone (a recursive removal of the run folder) is fine.
        let _ = fs::remove_file(&self.path);
    }
}

/// Wakes every waiter armed in `<parent>/guidance-wake/`.
///
/// Never blocks, never fails and creates nothing. When the wake folder is
/// missing there is no waiter and nothing happens. A FIFO with no reader
/// (`ENXIO`) is a dead waiter's and is removed. Only names matching
/// `^\d+-\d+\.fifo$` are opened or removed; `.tmp-*` and every other name
/// are left alone.
///
/// Returns one warning line when something unexpected happened: the wake
/// folder fails the trust rule, a name is a link, not a FIFO or not this
/// user's, or an I/O error. A FIFO that is full already has a byte waiting
/// and is not a problem.
pub(crate) fn notify(parent: &Path) -> Option<String> {
    let dir = parent.join(WAKE_DIR);
    match fs_safety::dir_state(&dir) {
        Ok(DirState::Trusted) => {}
        Ok(DirState::Missing) => return None,
        Ok(DirState::Untrusted(reason)) => {
            return Some(format!(
                "guidance waiters in {} were not woken: {reason}",
                dir.display()
            ));
        }
        Err(e) => return Some(format!("guidance waiters were not woken: {e}")),
    }
    let listing = match fs::read_dir(&dir) {
        Ok(listing) => listing,
        Err(e) => {
            return Some(format!(
                "guidance waiters in {} were not woken: cannot list it: {e}",
                dir.display()
            ));
        }
    };
    let mut problems: Vec<String> = Vec::new();
    for item in listing {
        let name = match item {
            Ok(item) => item.file_name(),
            Err(e) => {
                problems.push(format!("cannot list {}: {e}", dir.display()));
                break;
            }
        };
        let Some(name) = name.to_str().filter(|n| is_fifo_name(n)) else {
            continue;
        };
        if let Err(problem) = wake_one(&dir.join(name)) {
            problems.push(problem);
        }
    }
    (!problems.is_empty()).then(|| {
        format!(
            "a guidance waiter may not have been woken: {}",
            problems.join("; ")
        )
    })
}

/// Writes one byte to the FIFO at `path`, or removes it when it has no
/// reader. `Err` holds what went wrong, naming the path.
fn wake_one(path: &Path) -> Result<(), String> {
    let mut fifo = match open_nofollow(path, true) {
        Ok(fifo) => fifo,
        Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
            return match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("cannot remove the stale {}: {e}", path.display())),
            };
        }
        // Its waiter finished between the listing and the open.
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(format!(
                "left {} alone: it is a symbolic link",
                path.display()
            ));
        }
        Err(e) => return Err(format!("cannot open {}: {e}", path.display())),
    };
    match not_our_fifo(&fifo) {
        Ok(None) => {}
        Ok(Some(reason)) => return Err(format!("left {} alone: {reason}", path.display())),
        Err(e) => return Err(format!("cannot check {}: {e}", path.display())),
    }
    match fifo.write(&[1]) {
        Ok(_) => Ok(()),
        // Full: the waiter has unread bytes and will read again anyway.
        Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(()),
        // The waiter closed between the open and the write; the Rust runtime
        // ignores SIGPIPE, so this is an error value, not a signal.
        Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(format!("cannot write to {}: {e}", path.display())),
    }
}
