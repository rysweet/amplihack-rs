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

#[derive(Debug, Clone, serde::Serialize, Default)]
struct CounterData {
    value: u64,
}

// Custom deserializer: accept both {"value": N} (Rust format) and plain N (Python compat).
impl<'de> serde::Deserialize<'de> for CounterData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct CounterVisitor;

        impl<'de> de::Visitor<'de> for CounterVisitor {
            type Value = CounterData;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(r#"{"value": N} or plain integer N"#)
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<CounterData, E> {
                Ok(CounterData { value: v })
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<CounterData, E> {
                u64::try_from(v)
                    .map(|v| CounterData { value: v })
                    .map_err(|_| de::Error::custom("counter value must be non-negative"))
            }

            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<CounterData, A::Error> {
                let mut value = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "value" {
                        value = Some(map.next_value()?);
                    } else {
                        let _ = map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(CounterData {
                    value: value.unwrap_or(0),
                })
            }
        }

        deserializer.deserialize_any(CounterVisitor)
    }
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

    #[test]
    fn reads_python_plain_integer_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("counter.txt");
        // Python writes plain integer text (no JSON object wrapper).
        std::fs::write(&path, "1").unwrap();
        let counter = AtomicCounter::new(&path);
        assert_eq!(counter.get().unwrap(), 1);
    }

    #[test]
    fn reads_rust_json_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("counter.txt");
        std::fs::write(&path, r#"{"value": 1}"#).unwrap();
        let counter = AtomicCounter::new(&path);
        assert_eq!(counter.get().unwrap(), 1);
    }

    #[test]
    fn rejects_invalid_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("counter.txt");
        std::fs::write(&path, "abc").unwrap();
        let counter = AtomicCounter::new(&path);
        assert!(counter.get().is_err());
    }

    #[test]
    fn increment_normalizes_python_format_to_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("counter.txt");
        // Start with Python format.
        std::fs::write(&path, "5").unwrap();
        let counter = AtomicCounter::new(&path);
        assert_eq!(counter.increment().unwrap(), 6);
        // After increment, file should be valid JSON (Rust format).
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["value"], 6);
    }
}
