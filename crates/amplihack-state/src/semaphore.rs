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
    private: bool,
}

impl AtomicFlag {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            private: false,
        }
    }

    /// Create missing parents with mode 0700 and the flag with mode 0600.
    /// Existing parent permissions and exclusive-create behavior are preserved.
    #[must_use]
    pub fn new_private(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            private: true,
        }
    }

    /// Atomically set the flag. Returns `true` if newly created,
    /// `false` if already set (file existed).
    pub fn set(&self) -> Result<bool, AtomicFlagError> {
        if let Some(parent) = self.path.parent() {
            (if self.private {
                amplihack_utils::secure_files::create_private_directories(parent)
            } else {
                fs::create_dir_all(parent)
            })
            .map_err(|e| AtomicFlagError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if self.private {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&self.path) {
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

    #[cfg(unix)]
    #[test]
    fn private_flag_creates_0700_parents_and_a_0600_file_under_umask_0002() {
        use crate::permission_tests::{mode, under_umask_0002};
        under_umask_0002(
            "semaphore::tests::private_flag_creates_0700_parents_and_a_0600_file_under_umask_0002",
            || {
                let dir = tempfile::tempdir().unwrap();
                let parent = dir.path().join("session/nested");
                let flag = AtomicFlag::new_private(parent.join("initialized"));
                assert!(flag.set().unwrap());
                assert_eq!(mode(&dir.path().join("session")), 0o700);
                assert_eq!(mode(&parent), 0o700);
                assert_eq!(mode(flag.path()), 0o600);
                assert!(
                    !flag.set().unwrap(),
                    "private creation retains exclusive-create semantics"
                );
                flag.clear().unwrap();
                assert!(!flag.is_set());
            },
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_flag_preserves_existing_parents_and_generic_flag_modes() {
        use crate::permission_tests::{mode, under_umask_0002};
        use std::os::unix::fs::PermissionsExt;
        under_umask_0002(
            "semaphore::tests::private_flag_preserves_existing_parents_and_generic_flag_modes",
            || {
                let dir = tempfile::tempdir().unwrap();
                fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
                let private = AtomicFlag::new_private(dir.path().join("private.flag"));
                assert!(private.set().unwrap());
                assert_eq!(mode(dir.path()), 0o755);
                let generic = AtomicFlag::new(dir.path().join("generic/flag"));
                assert!(generic.set().unwrap());
                assert_eq!(mode(generic.path()), 0o664);
                assert_eq!(mode(generic.path().parent().unwrap()), 0o775);
            },
        );
    }
}
