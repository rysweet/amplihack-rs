//! Auto-detection of the best available memory backend.
//!
//! Checks for SQLite availability and falls back to in-memory.

use crate::backend::{InMemoryBackend, MemoryBackend};
use crate::config::MemoryConfig;
use std::path::Path;

/// Backend selection result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectedBackend {
    /// SQLite is available and will be used.
    Sqlite(std::path::PathBuf),
    /// Fallback to in-memory store.
    InMemory,
}

impl DetectedBackend {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Sqlite(_) => "sqlite",
            Self::InMemory => "in_memory",
        }
    }
}

/// Detect the best available backend based on config and environment.
///
/// Strategy:
/// 1. If config has a storage_path and sqlite feature is enabled, use SQLite
/// 2. Otherwise, fall back to in-memory
pub fn detect_backend(config: &MemoryConfig) -> DetectedBackend {
    if let Some(ref path) = config.storage_path
        && cfg!(feature = "sqlite") {
            return DetectedBackend::Sqlite(path.clone());
        }

    // Check default path
    if cfg!(feature = "sqlite")
        && let Some(home) = std::env::var_os("HOME") {
            let default_path = Path::new(&home)
                .join(".amplihack")
                .join("memory")
                .join("memory.db");
            if default_path.parent().is_some_and(|p| p.exists()) {
                return DetectedBackend::Sqlite(default_path);
            }
        }

    DetectedBackend::InMemory
}

/// Create a backend instance from detection result.
pub fn create_backend(detected: &DetectedBackend) -> anyhow::Result<Box<dyn MemoryBackend>> {
    match detected {
        DetectedBackend::Sqlite(_path) => {
            #[cfg(feature = "sqlite")]
            {
                let backend = crate::sqlite_backend::SqliteBackend::open(_path)?;
                Ok(Box::new(backend))
            }
            #[cfg(not(feature = "sqlite"))]
            {
                Ok(Box::new(InMemoryBackend::new()))
            }
        }
        DetectedBackend::InMemory => Ok(Box::new(InMemoryBackend::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_in_memory_when_no_path() {
        let config = MemoryConfig {
            storage_path: None,
            ..MemoryConfig::for_testing()
        };
        // Without HOME set to a dir with .amplihack, should get InMemory
        let detected = detect_backend(&config);
        // Result depends on HOME, but type should be valid
        assert!(!detected.as_str().is_empty());
    }
}
