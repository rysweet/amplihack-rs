//! Persistence backends for the PR-ownership lease.
//!
//! [`InMemoryLeaseStore`] backs the contract tests and single-process
//! coordination. [`FileLeaseStore`] backs real sessions with cross-process
//! coordination via `amplihack_state::AtomicJsonFile` (advisory file lock +
//! atomic rename).

use std::cell::Cell;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use amplihack_state::AtomicJsonFile;
use amplihack_state::atomic_json::AtomicJsonError;

use super::{LeaseError, LeaseFile, LeaseKey, LeaseRecord, SCHEMA_VERSION};

/// Persistence abstraction for lease records.
///
/// `compare_and_set` closes the acquire time-of-check / time-of-use race by
/// holding the underlying lock across the read-modify-write.
pub trait LeaseStore {
    /// Load the current record for `key`, or `None` if unheld.
    fn load(&self, key: &LeaseKey) -> Result<Option<LeaseRecord>, LeaseError>;

    /// Unconditionally write `record`.
    fn store(&self, record: &LeaseRecord) -> Result<(), LeaseError>;

    /// Remove any record for `key`.
    fn remove(&self, key: &LeaseKey) -> Result<(), LeaseError>;

    /// Atomically replace the record for `key` only if the current value equals
    /// `expected`. Returns `true` on success, `false` if another writer changed
    /// it first.
    fn compare_and_set(
        &self,
        key: &LeaseKey,
        expected: Option<&LeaseRecord>,
        new: Option<&LeaseRecord>,
    ) -> Result<bool, LeaseError>;
}

// ---------------------------------------------------------------------------
// In-memory store
// ---------------------------------------------------------------------------

/// A `Mutex<HashMap>`-backed store for contract tests and single-process use.
#[derive(Debug, Default)]
pub struct InMemoryLeaseStore {
    map: Mutex<HashMap<LeaseKey, LeaseRecord>>,
}

impl LeaseStore for InMemoryLeaseStore {
    fn load(&self, key: &LeaseKey) -> Result<Option<LeaseRecord>, LeaseError> {
        Ok(self
            .map
            .lock()
            .expect("InMemoryLeaseStore mutex poisoned")
            .get(key)
            .cloned())
    }

    fn store(&self, record: &LeaseRecord) -> Result<(), LeaseError> {
        self.map
            .lock()
            .expect("InMemoryLeaseStore mutex poisoned")
            .insert(record.key.clone(), record.clone());
        Ok(())
    }

    fn remove(&self, key: &LeaseKey) -> Result<(), LeaseError> {
        self.map
            .lock()
            .expect("InMemoryLeaseStore mutex poisoned")
            .remove(key);
        Ok(())
    }

    fn compare_and_set(
        &self,
        key: &LeaseKey,
        expected: Option<&LeaseRecord>,
        new: Option<&LeaseRecord>,
    ) -> Result<bool, LeaseError> {
        let mut map = self.map.lock().expect("InMemoryLeaseStore mutex poisoned");
        let current = map.get(key);
        if current != expected {
            return Ok(false);
        }
        match new {
            Some(record) => {
                map.insert(key.clone(), record.clone());
            }
            None => {
                map.remove(key);
            }
        }
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// File store
// ---------------------------------------------------------------------------

/// An `AtomicJsonFile`-backed store for cross-process coordination.
///
/// Each key maps to one `LeaseFile` under the store directory. The directory is
/// created `0o700` and lease files `0o600`. `compare_and_set` is implemented
/// over `AtomicJsonFile::update()`, whose advisory lock makes the compare and
/// conditional write a single critical section — concurrent OS processes cannot
/// both acquire the same key.
#[derive(Debug, Clone)]
pub struct FileLeaseStore {
    dir: PathBuf,
}

impl FileLeaseStore {
    /// Create a store rooted at `dir`, creating it `0o700` if missing.
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, LeaseError> {
        let dir = dir.into();
        amplihack_utils::secure_files::create_private_directories(&dir)?;
        set_dir_permissions(&dir)?;
        Ok(Self { dir })
    }

    fn atomic_file(&self, key: &LeaseKey) -> AtomicJsonFile {
        AtomicJsonFile::new_private(self.dir.join(key.file_slug()))
    }
}

impl LeaseStore for FileLeaseStore {
    fn load(&self, key: &LeaseKey) -> Result<Option<LeaseRecord>, LeaseError> {
        match self.atomic_file(key).read::<LeaseFile>() {
            Ok(None) => Ok(None),
            Ok(Some(lease_file)) => match lease_file.record {
                Some(record) if record.schema_version != SCHEMA_VERSION => {
                    Err(LeaseError::UnsupportedVersion)
                }
                other => Ok(other),
            },
            Err(AtomicJsonError::Parse { .. }) => Err(LeaseError::Corrupt),
            Err(err) => Err(atomic_to_lease(err)),
        }
    }

    fn store(&self, record: &LeaseRecord) -> Result<(), LeaseError> {
        let file = self.atomic_file(&record.key);
        let lease_file = LeaseFile {
            record: Some(record.clone()),
        };
        file.write(&lease_file).map_err(atomic_to_lease)?;
        set_file_permissions(file.path())?;
        Ok(())
    }

    fn remove(&self, key: &LeaseKey) -> Result<(), LeaseError> {
        let file = self.atomic_file(key);
        let empty = LeaseFile::default();
        file.write(&empty).map_err(atomic_to_lease)?;
        set_file_permissions(file.path())?;
        Ok(())
    }

    fn compare_and_set(
        &self,
        key: &LeaseKey,
        expected: Option<&LeaseRecord>,
        new: Option<&LeaseRecord>,
    ) -> Result<bool, LeaseError> {
        let file = self.atomic_file(key);
        let expected_owned = expected.cloned();
        let new_owned = new.cloned();
        let swapped = Cell::new(false);

        let result = file.update(|lease_file: &mut LeaseFile| {
            if lease_file.record == expected_owned {
                lease_file.record = new_owned.clone();
                swapped.set(true);
            }
        });

        match result {
            Ok(_) => {
                if swapped.get() {
                    set_file_permissions(file.path())?;
                }
                Ok(swapped.get())
            }
            Err(AtomicJsonError::Parse { .. }) => Err(LeaseError::Corrupt),
            Err(err) => Err(atomic_to_lease(err)),
        }
    }
}

/// Map an `AtomicJsonError` to a `LeaseError`, preserving I/O detail.
fn atomic_to_lease(err: AtomicJsonError) -> LeaseError {
    match err {
        AtomicJsonError::Io { source, .. } => LeaseError::Io(source),
        AtomicJsonError::Parse { .. } => LeaseError::Corrupt,
        other => LeaseError::Io(io::Error::other(other.to_string())),
    }
}

#[cfg(unix)]
fn set_dir_permissions(dir: &Path) -> Result<(), LeaseError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_dir_permissions(_dir: &Path) -> Result<(), LeaseError> {
    Ok(())
}

#[cfg(unix)]
fn set_file_permissions(path: &Path) -> Result<(), LeaseError> {
    use std::os::unix::fs::PermissionsExt;
    // The lock sidecar file may not exist here; only the data file is chmodded.
    if path.exists() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path) -> Result<(), LeaseError> {
    Ok(())
}
