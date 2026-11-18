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
    #[error("JSON serialize error on {path}: {source}")]
    Serialize {
        path: PathBuf,
        source: serde_json::Error,
    },
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
    #[must_use]
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
    #[must_use]
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

    /// Ensure the directory holding the target file (and its sibling lock file)
    /// exists. Must run before acquiring the lock, since opening the lock file
    /// would otherwise fail with `NotFound` on a fresh nested path.
    fn ensure_parent_dir(&self) -> Result<(), AtomicJsonError> {
        let parent = match self.path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => return Ok(()),
        };
        fs::create_dir_all(parent).map_err(|e| AtomicJsonError::Io {
            path: parent.to_path_buf(),
            source: e,
        })
    }
    /// Write a value atomically (temp file + rename).
    pub fn write<T: Serialize>(&self, value: &T) -> Result<(), AtomicJsonError> {
        // Create the parent directory *before* acquiring the lock: the lock
        // file lives in the same directory, so `FileLock::exclusive` (which
        // opens `{path}.lock` with `create(true)`) fails with `NotFound` if the
        // directory does not yet exist. This matters for first writes to a
        // fresh nested path (e.g. per-session Signal state/inbox dirs).
        self.ensure_parent_dir()?;

        let _lock = FileLock::exclusive(&self.lock_path, self.lock_timeout)?;

        let dir = self.path.parent().unwrap_or(Path::new("."));
        let temp = NamedTempFile::new_in(dir).map_err(|e| AtomicJsonError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        serde_json::to_writer_pretty(temp.as_file(), value).map_err(|e| {
            AtomicJsonError::Serialize {
                path: self.path.clone(),
                source: e,
            }
        })?;
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
        // See `write`: the parent directory must exist before we open the lock
        // file, otherwise the first write to a fresh nested path fails.
        self.ensure_parent_dir()?;

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

        let dir = self.path.parent().unwrap_or(Path::new("."));
        let temp = NamedTempFile::new_in(dir).map_err(|e| AtomicJsonError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        serde_json::to_writer_pretty(temp.as_file(), &data).map_err(|e| {
            AtomicJsonError::Serialize {
                path: self.path.clone(),
                source: e,
            }
        })?;
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
    fn write_creates_missing_nested_dirs() {
        // Regression: the lock file lives in the target's directory, so a first
        // write to a not-yet-existing nested path must create the directory
        // before opening the lock (otherwise `NotFound`). This is exactly the
        // per-session Signal state/inbox path shape (`<root>/<session>/x.json`).
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("state.json");
        let file = AtomicJsonFile::new(&nested);
        let data = TestData {
            count: 7,
            name: "nested".to_string(),
        };
        file.write(&data).unwrap();
        assert!(nested.exists());
        let read: TestData = file.read().unwrap().unwrap();
        assert_eq!(read, data);
    }

    #[test]
    fn update_creates_missing_nested_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("x").join("y").join("inbox.json");
        let file = AtomicJsonFile::new(&nested);
        let result: TestData = file
            .update(|d: &mut TestData| {
                d.count = 3;
                d.name = "deep".to_string();
            })
            .unwrap();
        assert!(nested.exists());
        assert_eq!(result.count, 3);
        assert_eq!(result.name, "deep");
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

    #[test]
    fn read_or_default_returns_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let file = AtomicJsonFile::new(dir.path().join("default.json"));
        let result: TestData = file.read_or_default().unwrap();
        assert_eq!(result, TestData::default());
    }

    #[test]
    fn parse_error_includes_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not valid json").unwrap();
        let file = AtomicJsonFile::new(path.clone());
        let err = file.read::<TestData>().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("bad.json"),
            "parse error should include file path, got: {msg}"
        );
    }

    #[test]
    fn path_accessor_returns_configured_path() {
        let dir = tempfile::tempdir().unwrap();
        let expected = dir.path().join("accessor.json");
        let file = AtomicJsonFile::new(expected.clone());
        assert_eq!(file.path(), expected);
    }

    #[cfg(unix)]
    #[test]
    fn private_write_and_update_create_private_parents_documents_and_locks() {
        use crate::permission_tests::{mode, under_umask_0002};
        under_umask_0002(
            "atomic_json::tests::private_write_and_update_create_private_parents_documents_and_locks",
            || {
                let dir = tempfile::tempdir().unwrap();
                for operation in ["write", "update"] {
                    let parent = dir.path().join(operation).join("nested");
                    let path = parent.join("state.json");
                    let file = AtomicJsonFile::new_private(&path);
                    if operation == "write" {
                        file.write(&TestData {
                            count: 1,
                            name: "private".into(),
                        })
                        .unwrap();
                    } else {
                        file.update::<TestData, _>(|data| data.count += 1).unwrap();
                    }
                    assert_eq!(mode(&dir.path().join(operation)), 0o700);
                    assert_eq!(mode(&parent), 0o700);
                    assert_eq!(mode(&path), 0o600);
                    assert_eq!(mode(&path.with_extension("json.lock")), 0o600);
                    assert_eq!(file.read::<TestData>().unwrap().unwrap().count, 1);
                    assert_eq!(
                        fs::read_dir(&parent).unwrap().count(),
                        2,
                        "no publication temp remains"
                    );
                }
            },
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_storage_preserves_existing_parents_and_generic_storage_modes() {
        use crate::permission_tests::{mode, under_umask_0002};
        use std::os::unix::fs::PermissionsExt;
        under_umask_0002(
            "atomic_json::tests::private_storage_preserves_existing_parents_and_generic_storage_modes",
            || {
                let dir = tempfile::tempdir().unwrap();
                let existing = dir.path().join("existing");
                fs::create_dir(&existing).unwrap();
                fs::set_permissions(&existing, fs::Permissions::from_mode(0o755)).unwrap();
                AtomicJsonFile::new_private(existing.join("state.json"))
                    .write(&TestData::default())
                    .unwrap();
                assert_eq!(
                    mode(&existing),
                    0o755,
                    "private construction must not chmod an existing parent"
                );
                let path = dir.path().join("generic/nested/state.json");
                AtomicJsonFile::new(&path)
                    .write(&TestData::default())
                    .unwrap();
                assert_eq!(
                    mode(path.parent().unwrap()),
                    0o775,
                    "generic parent contract changed"
                );
                assert_eq!(
                    mode(&path.with_extension("json.lock")),
                    0o664,
                    "generic lock contract changed"
                );
            },
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_private_update_preserves_the_document_and_cleans_up() {
        use crate::permission_tests::under_umask_0002;
        under_umask_0002(
            "atomic_json::tests::a_failed_private_update_preserves_the_document_and_cleans_up",
            || {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("state.json");
                let file = AtomicJsonFile::new_private(&path);
                file.write(&TestData {
                    count: 9,
                    name: "keep".into(),
                })
                .unwrap();
                let before = fs::read(&path).unwrap();
                let panic = std::panic::catch_unwind(|| {
                    let _ = file.update::<TestData, _>(|_| panic!("abort before publication"));
                });
                assert!(panic.is_err());
                assert_eq!(fs::read(&path).unwrap(), before);
                file.update::<TestData, _>(|data| data.count += 1).unwrap();
                assert_eq!(
                    file.read::<TestData>().unwrap().unwrap().count,
                    10,
                    "a later update must still work after a panic"
                );
                assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
            },
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_update_acquires_the_lock_before_reading_or_calling_the_mutator() {
        use crate::permission_tests::under_umask_0002;
        const NAME: &str = "atomic_json::tests::private_update_acquires_the_lock_before_reading_or_calling_the_mutator";
        const PROBE: &str = "AMPLIHACK_STATE_PRIVATE_UPDATE_PROBE";
        if let Some(path) = std::env::var_os(PROBE) {
            let path = PathBuf::from(path);
            let file = AtomicJsonFile::new_private(&path).with_lock_timeout(Duration::ZERO);
            let result = file.update::<TestData, _>(|data| {
                fs::write(path.with_extension("mutator-called"), b"called").unwrap();
                data.count += 1;
            });
            assert!(
                matches!(
                    result,
                    Err(AtomicJsonError::Lock(
                        crate::file_lock::FileLockError::Timeout { .. }
                    ))
                ),
                "a busy lock must be returned before parsing the intentionally invalid document: {result:?}"
            );
            return;
        }
        under_umask_0002(NAME, || {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("state.json");
            let file = AtomicJsonFile::new_private(&path);
            file.write(&TestData::default()).unwrap();
            let lock =
                FileLock::exclusive_private(&path.with_extension("json.lock"), Duration::ZERO)
                    .unwrap();
            fs::write(&path, b"invalid JSON to detect a read before locking").unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--test-threads=1", "--nocapture"])
                .env(PROBE, &path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!path.with_extension("mutator-called").exists());
            assert_eq!(
                fs::read(&path).unwrap(),
                b"invalid JSON to detect a read before locking"
            );
            drop(lock);
            file.write(&TestData {
                count: 42,
                name: "recovered".into(),
            })
            .unwrap();
            assert_eq!(file.read::<TestData>().unwrap().unwrap().count, 42);
        });
    }
}
