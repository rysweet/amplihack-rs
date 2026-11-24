//! The loop guard: at most one resume per limit event and one per waiting
//! period of a session, kept on disk under
//! `<root>/.auto-switch.d/resume/`. There is no cap on how often a session
//! is resumed (issue #2912): a resume follows a login with room, so the
//! count is evidence for the log and never a refusal.
//!
//! Only a resume that reached the session is recorded (issue #3104): the
//! hook commits just before it types into a proved pane, and releases its
//! own commit if a key then fails. An exit-2 wake records nothing, so the
//! event stays armed for the next room event.
//!
//! ```text
//! lock                  flock taken around every commit and release
//! events/<sha256 hex>   one empty marker per event resumed with keys, kept 7 days
//! sessions/<sha256 hex> one line per resume with keys: epoch seconds
//! prompted/<sha256 hex> epoch seconds of the session's last user prompt,
//!                       kept MAX_WAIT + 1 hour
//! resume.log            see log.rs
//! ```
//!
//! Directories are created one level at a time with mode 0700. A symlinked,
//! foreign-owned or group/other-accessible path is refused, never chmod-ed
//! or followed. Only hashes and integers are written.

use amplihack_utils::clock::{Clock, SystemClock};
use amplihack_utils::rotator_status::MAX_WAIT;
use amplihack_utils::secure_files::{
    check_private_dir, ensure_private_dir, fsync_dir, lock_exclusive_timeout_with,
    open_private_nofollow, owned_by_euid, read_bounded_nofollow, writable_by_others,
    write_private_atomic, write_private_atomic_nosync,
};
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

/// Why a commit did not record a resume. There is no count among these:
/// the per-event marker and the since-start rule are the only duplicate
/// guards (issue #2912).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitError {
    /// The event already has a marker, or the session was resumed after
    /// this hook's start.
    Duplicate,
    /// The state could not be read or written safely.
    Io,
}

/// What [`GuardStore::resumes`] found in a session's record.
///
/// Neither field refuses a resume. `count` is evidence for the log line
/// (`resumes=<n>`), and `malformed` says the record could not be parsed, so
/// it is read as no earlier resumes and the next commit rewrites it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Resumes {
    /// Resumes recorded in the 24 hours before `now`.
    pub count: u32,
    /// The record could not be parsed.
    pub malformed: bool,
}

impl From<io::Error> for CommitError {
    fn from(_: io::Error) -> Self {
        CommitError::Io
    }
}

/// Where resumes are recorded. `seen` and `resumes` are read-only checks
/// made before waiting; `commit` repeats both under a lock and records the
/// resume durably, so only a successful commit may type into the session.
/// A marker means the event was resumed with keys; `release` takes back a
/// commit whose keys did not reach the session.
pub trait GuardStore {
    /// Whether `event` already has a marker.
    fn seen(&mut self, event: &str) -> io::Result<bool>;
    /// Resumes recorded for `session` in the 24 hours before `now`, and
    /// whether its record could be parsed. Read for the log only.
    fn resumes(&mut self, session: &str, now: i64) -> io::Result<Resumes>;
    /// The latest resume recorded for `session`, at any age; `None` when it
    /// has none or its record is malformed.
    fn last_resume(&mut self, session: &str) -> io::Result<Option<i64>>;
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
    /// Takes back this caller's own [`GuardStore::commit`] of `event` in
    /// `session` after its keys did not reach the session (issue #3104), so
    /// the event stays armed for the next room event.
    ///
    /// Precondition: `now` is the value this caller passed to `commit`.
    /// Under the commit's lock, the event's marker is removed (a marker that
    /// is already absent is success) and then one occurrence of `now` is
    /// removed from the session's record. Times the commit pruned are not
    /// restored. A marker that is not a regular file of this user, or a
    /// record that cannot be parsed, is an error and is left in place.
    fn release(&mut self, event: &str, session: &str, now: i64) -> io::Result<()>;
}

/// The on-disk guard for a profile root. Creating one touches nothing.
#[derive(Clone)]
pub struct FsGuard {
    root: PathBuf,
    lock_timeout: Duration,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for FsGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsGuard")
            .field("root", &self.root)
            .field("lock_timeout", &self.lock_timeout)
            .finish_non_exhaustive()
    }
}

impl FsGuard {
    pub fn new(root: &Path) -> Self {
        Self::with_lock_timeout(root, LOCK_TIMEOUT)
    }

    pub fn with_lock_timeout(root: &Path, lock_timeout: Duration) -> Self {
        Self::with_clock(root, lock_timeout, Arc::new(SystemClock))
    }

    /// A guard whose commit lock wait is timed on `clock`. For tests only:
    /// a clock whose time never moves forward makes the wait unbounded
    /// while another commit holds the lock.
    #[doc(hidden)]
    pub fn with_clock(root: &Path, lock_timeout: Duration, clock: Arc<dyn Clock>) -> Self {
        Self {
            root: root.to_path_buf(),
            lock_timeout,
            clock,
        }
    }

    /// How long a commit waits for the lock. It exists so that tests can
    /// check the defaults.
    pub fn lock_timeout(&self) -> Duration {
        self.lock_timeout
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
    ///
    /// A record longer than [`MAX_SESSION_BYTES`] cannot hold a list this
    /// hook wrote, so it is malformed rather than an error, and it is never
    /// read into memory (V6). The size is checked only for a private
    /// regular file of this user; anything else falls through to the read,
    /// which refuses it.
    fn read_session(&self, file: &Path) -> io::Result<Option<Option<Vec<i64>>>> {
        match fs::symlink_metadata(file) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Ok(m) if m.is_file() && private_record(&m) && m.len() > MAX_SESSION_BYTES => {
                return Ok(Some(None));
            }
            _ => {}
        }
        let Some((raw, meta)) = read_bounded_nofollow(file, MAX_SESSION_BYTES)? else {
            return Ok(None);
        };
        if !private_record(&meta) {
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

/// A session record must be a regular file of this user, with no group or
/// other bits and exactly one link.
fn private_record(m: &fs::Metadata) -> bool {
    owned_by_euid(m) && m.mode() & 0o077 == 0 && m.nlink() == 1
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

    fn resumes(&mut self, session: &str, now: i64) -> io::Result<Resumes> {
        check_key(session)?;
        let Some(sessions) = self.existing_subdir("sessions")? else {
            return Ok(Resumes::default());
        };
        Ok(match self.read_session(&sessions.join(session))? {
            None => Resumes::default(),
            Some(None) => Resumes {
                count: 0,
                malformed: true,
            },
            Some(Some(times)) => Resumes {
                // Saturating, so a record of any length is a number (V6).
                count: u32::try_from(recent(&times, now).len()).unwrap_or(u32::MAX),
                malformed: false,
            },
        })
    }

    fn last_resume(&mut self, session: &str) -> io::Result<Option<i64>> {
        check_key(session)?;
        let Some(sessions) = self.existing_subdir("sessions")? else {
            return Ok(None);
        };
        Ok(match self.read_session(&sessions.join(session))? {
            Some(Some(times)) => times.into_iter().max(),
            _ => None,
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
        let _lock = lock_exclusive_timeout_with(
            open_private_nofollow(&dir.join("lock"))?,
            self.lock_timeout,
            &*self.clock,
        )?;

        self.prune(&events, now, MARKER_RETENTION_SECS);
        self.prune(&dir.join("prompted"), now, PROMPTED_RETENTION_SECS);
        let marker = events.join(event);
        match fs::symlink_metadata(&marker) {
            Ok(_) => return Err(CommitError::Duplicate),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        // A record that cannot be parsed means no earlier resumes (#2912).
        // It is rewritten below holding only this resume, so a damaged file
        // never leaves a session stopped. The hook logs the reset.
        let times = match self.read_session(&sessions.join(session))? {
            None | Some(None) => Vec::new(),
            Some(Some(times)) if times.iter().any(|t| *t > start) => {
                return Err(CommitError::Duplicate);
            }
            Some(Some(times)) => recent(&times, now),
        };

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

    fn release(&mut self, event: &str, session: &str, now: i64) -> io::Result<()> {
        check_key(event)?;
        check_key(session)?;
        // No `resume/` means nothing was ever committed: there is nothing
        // to take back, and nothing is created.
        let Some(dir) = self.existing_subdir("")? else {
            return Ok(());
        };
        let _lock = lock_exclusive_timeout_with(
            open_private_nofollow(&dir.join("lock"))?,
            self.lock_timeout,
            &*self.clock,
        )?;

        // The marker first: it is what refuses the event.
        if let Some(events) = self.existing_subdir("events")? {
            let marker = events.join(event);
            match fs::symlink_metadata(&marker) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
                Ok(m) if m.file_type().is_file() && owned_by_euid(&m) => {
                    // `remove_file` unlinks the name and never follows it.
                    fs::remove_file(&marker)?;
                    fsync_dir(&events)?;
                }
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "event marker is not a regular file of the current user",
                    ));
                }
            }
        }

        let Some(sessions) = self.existing_subdir("sessions")? else {
            return Ok(());
        };
        let mut times = match self.read_session(&sessions.join(session))? {
            None => return Ok(()),
            Some(None) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "session record is malformed",
                ));
            }
            Some(Some(times)) => times,
        };
        let Some(at) = times.iter().position(|t| *t == now) else {
            return Ok(());
        };
        times.remove(at);
        let record: String = times.iter().map(|t| format!("{t}\n")).collect();
        write_private_atomic(&sessions, session, record.as_bytes())
    }
}
