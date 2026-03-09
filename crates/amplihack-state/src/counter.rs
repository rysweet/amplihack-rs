//! Atomic counter files for power steering and lock mode.
//!
//! Each counter is a file containing a single integer. Updates use
//! `AtomicJsonFile` for crash-safety.

use crate::atomic_json::AtomicJsonFile;
use std::path::{Path, PathBuf};

/// Errors from counter operations.
#[derive(Debug, thiserror::Error)]
pub enum CounterError {
    #[error("Counter error: {0}")]
    Storage(#[from] crate::atomic_json::AtomicJsonError),
}

/// An atomic counter backed by a JSON file.
///
/// The file stores `{"value": N}`. Operations are serialized via file lock.
pub struct AtomicCounter {
    file: AtomicJsonFile,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct CounterData {
    value: u64,
}

impl AtomicCounter {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            file: AtomicJsonFile::new(path),
        }
    }

    /// Read the current counter value. Returns 0 if file doesn't exist.
    pub fn get(&self) -> Result<u64, CounterError> {
        let data: CounterData = self.file.read_or_default()?;
        Ok(data.value)
    }

    /// Increment by 1 and return the new value.
    pub fn increment(&self) -> Result<u64, CounterError> {
        let data = self.file.update(|d: &mut CounterData| {
            d.value += 1;
        })?;
        Ok(data.value)
    }

    /// Reset the counter to 0.
    pub fn reset(&self) -> Result<(), CounterError> {
        self.file.write(&CounterData { value: 0 })?;
        Ok(())
    }

    /// Return the path to the counter file.
    pub fn path(&self) -> &Path {
        self.file.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let counter = AtomicCounter::new(dir.path().join("counter.json"));
        assert_eq!(counter.get().unwrap(), 0);
        assert_eq!(counter.increment().unwrap(), 1);
        assert_eq!(counter.increment().unwrap(), 2);
        assert_eq!(counter.get().unwrap(), 2);
        counter.reset().unwrap();
        assert_eq!(counter.get().unwrap(), 0);
    }
}
