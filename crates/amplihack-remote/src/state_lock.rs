//! Advisory file-locking utilities for state management.
//!
//! Uses POSIX `flock(2)` to provide exclusive file locks that
//! coordinate concurrent access to JSON state files.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// RAII guard that releases the lock on drop.
pub struct FileLockGuard {
    _file: File,
}

/// Acquire an exclusive advisory file lock.
///
/// Blocks until the lock is available. The lock is released when the
/// returned [`FileLockGuard`] is dropped.
pub fn file_lock(lock_path: &Path) -> io::Result<FileLockGuard> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;

    // Acquire exclusive lock (blocking).
    flock_exclusive(&file)?;

    Ok(FileLockGuard { _file: file })
}

// -- platform-specific flock wrappers --

#[cfg(unix)]
fn flock_exclusive(file: &File) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn flock_exclusive(_file: &File) -> io::Result<()> {
    // No-op on non-unix; Windows would need LockFileEx.
    Ok(())
}

// Lock is released when the File is closed (guard dropped).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("test.lock");

        {
            let _guard = file_lock(&lock_path).unwrap();
            assert!(lock_path.exists());
        }
        // Guard dropped → lock released; re-acquire should succeed.
        let _guard2 = file_lock(&lock_path).unwrap();
    }

    #[test]
    fn creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("a/b/c/test.lock");
        let _guard = file_lock(&lock_path).unwrap();
        assert!(lock_path.exists());
    }

    #[test]
    fn lock_file_is_reusable() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("reuse.lock");
        for _ in 0..5 {
            let _g = file_lock(&lock_path).unwrap();
        }
    }
}
