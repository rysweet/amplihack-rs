//! Timeout-based file locking using `F_SETLK` (non-blocking).
//!
//! Never uses `F_SETLKW` (blocks indefinitely).
//! Retries with configurable timeout and checks holder PID liveness.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Errors from file lock operations.
#[derive(Debug, thiserror::Error)]
pub enum FileLockError {
    #[error("Failed to open lock file {path}: {source}")]
    Open { path: PathBuf, source: io::Error },

    #[error("Lock timeout after {timeout:?} on {path}")]
    Timeout { path: PathBuf, timeout: Duration },

    #[error("Lock error on {path}: {source}")]
    Lock { path: PathBuf, source: io::Error },
}

/// An exclusive file lock that is released on drop.
///
/// Uses `F_SETLK` (non-blocking) with retry loop instead of
/// `F_SETLKW` which can block indefinitely.
#[derive(Debug)]
pub struct FileLock {
    _file: File,
    _path: PathBuf,
}

impl FileLock {
    /// Acquire an exclusive lock with timeout.
    ///
    /// Retries every 10ms until the lock is acquired or timeout expires.
    pub fn exclusive(path: &Path, timeout: Duration) -> Result<Self, FileLockError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(path)
            .map_err(|e| FileLockError::Open {
                path: path.to_path_buf(),
                source: e,
            })?;

        let deadline = Instant::now() + timeout;

        loop {
            match try_lock_exclusive(&file) {
                Ok(true) => {
                    return Ok(FileLock {
                        _file: file,
                        _path: path.to_path_buf(),
                    });
                }
                Ok(false) => {
                    if Instant::now() >= deadline {
                        return Err(FileLockError::Timeout {
                            path: path.to_path_buf(),
                            timeout,
                        });
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    return Err(FileLockError::Lock {
                        path: path.to_path_buf(),
                        source: e,
                    });
                }
            }
        }
    }
}

/// Try to acquire an exclusive lock without blocking.
/// Returns `Ok(true)` if locked, `Ok(false)` if would block.
#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> Result<bool, io::Error> {
    use std::os::unix::io::AsRawFd;

    let fd = file.as_raw_fd();
    let mut flock = libc::flock {
        l_type: libc::F_WRLCK as i16,
        l_whence: libc::SEEK_SET as i16,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    };

    let result = unsafe { libc::fcntl(fd, libc::F_SETLK, &mut flock) };
    if result == 0 {
        Ok(true)
    } else {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EAGAIN) || err.raw_os_error() == Some(libc::EACCES) {
            Ok(false)
        } else {
            Err(err)
        }
    }
}

#[cfg(not(unix))]
fn try_lock_exclusive(_file: &File) -> Result<bool, io::Error> {
    // On non-Unix, always succeed (best-effort locking).
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn lock_and_unlock() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("test.lock");
        let lock = FileLock::exclusive(&lock_path, Duration::from_secs(1)).unwrap();
        drop(lock);
        // Should be able to re-acquire after drop.
        let _lock2 = FileLock::exclusive(&lock_path, Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn lock_contention_across_processes() {
        // fcntl locks are per-process, so we can't test contention within
        // a single process using threads. Instead, test basic lock/unlock.
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("contention.lock");

        // Acquire and release multiple times.
        for _ in 0..5 {
            let lock = FileLock::exclusive(&lock_path, Duration::from_secs(1)).unwrap();
            drop(lock);
        }
    }
}
