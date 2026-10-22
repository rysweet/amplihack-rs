//! The loop guard: at most one resume per limit event, one per waiting
//! period of a session, and at most `MAX_RESUMES_PER_SESSION` per session
//! in 24 hours, kept on disk under
//! `<root>/.auto-switch.d/resume/`:
//!
//! ```text
//! lock                  flock taken around every commit
//! events/<sha256 hex>   one empty marker per resumed event, kept 7 days
//! sessions/<sha256 hex> one line per resume: epoch seconds
//! prompted/<sha256 hex> epoch seconds of the session's last user prompt,
//!                       kept MAX_WAIT + 1 hour
//! resume.log            see log.rs
//! ```
//!
//! Directories are created one level at a time with mode 0700. A symlinked,
//! foreign-owned or group/other-accessible path is refused, never chmod-ed
//! or followed. Only hashes and integers are written.

use super::MAX_RESUMES_PER_SESSION;
use amplihack_utils::rotator_status::MAX_WAIT;
use amplihack_utils::secure_files::{
    check_private_dir, ensure_private_dir, fsync_dir, lock_exclusive_timeout,
    open_private_nofollow, owned_by_euid, read_bounded_nofollow, writable_by_others,
    write_private_atomic, write_private_atomic_nosync,
};
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a commit waits for the lock.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an event marker is kept.
const MARKER_RETENTION_SECS: i64 = 7 * DAY_SECS;
/// How long a prompt marker is kept: no waiter is older than `MAX_WAIT`.
pub const PROMPTED_RETENTION_SECS: i64 = MAX_WAIT.as_secs() as i64 + 3600;
const DAY_SECS: i64 = 86_400;
const MAX_SESSION_BYTES: u64 = 4096;
const MAX_PROMPTED_BYTES: u64 = 64;

/// Why a commit did not record a resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitError {
    /// The session already has `MAX_RESUMES_PER_SESSION` resumes in the last
    /// 24 hours, or its record is malformed.
    Cap,
    /// The event already has a marker, or the session was resumed after
    /// this hook's start.
    Duplicate,
    /// The state could not be read or written safely.
    Io,
}

impl From<io::Error> for CommitError {
    fn from(_: io::Error) -> Self {
        CommitError::Io
    }
}

/// Where resumes are recorded. `seen` and `resumes` are read-only checks
/// made before waiting; `commit` repeats both under a lock and records the
/// resume durably, so only a successful commit may wake the session.
pub trait GuardStore {
    /// Whether `event` already has a marker.
    fn seen(&mut self, event: &str) -> io::Result<bool>;
    /// Resumes recorded for `session` in the 24 hours before `now`.
    fn resumes(&mut self, session: &str, now: i64) -> io::Result<u32>;
    /// Creates `resume/` and `resume/prompted/` before a wait, so a prompt
    /// sent during the first-ever wait is recorded, and prunes old prompt
    /// markers.
    fn prepare(&mut self, now: i64) -> io::Result<()>;
    /// When the user last sent a prompt in `session`, if recorded.
    fn prompted_at(&mut self, session: &str) -> io::Result<Option<i64>>;
    /// Records a resume of `event` in `session` at `now`, for a hook that
    /// started at `start`. A session already resumed after `start` was
    /// woken by a newer or concurrent hook and is `Duplicate`.
    fn commit(
        &mut self,
        event: &str,
        session: &str,
        start: i64,
        now: i64,
    ) -> Result<(), CommitError>;
}

/// The on-disk guard for a profile root. Creating one touches nothing.
#[derive(Debug, Clone)]
pub struct FsGuard {
    root: PathBuf,
    lock_timeout: Duration,
}

impl FsGuard {
    pub fn new(root: &Path) -> Self {
        Self::with_lock_timeout(root, LOCK_TIMEOUT)
    }

    pub fn with_lock_timeout(root: &Path, lock_timeout: Duration) -> Self {
        Self {
            root: root.to_path_buf(),
            lock_timeout,
        }
    }

    fn state_dir(&self) -> PathBuf {
        self.root.join(".auto-switch.d")
    }

    /// `<root>/.auto-switch.d/resume`.
    pub fn resume_dir(&self) -> PathBuf {
        self.state_dir().join("resume")
    }

    /// The root must be a real directory of the current user that others
    /// cannot write to.
    fn check_root(&self) -> io::Result<()> {
        let m = fs::symlink_metadata(&self.root)?;
        if m.file_type().is_symlink() || !m.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile root is not a real directory",
            ));
        }
        if !owned_by_euid(&m) || writable_by_others(&m) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile root is not private",
            ));
        }
        Ok(())
    }

    /// Creates `.auto-switch.d` and `resume` when missing, or checks them.
    pub fn ensure_resume_dir(&self) -> io::Result<PathBuf> {
        self.check_root()?;
        ensure_private_dir(&self.state_dir())?;
        let dir = self.resume_dir();
        ensure_private_dir(&dir)?;
        Ok(dir)
    }

    /// `resume/<sub>` (`resume/` itself for an empty `sub`) when it and every
    /// directory above it up to the root exist and are private; `None` when one is missing; an error when one
    /// is unsafe. Creates nothing.
    fn existing_subdir(&self, sub: &str) -> io::Result<Option<PathBuf>> {
        self.check_root()?;
        let mut dirs = vec![self.state_dir(), self.resume_dir()];
        if !sub.is_empty() {
            dirs.push(self.resume_dir().join(sub));
        }
        for d in &dirs {
            match fs::symlink_metadata(d) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e),
                Ok(_) => check_private_dir(d)?,
            }
        }
        Ok(dirs.pop())
    }

    /// The session's resume times; `Ok(None)` when it has no record and
    /// `Ok(Some(None))` when the record is malformed.
    fn read_session(&self, file: &Path) -> io::Result<Option<Option<Vec<i64>>>> {
        let Some((raw, meta)) = read_bounded_nofollow(file, MAX_SESSION_BYTES)? else {
            return Ok(None);
        };
        if !owned_by_euid(&meta) || meta.mode() & 0o077 != 0 || meta.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "session record is not private",
            ));
        }
        Ok(Some(parse_session(&raw)))
    }

    /// Records that the user sent a prompt in `session` at `now`. Writes only
    /// when `resume/` already exists and is private, so nothing is created
    /// for a user who never had a limit resumed; may create `prompted/`.
    pub fn record_prompt(&self, session: &str, now: i64) -> io::Result<()> {
        check_key(session)?;
        let Some(dir) = self.existing_subdir("")? else {
            return Ok(());
        };
        let prompted = dir.join("prompted");
        ensure_private_dir(&prompted)?;
        // Not synced: this runs on every prompt, and a marker lost in a
        // crash costs at most one extra wake.
        write_private_atomic_nosync(&prompted, session, format!("{now}\n").as_bytes())
    }

    /// Deletes entries of `dir` older than `retention` seconds: only regular
    /// files of the current user named like a key. Best effort: one that
    /// cannot be removed now is tried again next time.
    fn prune(&self, dir: &Path, now: i64, retention: i64) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_str().is_some_and(is_key) {
                continue;
            }
            let path = entry.path();
            let Ok(m) = fs::symlink_metadata(&path) else {
                continue;
            };
            if m.is_file() && owned_by_euid(&m) && m.mtime() < now - retention {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

/// One positive epoch-seconds value per line; anything else is malformed.
fn parse_session(raw: &[u8]) -> Option<Vec<i64>> {
    let text = std::str::from_utf8(raw).ok()?;
    let body = text.strip_suffix('\n').unwrap_or(text);
    if body.is_empty() {
        return Some(Vec::new());
    }
    body.split('\n')
        .map(|line| {
            if line.is_empty() || !line.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            line.parse::<i64>().ok().filter(|t| *t > 0)
        })
        .collect()
}

fn recent(times: &[i64], now: i64) -> Vec<i64> {
    times
        .iter()
        .copied()
        .filter(|t| now - *t < DAY_SECS)
        .collect()
}

/// Keys are 64 lowercase hex digits, so they are safe file names.
fn is_key(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn check_key(s: &str) -> io::Result<()> {
    if is_key(s) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "key is not 64 lowercase hex digits",
        ))
    }
}

impl GuardStore for FsGuard {
    fn seen(&mut self, event: &str) -> io::Result<bool> {
        check_key(event)?;
        let Some(events) = self.existing_subdir("events")? else {
            return Ok(false);
        };
        match fs::symlink_metadata(events.join(event)) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn resumes(&mut self, session: &str, now: i64) -> io::Result<u32> {
        check_key(session)?;
        let Some(sessions) = self.existing_subdir("sessions")? else {
            return Ok(0);
        };
        Ok(match self.read_session(&sessions.join(session))? {
            None => 0,
            Some(None) => MAX_RESUMES_PER_SESSION,
            Some(Some(times)) => recent(&times, now).len() as u32,
        })
    }

    fn prepare(&mut self, now: i64) -> io::Result<()> {
        let prompted = self.ensure_resume_dir()?.join("prompted");
        ensure_private_dir(&prompted)?;
        self.prune(&prompted, now, PROMPTED_RETENTION_SECS);
        Ok(())
    }

    fn prompted_at(&mut self, session: &str) -> io::Result<Option<i64>> {
        check_key(session)?;
        let Some(prompted) = self.existing_subdir("prompted")? else {
            return Ok(None);
        };
        let Some((raw, meta)) = read_bounded_nofollow(&prompted.join(session), MAX_PROMPTED_BYTES)?
        else {
            return Ok(None);
        };
        if !owned_by_euid(&meta) || meta.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "prompt marker is not private",
            ));
        }
        // The marker is written without fsync, so a crash can leave it
        // empty; that is a lost prompt, not a malformed one.
        match parse_session(&raw).as_deref() {
            Some([t]) => Ok(Some(*t)),
            Some([]) => Ok(None),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "prompt marker is malformed",
            )),
        }
    }

    fn commit(
        &mut self,
        event: &str,
        session: &str,
        start: i64,
        now: i64,
    ) -> Result<(), CommitError> {
        check_key(event)?;
        check_key(session)?;
        let dir = self.ensure_resume_dir()?;
        let (events, sessions) = (dir.join("events"), dir.join("sessions"));
        ensure_private_dir(&events)?;
        ensure_private_dir(&sessions)?;
        let _lock =
            lock_exclusive_timeout(open_private_nofollow(&dir.join("lock"))?, self.lock_timeout)?;

        self.prune(&events, now, MARKER_RETENTION_SECS);
        self.prune(&dir.join("prompted"), now, PROMPTED_RETENTION_SECS);
        let marker = events.join(event);
        match fs::symlink_metadata(&marker) {
            Ok(_) => return Err(CommitError::Duplicate),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let times = match self.read_session(&sessions.join(session))? {
            None => Vec::new(),
            Some(None) => return Err(CommitError::Cap),
            Some(Some(times)) if times.iter().any(|t| *t > start) => {
                return Err(CommitError::Duplicate);
            }
            Some(Some(times)) => recent(&times, now),
        };
        if times.len() as u32 >= MAX_RESUMES_PER_SESSION {
            return Err(CommitError::Cap);
        }

        // `create_new` (O_CREAT|O_EXCL) never follows a symlink: a planted
        // one makes the open fail with AlreadyExists.
        let created = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&marker);
        match created {
            Ok(f) => f.sync_all()?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(CommitError::Duplicate);
            }
            Err(e) => return Err(e.into()),
        }
        fsync_dir(&events)?;

        let mut record = String::new();
        for t in times.iter().chain(std::iter::once(&now)) {
            record.push_str(&format!("{t}\n"));
        }
        write_private_atomic(&sessions, session, record.as_bytes())?;
        Ok(())
    }
}
