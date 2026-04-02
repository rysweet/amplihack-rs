//! Atomic JSON file operations with panic-safe temp cleanup.
//!
//! Uses temp file + rename for atomic writes. Lock is acquired before
//! read-modify-write to prevent concurrent corruption.

use crate::file_lock::FileLock;
use serde::{Serialize, de::DeserializeOwned};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::NamedTempFile;

/// Errors from atomic JSON file operations.
#[derive(Debug, thiserror::Error)]
pub enum AtomicJsonError {
    #[error("IO error on {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("JSON parse error on {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("JSON serialize error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("Lock error: {0}")]
    Lock(#[from] crate::file_lock::FileLockError),
    #[error("Temp file persist error: {0}")]
    Persist(#[from] tempfile::PersistError),
}

/// Atomic JSON file with read-modify-write support.
///
/// All mutations go through a lock + temp file + rename pattern
/// to ensure crash-safety.
pub struct AtomicJsonFile {
    path: PathBuf,
    lock_path: PathBuf,
    lock_timeout: Duration,
}

impl AtomicJsonFile {
    /// Create a new `AtomicJsonFile` for the given path.
    ///
    /// The lock file is `{path}.lock`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let lock_path = path.with_extension("json.lock");
        Self {
            path,
            lock_path,
            lock_timeout: Duration::from_secs(5),
        }
    }

    /// Set the lock timeout (default: 5 seconds).
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// Read and deserialize the file. Returns `None` if file doesn't exist.
    pub fn read<T: DeserializeOwned>(&self) -> Result<Option<T>, AtomicJsonError> {
        match fs::read_to_string(&self.path) {
            Ok(content) => {
                let value = serde_json::from_str(&content).map_err(|e| AtomicJsonError::Parse {
                    path: self.path.clone(),
                    source: e,
                })?;
                Ok(Some(value))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AtomicJsonError::Io {
                path: self.path.clone(),
                source: e,
            }),
        }
    }

    /// Read with a default value if file doesn't exist.
    pub fn read_or_default<T: DeserializeOwned + Default>(&self) -> Result<T, AtomicJsonError> {
        self.read().map(|opt| opt.unwrap_or_default())
    }

    /// Write a value atomically (temp file + rename).
    pub fn write<T: Serialize>(&self, value: &T) -> Result<(), AtomicJsonError> {
        let _lock = FileLock::exclusive(&self.lock_path, self.lock_timeout)?;

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| AtomicJsonError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        let dir = self.path.parent().unwrap_or(Path::new("."));
        let temp = NamedTempFile::new_in(dir).map_err(|e| AtomicJsonError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        serde_json::to_writer_pretty(temp.as_file(), value)?;
        temp.as_file().flush().map_err(|e| AtomicJsonError::Io {
            path: self.path.clone(),
            source: e,
        })?;
        temp.as_file().sync_all().map_err(|e| AtomicJsonError::Io {
            path: self.path.clone(),
            source: e,
        })?;
        temp.persist(&self.path)?;
        Ok(())
    }

    /// Read-modify-write with exclusive lock.
    ///
    /// If the file doesn't exist, starts with the default value.
    pub fn update<T, F>(&self, f: F) -> Result<T, AtomicJsonError>
    where
        T: Serialize + DeserializeOwned + Default + Clone,
        F: FnOnce(&mut T),
    {
        let _lock = FileLock::exclusive(&self.lock_path, self.lock_timeout)?;

        let mut data: T = match fs::read_to_string(&self.path) {
            Ok(content) => serde_json::from_str(&content).map_err(|e| AtomicJsonError::Parse {
                path: self.path.clone(),
                source: e,
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => T::default(),
            Err(e) => {
                return Err(AtomicJsonError::Io {
                    path: self.path.clone(),
                    source: e,
                });
            }
        };

        f(&mut data);

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| AtomicJsonError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        let dir = self.path.parent().unwrap_or(Path::new("."));
        let temp = NamedTempFile::new_in(dir).map_err(|e| AtomicJsonError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        serde_json::to_writer_pretty(temp.as_file(), &data)?;
        temp.as_file().flush().map_err(|e| AtomicJsonError::Io {
            path: self.path.clone(),
            source: e,
        })?;
        temp.as_file().sync_all().map_err(|e| AtomicJsonError::Io {
            path: self.path.clone(),
            source: e,
        })?;
        temp.persist(&self.path)?;
        Ok(data)
    }

    /// Return the path to the underlying file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
    struct TestData {
        count: u32,
        name: String,
    }

    #[test]
    fn read_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let file = AtomicJsonFile::new(dir.path().join("nonexistent.json"));
        let result: Option<TestData> = file.read().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file = AtomicJsonFile::new(dir.path().join("test.json"));
        let data = TestData {
            count: 42,
            name: "test".to_string(),
        };
        file.write(&data).unwrap();
        let read: TestData = file.read().unwrap().unwrap();
        assert_eq!(read, data);
    }

    #[test]
    fn update_creates_file_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let file = AtomicJsonFile::new(dir.path().join("update.json"));
        let result: TestData = file
            .update(|d: &mut TestData| {
                d.count = 1;
                d.name = "created".to_string();
            })
            .unwrap();
        assert_eq!(result.count, 1);
        assert_eq!(result.name, "created");
    }

    #[test]
    fn update_modifies_existing() {
        let dir = tempfile::tempdir().unwrap();
        let file = AtomicJsonFile::new(dir.path().join("modify.json"));
        let data = TestData {
            count: 10,
            name: "original".to_string(),
        };
        file.write(&data).unwrap();
        let result: TestData = file.update(|d: &mut TestData| d.count += 5).unwrap();
        assert_eq!(result.count, 15);
        assert_eq!(result.name, "original");
    }
}
