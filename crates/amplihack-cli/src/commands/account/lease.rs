//! Lease files and the lock that serialises picks (issue #2006).
//!
//! Each pick writes one lease, `<root>/<cli>/<pid-or-0>-<nanos>-<seq>.json`,
//! while it holds an exclusive `flock` on `<root>/<cli>/.lock`. The next
//! picker sees it and lowers that account's score, so picks started together
//! spread out. The lock wait has no deadline and no retry: the holder only
//! reads `/proc` and the lease files.
//!
//! The directories must be private to the user: a symlink, another owner,
//! or group or world write is refused, never repaired. Lease files are read
//! bounded, without following a symlink, and only when private. A file with
//! an unknown version or unparseable content is left where it is.

use super::Cli;
use super::pick::PickError;
use super::procscan::{JobCounts, read_bounded_nofollow};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const LEASE_VERSION: u32 = 1;
const LOCK_NAME: &str = ".lock";
const MAX_LEASE_BYTES: u64 = 4096;

static SEQ: AtomicU64 = AtomicU64::new(0);

/// One lease: an account chosen at `created_at` by `pid` (`None` for a
/// standalone `account pick`, whose lease expires by age only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    pub version: u32,
    pub cli: Cli,
    pub home: PathBuf,
    pub pid: Option<u32>,
    pub created_at: i64,
}

impl Lease {
    pub fn new(cli: Cli, home: PathBuf, pid: Option<u32>, created_at: i64) -> Self {
        Self {
            version: LEASE_VERSION,
            cli,
            home,
            pid,
            created_at,
        }
    }
}

/// A lease and the file it was read from.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub path: PathBuf,
    pub lease: Lease,
}

/// Whether `name` is a lease file name: `^[0-9]+-[0-9]+-[0-9]+\.json$`.
pub fn valid_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    let parts: Vec<&str> = stem.split('-').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Removes `path`; a file that is already gone counts as removed.
pub fn remove(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Whether `pid` is a running process: `kill(pid, 0)`, with `EPERM`
/// (another user's process) counted as running. A pid outside
/// `1..=i32::MAX` is never signalled and is not running.
pub fn pid_alive(pid: u32) -> bool {
    if !valid_pid(pid) {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists; `pid` is in
    // 1..=i32::MAX, so it names one process and never a group.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn valid_pid(pid: u32) -> bool {
    (1..=i32::MAX as u32).contains(&pid)
}

pub(super) fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn io_err(op: &'static str, path: &Path, source: std::io::Error) -> PickError {
    PickError::Io {
        op,
        path: path.to_path_buf(),
        source,
    }
}

/// Creates `dir` with mode 0700 when it is missing, then checks it: a real
/// directory (not a symlink), owned by the effective user, with no group or
/// world write. Nothing is repaired.
fn ensure_private_dir(dir: &Path) -> Result<(), PickError> {
    match fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
            {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io_err("create the lease directory", dir, e)),
            }
        }
        Err(e) => return Err(io_err("inspect the lease directory", dir, e)),
    }
    let meta =
        fs::symlink_metadata(dir).map_err(|e| io_err("inspect the lease directory", dir, e))?;
    let refuse = |why: &str| {
        Err(io_err(
            "use the lease directory",
            dir,
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, why.to_string()),
        ))
    };
    if meta.file_type().is_symlink() {
        return refuse("it is a symlink");
    }
    if !meta.is_dir() {
        return refuse("it is not a directory");
    }
    if meta.uid() != euid() {
        return refuse("it is owned by another user");
    }
    if meta.mode() & 0o022 != 0 {
        return refuse("it is writable by group or others");
    }
    Ok(())
}

/// `<root>/<cli>`, checked private.
#[derive(Debug)]
pub struct LeaseDir {
    dir: PathBuf,
}

impl LeaseDir {
    /// Creates and checks `<root>` and `<root>/<cli>`.
    pub fn open(root: &Path, cli: Cli) -> Result<Self, PickError> {
        ensure_private_dir(root)?;
        let dir = root.join(cli.as_str());
        ensure_private_dir(&dir)?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Takes the exclusive lock, waiting for any holder to release it. The
    /// lock file is never followed through a symlink.
    pub fn lock(&self) -> Result<LeaseLock<'_>, PickError> {
        let path = self.dir.join(LOCK_NAME);
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|e| io_err("open the lease lock", &path, e))?;
        let meta = file
            .metadata()
            .map_err(|e| io_err("inspect the lease lock", &path, e))?;
        if !meta.is_file() || meta.uid() != euid() {
            return Err(io_err(
                "use the lease lock",
                &path,
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "not a regular file owned by this user",
                ),
            ));
        }
        file.lock_exclusive()
            .map_err(|e| io_err("lock the lease directory", &path, e))?;
        Ok(LeaseLock { dir: self, file })
    }
}

/// The held lock; dropping it releases the lock.
#[derive(Debug)]
pub struct LeaseLock<'a> {
    dir: &'a LeaseDir,
    file: fs::File,
}

impl Drop for LeaseLock<'_> {
    fn drop(&mut self) {
        if let Err(e) = FileExt::unlock(&self.file) {
            // Closing the file releases the lock as well.
            tracing::debug!(%e, "explicit unlock of the lease lock failed");
        }
    }
}

impl LeaseLock<'_> {
    /// The leases on disk, in name order, and one warning per lease file
    /// that is not counted (unsafe, too large, unparseable or of an unknown
    /// version). Such files are left where they are.
    pub fn load(&self) -> (Vec<Stored>, Vec<String>) {
        let mut stored = Vec::new();
        let mut warnings = Vec::new();
        let rd = match fs::read_dir(&self.dir.dir) {
            Ok(rd) => rd,
            Err(e) => {
                warnings.push(format!(
                    "warning: account pick: could not list {}: {e}",
                    self.dir.dir.display()
                ));
                return (stored, warnings);
            }
        };
        let mut names: Vec<String> = rd
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| valid_name(n))
            .collect();
        names.sort();
        let me = euid();
        for name in names {
            let path = self.dir.dir.join(&name);
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue; // removed since the listing
            };
            if !meta.file_type().is_file() || meta.uid() != me || meta.mode() & 0o077 != 0 {
                warnings.push(format!(
                    "warning: account pick: ignoring lease {name:?}: not a private file of this user"
                ));
                continue;
            }
            let Some(bytes) = read_bounded_nofollow(&path, MAX_LEASE_BYTES) else {
                warnings.push(format!(
                    "warning: account pick: ignoring lease {name:?}: unreadable or too large"
                ));
                continue;
            };
            match serde_json::from_slice::<Lease>(&bytes) {
                Ok(lease) if lease.version == LEASE_VERSION => stored.push(Stored { path, lease }),
                Ok(_) => warnings.push(format!(
                    "warning: account pick: ignoring lease {name:?}: unknown version"
                )),
                Err(_) => warnings.push(format!(
                    "warning: account pick: ignoring lease {name:?}: not a valid lease"
                )),
            }
        }
        (stored, warnings)
    }

    /// Writes `lease` by a private temporary file and a rename, and returns
    /// its path. The file is synced before the rename, so a crash cannot
    /// leave an empty lease that every later pick would warn about. The
    /// directory is not synced: a rename lost in a crash loses a lease that
    /// no longer means anything once its processes are gone.
    pub fn write(&self, lease: &Lease) -> Result<PathBuf, PickError> {
        let body = serde_json::to_vec(lease).map_err(|e| {
            io_err(
                "serialise a lease",
                &self.dir.dir,
                std::io::Error::other(e.to_string()),
            )
        })?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = lease.pid.unwrap_or(0);
        let (name, path) = loop {
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let name = format!("{pid}-{nanos}-{seq}.json");
            let path = self.dir.dir.join(&name);
            // Under the lock no other picker writes, so a free name stays free.
            if fs::symlink_metadata(&path).is_err() {
                break (name, path);
            }
        };
        let tmp = self.dir.dir.join(format!(".tmp-{name}"));
        let result = (|| -> std::io::Result<()> {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&tmp)?;
            f.write_all(&body)?;
            f.sync_all()?;
            fs::rename(&tmp, &path)
        })();
        if let Err(e) = result {
            if let Err(rm) = remove(&tmp) {
                eprintln!(
                    "warning: account pick: could not remove {}: {rm}",
                    tmp.display()
                );
            }
            return Err(io_err("write a lease", &path, e));
        }
        Ok(path)
    }

    /// Deletes, and leaves out of the result, every lease that no longer
    /// counts: one at least `ttl_secs` old, one with a malformed pid, one
    /// whose pid is a counted job on the same home or one of that job's
    /// ancestors (the launch it made now counts instead), and one whose pid
    /// has exited.
    pub fn sweep(
        &self,
        stored: Vec<Stored>,
        jobs: &JobCounts,
        now: i64,
        ttl_secs: u64,
        alive: &dyn Fn(u32) -> bool,
    ) -> Vec<Stored> {
        let mut kept = Vec::new();
        for s in stored {
            let age = i128::from(now) - i128::from(s.lease.created_at);
            let drop = if age >= i128::from(ttl_secs) {
                true
            } else if let Some(pid) = s.lease.pid {
                !valid_pid(pid)
                    || jobs.jobs.iter().any(|j| {
                        j.home == s.lease.home && (j.pid == pid || j.ancestors.contains(&pid))
                    })
                    || !alive(pid)
            } else {
                false
            };
            if drop {
                if let Err(e) = remove(&s.path) {
                    eprintln!(
                        "warning: account pick: could not remove lease {}: {e}",
                        s.path.display()
                    );
                }
            } else {
                kept.push(s);
            }
        }
        kept
    }
}
