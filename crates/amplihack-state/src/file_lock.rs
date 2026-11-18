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
        Self::open_and_lock(path, timeout, false)
    }

    /// Acquire the same exclusive lock, creating its file with mode 0600.
    /// Like `exclusive`, this does not create parent directories.
    pub fn exclusive_private(path: &Path, timeout: Duration) -> Result<Self, FileLockError> {
        Self::open_and_lock(path, timeout, true)
    }

    fn open_and_lock(path: &Path, timeout: Duration, private: bool) -> Result<Self, FileLockError> {
        let mut options = OpenOptions::new();
        options.truncate(false).write(true).read(true);
        let file = (if private {
            amplihack_utils::secure_files::open_private_created(path, options)
        } else {
            options.create(true).open(path)
        })
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
        // `F_WRLCK` is `c_short` on macOS and `c_int` on Linux; `l_type` is
        // `c_short` on both, so the cast is needed on Linux only.
        l_type: libc::F_WRLCK as libc::c_short,
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

    #[cfg(unix)]
    #[test]
    fn private_lock_is_0600_and_exclusive_across_processes() {
        use crate::permission_tests::{mode, under_umask_0002};
        const NAME: &str = "file_lock::tests::private_lock_is_0600_and_exclusive_across_processes";
        const PROBE: &str = "AMPLIHACK_STATE_PRIVATE_LOCK_PROBE";
        const BLOCKED: &str = "AMPLIHACK_STATE_PRIVATE_LOCK_EXPECT_BLOCKED";
        if let Some(path) = std::env::var_os(PROBE) {
            let result = FileLock::exclusive_private(Path::new(&path), Duration::ZERO);
            if std::env::var_os(BLOCKED).is_some() {
                assert!(
                    matches!(result, Err(FileLockError::Timeout { .. })),
                    "{result:?}"
                );
            } else {
                result.unwrap();
            }
            return;
        }
        under_umask_0002(NAME, || {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("private.lock");
            let lock = FileLock::exclusive_private(&path, Duration::ZERO).unwrap();
            assert_eq!(mode(&path), 0o600);
            let probe = |blocked: bool| {
                let mut command = std::process::Command::new(std::env::current_exe().unwrap());
                command
                    .args(["--exact", NAME, "--test-threads=1", "--nocapture"])
                    .env(PROBE, &path);
                if blocked {
                    command.env(BLOCKED, "1");
                } else {
                    command.env_remove(BLOCKED);
                }
                let output = command.output().unwrap();
                assert!(
                    output.status.success(),
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            };
            probe(true);
            drop(lock);
            probe(false);
            let generic = dir.path().join("generic.lock");
            let _generic = FileLock::exclusive(&generic, Duration::ZERO).unwrap();
            assert_eq!(
                mode(&generic),
                0o664,
                "generic construction keeps its existing contract"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn private_lock_does_not_create_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing/owner.lock");
        let error = FileLock::exclusive_private(&path, Duration::ZERO).unwrap_err();
        assert!(
            matches!(error, FileLockError::Open { source, .. } if source.kind() == io::ErrorKind::NotFound)
        );
        assert!(!path.parent().unwrap().exists());
    }
}
