//! Secure file and directory creation with restrictive permissions.
//!
//! Provides helpers that create files and directories with owner-only
//! access (`0o700` for dirs, `0o600` for files) on Unix systems.
//!
//! The Unix-only helpers below (`read_bounded_nofollow`,
//! `write_private_atomic`, `ensure_private_dir`, …) moved here from the
//! CLI's auto-switch `fsx.rs` (issue #1658) so the limit-resume hook, which
//! cannot depend on the CLI, uses the same implementation. Unlike
//! `ensure_private_directory` and `open_private_append`, they never create
//! parent directories, never follow a final symlink and never change the
//! mode of an existing path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

/// No passwd database off Unix: always `None`.
#[cfg(not(unix))]
pub fn passwd_home() -> Option<PathBuf> {
    None
}

/// The canonical temporary-directory bases: `temp_dir()`, an absolute
/// `$TMPDIR` and `/tmp`, in that order and without duplicates. A base that
/// cannot be canonicalised is left out.
///
/// The one list used both by the CLI's profile settings refresh (to refuse a
/// hooks binary from a temporary build) and by the issue #1699 test guard (to
/// skip profile roots that are test fixtures), so the two cannot drift.
pub fn temp_bases() -> Vec<PathBuf> {
    let tmpdir = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let mut bases: Vec<PathBuf> = Vec::new();
    for candidate in [
        Some(std::env::temp_dir()),
        tmpdir,
        Some(PathBuf::from("/tmp")),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(base) = candidate.canonicalize()
            && !bases.contains(&base)
        {
            bases.push(base);
        }
    }
    bases
}

/// An exclusive `flock` held on a file; released when dropped.
#[derive(Debug)]
pub struct ExclusiveLock {
    #[cfg_attr(not(unix), allow(dead_code))]
    file: fs::File,
}

impl ExclusiveLock {
    /// The locked file.
    pub fn file(&self) -> &fs::File {
        &self.file
    }
}

#[cfg(unix)]
impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // SAFETY: the descriptor is owned by `self.file` and still open.
        // Closing the file would release the lock too; unlocking first makes
        // the release independent of other descriptors for the same file.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Takes an exclusive `flock` on `file`, retrying every 100 ms until
/// `timeout` has passed. A zero timeout tries once. Gives up with
/// `ErrorKind::TimedOut`.
#[cfg(unix)]
pub fn lock_exclusive_timeout(file: fs::File, timeout: Duration) -> io::Result<ExclusiveLock> {
    use std::os::unix::io::AsRawFd;
    const RETRY: Duration = Duration::from_millis(100);
    let start = std::time::Instant::now();
    loop {
        // SAFETY: the descriptor is owned by `file` and open.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(ExclusiveLock { file });
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => {}
            _ => return Err(err),
        }
        let waited = start.elapsed();
        if waited >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for an exclusive lock",
            ));
        }
        std::thread::sleep(RETRY.min(timeout - waited));
    }
}

/// Not supported off Unix: always `ErrorKind::Unsupported`.
#[cfg(not(unix))]
pub fn lock_exclusive_timeout(
    _file: fs::File,
    _timeout: std::time::Duration,
) -> io::Result<ExclusiveLock> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "exclusive file locks need Unix",
    ))
}

/// Create a directory (and parents) with owner-only permissions.
///
/// On Unix the directory mode is set to `0o700`. On non-Unix platforms
/// the directory is created with default permissions.
pub fn ensure_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o700);
        fs::set_permissions(path, perms)?;
    }

    Ok(())
}

/// Open a file for appending with owner-only permissions.
///
/// On Unix the file is created with mode `0o600` using `O_APPEND | O_CREAT | O_WRONLY`.
/// On non-Unix platforms the file is created with default permissions.
pub fn open_private_append(path: &Path) -> io::Result<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
    }

    #[cfg(not(unix))]
    {
        fs::OpenOptions::new().create(true).append(true).open(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn temp_bases_are_canonical_and_distinct() {
        let bases = temp_bases();
        assert!(!bases.is_empty(), "temp_dir() always canonicalises here");
        for base in &bases {
            assert_eq!(&base.canonicalize().unwrap(), base);
        }
        let mut sorted = bases.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), bases.len());
        let temp = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(bases[0], temp);
    }

    #[cfg(unix)]
    #[test]
    fn passwd_home_is_absolute_when_present() {
        if let Some(home) = passwd_home() {
            assert!(home.is_absolute(), "{}", home.display());
        }
    }

    #[test]
    fn ensure_private_directory_creates_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("secure").join("nested");

        ensure_private_directory(&target).unwrap();
        assert!(target.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_directory_sets_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("private_dir");

        ensure_private_directory(&target).unwrap();

        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "directory should be 0o700, got {mode:o}");
    }

    #[test]
    fn ensure_private_directory_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("idem");

        ensure_private_directory(&target).unwrap();
        ensure_private_directory(&target).unwrap();
        assert!(target.is_dir());
    }

    #[test]
    fn open_private_append_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("data").join("log.txt");

        let mut f = open_private_append(&target).unwrap();
        writeln!(f, "line 1").unwrap();
        drop(f);

        assert!(target.exists());
        let content = fs::read_to_string(&target).unwrap();
        assert!(content.contains("line 1"));
    }

    #[test]
    fn open_private_append_actually_appends() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("append.log");

        let mut f = open_private_append(&target).unwrap();
        writeln!(f, "first").unwrap();
        drop(f);

        let mut f = open_private_append(&target).unwrap();
        writeln!(f, "second").unwrap();
        drop(f);

        let content = fs::read_to_string(&target).unwrap();
        assert!(content.contains("first"));
        assert!(content.contains("second"));
    }

    #[cfg(unix)]
    #[test]
    fn open_private_append_sets_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("secure.log");

        let f = open_private_append(&target).unwrap();
        drop(f);

        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "file should be 0o600, got {mode:o}");
    }
}
