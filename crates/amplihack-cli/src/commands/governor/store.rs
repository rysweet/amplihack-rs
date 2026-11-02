//! `<root>/.governor.d` (D13): the only code that touches it. The directory
//! is private (0700) and never chmodded; files are written privately and
//! atomically under an exclusive lock that is separate from the rotator's.
//! Reads are bounded and never follow a symlink. A corrupt file is renamed
//! to its fixed `.corrupt` name and rebuilt; a file from a newer version is
//! left alone. `peek` is the read-only path for `governor status`.

use super::model::{FailOpen, FailOpenCode};
use amplihack_utils::secure_files::{
    ensure_private_dir, euid, open_private_nofollow, read_bounded_nofollow, write_private_atomic,
};
use fs4::fs_std::FileExt;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use super::files::{History, Persisted, Queue, State, repair};

pub const GOVERNOR_DIR: &str = ".governor.d";
const LOCK_FILE: &str = "lock";
/// The largest state file read.
const MAX_FILE: u64 = 1 << 20;
const LOCK_POLL: Duration = Duration::from_millis(20);

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// An open `.governor.d`.
pub struct Store {
    dir: PathBuf,
}

/// The exclusive lock; released when dropped.
pub struct StoreLock {
    _file: fs::File,
}

/// What a read-only look at one file found.
pub enum Peek<T> {
    Ok(T),
    Missing,
    Corrupt,
    NewerVersion,
}

/// What a file's bytes are.
enum Parsed<T> {
    Ok(T),
    Corrupt(String),
    Newer,
}

fn parse<T: Persisted>(raw: &[u8], now: i64) -> Parsed<T> {
    let probe: serde_json::Value = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => return Parsed::Corrupt(format!("not JSON at line {}", e.line())),
    };
    match probe.get("version").and_then(serde_json::Value::as_u64) {
        Some(v) if v > u64::from(super::files::GOVERNOR_STATE_VERSION) => return Parsed::Newer,
        _ => {}
    }
    let value: T = match serde_json::from_value(probe) {
        Ok(v) => v,
        // serde's text can quote the file's own content, so only its
        // category and position are kept.
        Err(e) => {
            return Parsed::Corrupt(format!(
                "unexpected content ({:?} at line {}, column {})",
                e.classify(),
                e.line(),
                e.column()
            ));
        }
    };
    if value.version() != super::files::GOVERNOR_STATE_VERSION {
        return Parsed::Corrupt("unknown version".into());
    }
    match value.validate(now) {
        Ok(()) => Parsed::Ok(value),
        Err(e) => Parsed::Corrupt(e),
    }
}

fn state_dir_err(e: impl std::fmt::Display) -> FailOpen {
    FailOpen::new(FailOpenCode::StateDir, format!("{GOVERNOR_DIR}: {e}"))
}

fn corrupt(file: &str, why: impl std::fmt::Display) -> FailOpen {
    FailOpen::new(
        FailOpenCode::CorruptState,
        format!("{GOVERNOR_DIR}/{file}: {why}"),
    )
}

/// A regular file owned by this user, checked without following a symlink.
fn own_regular(path: &Path) -> Option<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) => Some(m.file_type().is_file() && m.uid() == euid()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(false),
    }
}

impl Store {
    /// Creates `<root>/.governor.d` (0700) or checks the existing one: a
    /// real directory owned by this user with no group or other bits.
    pub fn open(root: &Path) -> Result<Self, FailOpen> {
        match fs::symlink_metadata(root) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(state_dir_err("profile root is not a directory")),
            Err(e) => return Err(state_dir_err(format!("profile root: {}", e.kind()))),
        }
        let dir = root.join(GOVERNOR_DIR);
        ensure_private_dir(&dir).map_err(state_dir_err)?;
        Ok(Self { dir })
    }

    /// Takes the exclusive lock, trying for at most `timeout`.
    pub fn lock_exclusive(&self, timeout: Duration) -> Result<StoreLock, FailOpen> {
        let file = open_private_nofollow(&self.dir.join(LOCK_FILE))
            .map_err(|e| state_dir_err(format!("{LOCK_FILE}: {e}")))?;
        let start = Instant::now();
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(true) => return Ok(StoreLock { _file: file }),
                Ok(false) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(state_dir_err(format!("{LOCK_FILE}: {e}"))),
            }
            if start.elapsed() >= timeout {
                return Err(FailOpen::new(
                    FailOpenCode::LockTimeout,
                    format!(
                        "{GOVERNOR_DIR}/{LOCK_FILE} was busy for {} ms",
                        timeout.as_millis()
                    ),
                ));
            }
            std::thread::sleep(LOCK_POLL.min(timeout));
        }
    }

    /// Loads one file under the lock. Missing gives the default; corrupt is
    /// quarantined and gives `corrupt-state`; newer gives `state-version`.
    pub fn load<T: Persisted>(&self, _lock: &StoreLock) -> Result<T, FailOpen> {
        let path = self.dir.join(T::FILE);
        match own_regular(&path) {
            None => return Ok(T::default()),
            Some(true) => {}
            Some(false) => return Err(corrupt(T::FILE, "not a regular file owned by this user")),
        }
        let raw = match read_bounded_nofollow(&path, MAX_FILE) {
            Ok(Some((raw, _))) => raw,
            Ok(None) => return Ok(T::default()),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                return Err(self.quarantine::<T>(e));
            }
            Err(e) => return Err(corrupt(T::FILE, e)),
        };
        match parse::<T>(&raw, now()) {
            Parsed::Ok(v) => Ok(v),
            Parsed::Corrupt(why) => Err(self.quarantine::<T>(why)),
            Parsed::Newer => Err(FailOpen::new(
                FailOpenCode::StateVersion,
                format!(
                    "{GOVERNOR_DIR}/{} is from a newer amplihack; left unchanged",
                    T::FILE
                ),
            )),
        }
    }

    /// Renames a corrupt file to its `.corrupt` name, unless that name is
    /// taken by something other than a regular file of this user.
    fn quarantine<T: Persisted>(&self, why: impl std::fmt::Display) -> FailOpen {
        let target = self.dir.join(T::CORRUPT);
        if own_regular(&target) == Some(false) {
            return corrupt(
                T::FILE,
                format!(
                    "{why}; {} is not a regular file, not quarantined",
                    T::CORRUPT
                ),
            );
        }
        match fs::rename(self.dir.join(T::FILE), &target) {
            Ok(()) => {
                tracing::warn!(
                    file = T::FILE,
                    "governor state was corrupt and was quarantined"
                );
                corrupt(T::FILE, format!("{why}; moved to {}", T::CORRUPT))
            }
            Err(e) => corrupt(T::FILE, format!("{why}; quarantine failed: {}", e.kind())),
        }
    }

    fn write<T: Persisted>(&self, value: &T) -> Result<(), FailOpen> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| corrupt(T::FILE, e))?;
        write_private_atomic(&self.dir, T::FILE, &bytes)
            .map_err(|e| state_dir_err(format!("{}: {e}", T::FILE)))
    }

    /// Writes history, then queue, then state.
    pub fn save(
        &self,
        _lock: &StoreLock,
        history: &History,
        queue: &Queue,
        state: &State,
    ) -> Result<(), FailOpen> {
        self.write(history)?;
        self.write(queue)?;
        self.write(state)
    }

    /// Writes `queue.json` alone.
    pub fn save_queue(&self, _lock: &StoreLock, queue: &Queue) -> Result<(), FailOpen> {
        self.write(queue)
    }

    /// Writes `state.json` alone.
    pub fn save_state(&self, _lock: &StoreLock, state: &State) -> Result<(), FailOpen> {
        self.write(state)
    }

    /// Reads one file without a lock, creating, renaming and writing
    /// nothing. A missing root or directory is `Missing`.
    pub fn peek<T: Persisted>(root: &Path) -> Peek<T> {
        let path = root.join(GOVERNOR_DIR).join(T::FILE);
        match own_regular(&path) {
            None => return Peek::Missing,
            Some(true) => {}
            Some(false) => return Peek::Corrupt,
        }
        match read_bounded_nofollow(&path, MAX_FILE) {
            Ok(Some((raw, _))) => match parse::<T>(&raw, now()) {
                Parsed::Ok(v) => Peek::Ok(v),
                Parsed::Corrupt(_) => Peek::Corrupt,
                Parsed::Newer => Peek::NewerVersion,
            },
            Ok(None) => Peek::Missing,
            Err(_) => Peek::Corrupt,
        }
    }
}
