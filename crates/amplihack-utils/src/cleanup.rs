//! Cleanup registry and handler for tracked temporary paths.
//!
//! Ported from `amplihack/utils/cleanup_handler.py` and
//! `amplihack/utils/cleanup_registry.py`. Provides a file-backed registry of
//! paths created during a session and a handler that safely removes them on
//! exit.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced by cleanup operations.
#[derive(Debug, Error)]
pub enum CleanupError {
    /// An I/O error occurred during cleanup.
    #[error("cleanup I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization failure.
    #[error("cleanup registry JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The registry has reached its capacity limit.
    #[error("cleanup registry full (max {max} paths)")]
    RegistryFull {
        /// Maximum number of paths allowed.
        max: usize,
    },

    /// A path failed security validation.
    #[error("path validation failed for {path}: {reason}")]
    ValidationFailed {
        /// The offending path.
        path: String,
        /// Why validation failed.
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum tracked paths (denial-of-service prevention).
const MAX_TRACKED_PATHS: usize = 10_000;

/// Default name for the on-disk registry file.
const REGISTRY_FILENAME: &str = "amplihack-cleanup-registry.json";

// ---------------------------------------------------------------------------
// On-disk format
// ---------------------------------------------------------------------------

/// Serializable representation of the cleanup registry.
#[derive(Debug, Serialize, Deserialize)]
struct RegistryData {
    session_id: String,
    working_directory: String,
    paths: Vec<String>,
}

// ---------------------------------------------------------------------------
// CleanupRegistry
// ---------------------------------------------------------------------------

/// A file-backed registry that tracks paths created during a session for
/// later cleanup.
///
/// Paths are stored in insertion order and cleaned deepest-first to safely
/// handle nested directories.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::cleanup::CleanupRegistry;
/// use std::path::Path;
///
/// let mut reg = CleanupRegistry::new(Path::new("/session/dir"))?;
/// reg.register(Path::new("/session/dir/temp_file.txt"))?;
/// reg.save()?;
/// # Ok::<(), amplihack_utils::cleanup::CleanupError>(())
/// ```
#[derive(Debug)]
pub struct CleanupRegistry {
    /// Tracked paths in insertion order.
    tracked_paths: Vec<PathBuf>,
    /// Path to the on-disk registry JSON file.
    registry_file: PathBuf,
    /// Working directory used for path validation.
    working_dir: PathBuf,
    /// Session identifier.
    session_id: String,
}

impl CleanupRegistry {
    /// Create a new, empty registry stored in `registry_dir`.
    ///
    /// The session id is derived from the directory name. The registry file
    /// is placed at `<registry_dir>/amplihack-cleanup-registry.json`.
    ///
    /// # Errors
    ///
    /// Returns [`CleanupError::Io`] if the directory cannot be created.
    pub fn new(registry_dir: &Path) -> Result<Self, CleanupError> {
        std::fs::create_dir_all(registry_dir)?;

        let session_id = registry_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "default".to_owned());

        Ok(Self {
            tracked_paths: Vec::new(),
            registry_file: registry_dir.join(REGISTRY_FILENAME),
            working_dir: registry_dir.to_path_buf(),
            session_id,
        })
    }

    /// Register a path for later cleanup.
    ///
    /// The path is resolved to an absolute form. Duplicate paths are silently
    /// ignored.
    ///
    /// # Errors
    ///
    /// Returns [`CleanupError::RegistryFull`] if the registry is at capacity.
    pub fn register(&mut self, path: &Path) -> Result<(), CleanupError> {
        if self.tracked_paths.len() >= MAX_TRACKED_PATHS {
            return Err(CleanupError::RegistryFull {
                max: MAX_TRACKED_PATHS,
            });
        }

        let resolved = normalize_path(path);

        if !self.tracked_paths.contains(&resolved) {
            self.tracked_paths.push(resolved);
        }

        Ok(())
    }

    /// Return a slice of all tracked paths in insertion order.
    pub fn get_tracked_paths(&self) -> &[PathBuf] {
        &self.tracked_paths
    }

    /// Return tracked paths sorted deepest-first for safe deletion.
    pub fn deletion_order(&self) -> Vec<PathBuf> {
        let mut sorted = self.tracked_paths.clone();
        sorted.sort_by(|a, b| {
            let depth_a = a.components().count();
            let depth_b = b.components().count();
            depth_b.cmp(&depth_a)
        });
        sorted
    }

    /// Persist the registry to its JSON file on disk.
    ///
    /// # Errors
    ///
    /// Returns [`CleanupError`] on I/O or serialization failures.
    pub fn save(&self) -> Result<(), CleanupError> {
        let data = RegistryData {
            session_id: self.session_id.clone(),
            working_directory: self.working_dir.display().to_string(),
            paths: self
                .tracked_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
        };

        let json = serde_json::to_string_pretty(&data)?;

        if let Some(parent) = self.registry_file.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&self.registry_file, json)?;

        // Best-effort: restrict permissions on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&self.registry_file, perms);
        }

        Ok(())
    }

    /// Load a previously saved registry from `registry_dir`.
    ///
    /// Returns a fully populated [`CleanupRegistry`] with the paths that were
    /// persisted. If the file does not exist or is malformed, returns an empty
    /// registry.
    ///
    /// # Errors
    ///
    /// Returns [`CleanupError`] only on unexpected I/O errors (not
    /// file-not-found).
    pub fn load(registry_dir: &Path) -> Result<Self, CleanupError> {
        let registry_file = registry_dir.join(REGISTRY_FILENAME);

        if !registry_file.is_file() {
            return Self::new(registry_dir);
        }

        // Validate permissions on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&registry_file)?;
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                tracing::warn!(
                    path = %registry_file.display(),
                    mode = format!("{mode:o}"),
                    "cleanup registry has loose permissions"
                );
            }
        }

        let content = match std::fs::read_to_string(&registry_file) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Self::new(registry_dir);
            }
            Err(e) => return Err(CleanupError::Io(e)),
        };

        let data: RegistryData = match serde_json::from_str(&content) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "malformed cleanup registry — starting fresh");
                return Self::new(registry_dir);
            }
        };

        let tracked_paths: Vec<PathBuf> = data.paths.iter().map(PathBuf::from).collect();

        Ok(Self {
            tracked_paths,
            registry_file,
            working_dir: PathBuf::from(&data.working_directory),
            session_id: data.session_id,
        })
    }

    /// Remove all tracked paths and return the number of items cleaned.
    ///
    /// Paths are removed deepest-first. Symlinks, paths outside the working
    /// directory, and non-existent paths are skipped with a warning.
    ///
    /// # Errors
    ///
    /// Returns [`CleanupError::Io`] only if the registry file itself cannot
    /// be cleaned up. Individual path failures are logged but do not abort
    /// the operation.
    pub fn cleanup_all(&mut self) -> Result<usize, CleanupError> {
        let ordered = self.deletion_order();
        let mut cleaned = 0usize;

        for path in &ordered {
            if !path.exists() {
                continue;
            }

            if let Err(reason) = validate_cleanup_path(path, &self.working_dir) {
                tracing::warn!(path = %path.display(), %reason, "skipping invalid cleanup path");
                continue;
            }

            // Double-check symlink status right before deletion (TOCTOU mitigation).
            if path.is_symlink() {
                tracing::warn!(path = %path.display(), "skipping symlink during cleanup");
                continue;
            }

            let result = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };

            match result {
                Ok(()) => {
                    tracing::debug!(path = %path.display(), "cleaned up");
                    cleaned += 1;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to clean up path");
                }
            }
        }

        // Clean up the registry file itself.
        if self.registry_file.is_file() {
            let _ = std::fs::remove_file(&self.registry_file);
        }

        self.tracked_paths.clear();
        Ok(cleaned)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Validate that a path is safe to delete.
///
/// Rejects symlinks and paths that escape the working directory.
fn validate_cleanup_path(path: &Path, working_dir: &Path) -> Result<(), String> {
    if path.is_symlink() {
        return Err("path is a symlink".to_owned());
    }

    // Check containment: the path must be under the working directory.
    let resolved = match path.canonicalize() {
        Ok(p) => p,
        Err(e) => return Err(format!("canonicalize failed: {e}")),
    };

    let root = match working_dir.canonicalize() {
        Ok(p) => p,
        Err(e) => return Err(format!("working dir canonicalize failed: {e}")),
    };

    if !resolved.starts_with(&root) {
        return Err(format!(
            "path {} escapes working dir {}",
            resolved.display(),
            root.display()
        ));
    }

    Ok(())
}

/// Normalize a path to an absolute form without requiring it to exist.
///
/// Uses `canonicalize()` when the path exists, otherwise falls back to
/// joining with the current directory.
fn normalize_path(path: &Path) -> PathBuf {
    // Do NOT follow symlinks — we need to preserve symlink identity
    // so cleanup_all can detect and skip them.
    if path.is_symlink() {
        return if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        };
    }
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

#[cfg(test)]
#[path = "tests/cleanup_tests.rs"]
mod tests;
