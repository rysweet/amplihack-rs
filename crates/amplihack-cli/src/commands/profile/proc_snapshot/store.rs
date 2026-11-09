//! Where the snapshot lives and how it is read, locked and written (D1, D5).
//! Every check fails towards "no cache": a directory or file that fails one
//! is left as it is and the caller scans for itself.

use super::record::Snapshot;
use amplihack_utils::secure_files::{
    check_private_dir, check_private_file, invalid, read_bounded_nofollow, write_private_atomic,
};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// A snapshot is fresh while `0 <= now - taken_at < FRESH_SECS`.
pub const FRESH_SECS: i64 = 30;
/// A larger file is not a snapshot.
pub const MAX_SNAPSHOT: u64 = 4 * 1024 * 1024;
/// The subdirectory of the runtime directory that holds the files.
const SUBDIR: &str = "amplihack";

/// The first 16 hex characters of the SHA-256 of the two canonical paths'
/// raw bytes, separated by a zero byte (SR13, SR22).
pub fn key(canonical_proc_root: &Path, canonical_root: &Path) -> String {
    let mut h = Sha256::new();
    h.update(canonical_proc_root.as_os_str().as_bytes());
    h.update([0u8]);
    h.update(canonical_root.as_os_str().as_bytes());
    h.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The base directory for snapshots: `xdg` when it is an absolute real
/// directory owned by `euid` with no group or other bits; otherwise
/// `<home>/.amplihack/runtime`, created `0700` when missing, provided
/// `~/.amplihack` already exists, is owned by `euid` and is not writable by
/// group or others. An existing directory that fails a check is never
/// repaired, and `~/.amplihack` is never created here.
pub fn runtime_dir(xdg: Option<&OsStr>, home: &Path, euid: u32) -> Option<PathBuf> {
    if let Some(x) = xdg.map(Path::new)
        && x.is_absolute()
        && fs::symlink_metadata(x)
            .is_ok_and(|m| m.is_dir() && m.uid() == euid && m.mode() & 0o077 == 0)
    {
        return Some(x.to_path_buf());
    }
    if !home.is_absolute() {
        return None;
    }
    let amplihack = home.join(".amplihack");
    let parent_ok = fs::symlink_metadata(&amplihack)
        .is_ok_and(|m| m.is_dir() && m.uid() == euid && m.mode() & 0o022 == 0);
    if !parent_ok {
        return None;
    }
    let runtime = amplihack.join("runtime");
    private_dir(&runtime, euid).ok()?;
    Some(runtime)
}

/// Creates `path` with mode `0700` when it is missing, then checks it: a
/// real directory owned by `euid` with no group or other bits.
fn private_dir(path: &Path, euid: u32) -> io::Result<()> {
    create_0700(path)?;
    let m = fs::symlink_metadata(path)?;
    if m.file_type().is_symlink() || !m.is_dir() || m.uid() != euid || m.mode() & 0o077 != 0 {
        return Err(invalid("is not a private directory"));
    }
    Ok(())
}

/// Creates `path` with mode `0700`; an existing entry is left as it is.
fn create_0700(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

/// The snapshot files of one key in one checked directory.
pub struct Store {
    dir: PathBuf,
    key: String,
}

/// What a reader finds.
pub enum Found {
    Fresh(Snapshot),
    Stale(Snapshot),
    /// Missing, or failed a check: treated alike.
    Missing,
}

/// The result of one non-blocking attempt on the snapshot lock.
pub enum Lock {
    Held(Held),
    Busy,
}

/// The held snapshot lock. Dropping it unlocks the file before closing it:
/// a child forked by another thread holds a copy of the descriptor until it
/// execs, and `flock` would otherwise stay held until that copy closes too.
pub struct Held(File);

#[cfg(test)]
impl Held {
    /// A second descriptor on the same open file, as a forked child has.
    pub fn share(&self) -> File {
        self.0.try_clone().unwrap()
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // Closing the file releases the lock anyway once every copy is closed.
        let _ = FileExt::unlock(&self.0);
    }
}

impl Store {
    /// `<runtime>/amplihack/`, created `0700` when missing and checked on
    /// every use without following a symlink.
    pub fn open(runtime: &Path, key: String) -> io::Result<Self> {
        let dir = runtime.join(SUBDIR);
        create_0700(&dir)?;
        check_private_dir(&dir)?;
        Ok(Self { dir, key })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    fn json_name(&self) -> String {
        format!("proc-snapshot-{}.json", self.key)
    }

    fn lock_path(&self) -> PathBuf {
        self.dir.join(format!("proc-snapshot-{}.lock", self.key))
    }

    /// The snapshot, judged against `uid` and the clock `now`.
    pub fn read(&self, uid: u32, now: i64) -> Found {
        let path = self.dir.join(self.json_name());
        let raw = match read_bounded_nofollow(&path, MAX_SNAPSHOT) {
            Ok(Some((raw, meta))) => match check_private_file(&meta) {
                Ok(()) => raw,
                Err(e) => {
                    tracing::debug!("proc snapshot {}: {e}; ignored", path.display());
                    return Found::Missing;
                }
            },
            Ok(None) => return Found::Missing,
            Err(e) => {
                tracing::debug!("proc snapshot {}: {e}; ignored", path.display());
                return Found::Missing;
            }
        };
        let Ok(snap) = serde_json::from_slice::<Snapshot>(&raw) else {
            tracing::debug!("proc snapshot {}: unreadable; ignored", path.display());
            return Found::Missing;
        };
        if !snap.matches(uid, &self.key) {
            return Found::Missing;
        }
        match now.checked_sub(snap.taken_at) {
            Some(age) if (0..FRESH_SECS).contains(&age) => Found::Fresh(snap),
            Some(age) if age >= FRESH_SECS => Found::Stale(snap),
            // From the future, or an age that overflows.
            _ => Found::Missing,
        }
    }

    /// One non-blocking attempt on the lock file, opened without following
    /// a symlink, close-on-exec, mode `0600`, never truncated. A file that
    /// fails [`check_private_file`] is an error, not a busy lock.
    pub fn try_lock(&self) -> io::Result<Lock> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(self.lock_path())
            .map_err(|e| {
                if e.raw_os_error() == Some(libc::ELOOP) {
                    invalid("is a symlink")
                } else {
                    e
                }
            })?;
        check_private_file(&file.metadata()?)?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Lock::Held(Held(file))),
            Err(e) if e.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                Ok(Lock::Busy)
            }
            Err(e) => Err(e),
        }
    }

    /// Replaces the snapshot. The caller holds the lock.
    pub fn write(&self, snap: &Snapshot) -> io::Result<()> {
        let bytes = serde_json::to_vec(snap).map_err(io::Error::other)?;
        write_private_atomic(&self.dir, &self.json_name(), &bytes)
    }
}
