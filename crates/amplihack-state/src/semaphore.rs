//! TOCTOU-safe atomic flags using `O_CREAT|O_EXCL`.
//!
//! A semaphore file that either exists or doesn't. Creation is atomic
//! (O_CREAT|O_EXCL guarantees no race). Useful for one-shot flags like
//! "session initialized" or "migration complete".

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Errors from atomic flag operations.
#[derive(Debug, thiserror::Error)]
pub enum AtomicFlagError {
    #[error("IO error on {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

/// A TOCTOU-safe boolean flag backed by a file.
///
/// `set()` uses `O_CREAT|O_EXCL` for atomic creation.
/// `is_set()` checks file existence.
/// `clear()` removes the file.
pub struct AtomicFlag {
    path: PathBuf,
}

impl AtomicFlag {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Atomically set the flag. Returns `true` if newly created,
    /// `false` if already set (file existed).
    pub fn set(&self) -> Result<bool, AtomicFlagError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| AtomicFlagError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
        {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(AtomicFlagError::Io {
                path: self.path.clone(),
                source: e,
            }),
        }
    }

    /// Check if the flag is set.
    pub fn is_set(&self) -> bool {
        self.path.exists()
    }

    /// Clear the flag.
    pub fn clear(&self) -> Result<(), AtomicFlagError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AtomicFlagError::Io {
                path: self.path.clone(),
                source: e,
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_clear_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let flag = AtomicFlag::new(dir.path().join("test.flag"));
        assert!(!flag.is_set());
        assert!(flag.set().unwrap()); // newly created
        assert!(flag.is_set());
        assert!(!flag.set().unwrap()); // already exists
        flag.clear().unwrap();
        assert!(!flag.is_set());
    }

    #[test]
    fn clear_nonexistent_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let flag = AtomicFlag::new(dir.path().join("gone.flag"));
        flag.clear().unwrap(); // should not error
    }
}
