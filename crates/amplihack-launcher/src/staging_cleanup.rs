//! Legacy skill directory cleanup.
//!
//! Removes obsolete/legacy skill directories after verifying they are safe
//! to delete via [`crate::staging_safety::is_safe_to_delete`].

use crate::staging_safety::{SafetyStatus, is_safe_to_delete};
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Result of a cleanup operation.
#[derive(Debug, Clone, Default)]
pub struct CleanupResult {
    /// Directories that were successfully removed.
    pub cleaned: Vec<PathBuf>,
    /// Directories that were skipped with a reason.
    pub skipped: Vec<(PathBuf, String)>,
    /// Directories that failed to remove with an error message.
    pub errors: Vec<(PathBuf, String)>,
}

/// Remove legacy skill directories that pass the safety check.
///
/// When `dry_run` is `true`, no directories are deleted — only the
/// classification (cleaned vs skipped) is reported. Pass explicit
/// `legacy_dirs` or `None` to use the default legacy paths.
pub fn cleanup_legacy_skills(dry_run: bool, legacy_dirs: Option<&[PathBuf]>) -> CleanupResult {
    let defaults = default_legacy_dirs();
    let dirs = legacy_dirs.unwrap_or(&defaults);

    let mut result = CleanupResult::default();

    for dir in dirs {
        if !dir.exists() {
            debug!(dir = %dir.display(), "Legacy dir does not exist, skipping");
            result.skipped.push((dir.clone(), "does not exist".into()));
            continue;
        }

        let check = is_safe_to_delete(dir);

        match check.status {
            SafetyStatus::Safe => {
                if dry_run {
                    info!(dir = %dir.display(), "Would remove (dry run)");
                    result.cleaned.push(dir.clone());
                } else {
                    match std::fs::remove_dir_all(dir) {
                        Ok(()) => {
                            info!(dir = %dir.display(), "Removed legacy directory");
                            result.cleaned.push(dir.clone());
                        }
                        Err(e) => {
                            warn!(dir = %dir.display(), error = %e, "Failed to remove directory");
                            result.errors.push((dir.clone(), e.to_string()));
                        }
                    }
                }
            }
            SafetyStatus::Unsafe => {
                debug!(dir = %dir.display(), reason = %check.reason, "Skipping unsafe directory");
                result.skipped.push((dir.clone(), check.reason));
            }
            SafetyStatus::Uncertain => {
                debug!(dir = %dir.display(), reason = %check.reason, "Skipping uncertain directory");
                result.skipped.push((dir.clone(), check.reason));
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Default list of legacy skill directories to consider for cleanup.
fn default_legacy_dirs() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return vec![];
    };

    vec![
        home.join(".copilot").join("skills").join("_legacy"),
        home.join(".amplihack").join("skills").join("_legacy"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cleanup_nonexistent_dirs_skips() {
        let dirs = vec![PathBuf::from("/nonexistent/legacy/dir")];
        let result = cleanup_legacy_skills(false, Some(&dirs));
        assert!(result.cleaned.is_empty());
        assert_eq!(result.skipped.len(), 1);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn cleanup_dry_run_does_not_delete() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy-skill");
        fs::create_dir_all(&legacy).unwrap();
        // Put an "amplihack" subdir so it passes safety
        fs::create_dir_all(legacy.join("amplihack")).unwrap();

        let dirs = vec![legacy.clone()];
        let result = cleanup_legacy_skills(true, Some(&dirs));
        assert_eq!(result.cleaned.len(), 1);
        assert!(legacy.exists(), "dry run must not delete the directory");
    }

    #[test]
    fn cleanup_removes_safe_directory() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("to-delete");
        fs::create_dir_all(&legacy).unwrap();
        // Empty dir → safe

        let dirs = vec![legacy.clone()];
        let result = cleanup_legacy_skills(false, Some(&dirs));
        assert_eq!(result.cleaned.len(), 1);
        assert!(!legacy.exists(), "directory should have been deleted");
    }

    #[test]
    fn cleanup_skips_unsafe_directory() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("git-repo");
        fs::create_dir_all(legacy.join(".git")).unwrap();

        let dirs = vec![legacy.clone()];
        let result = cleanup_legacy_skills(false, Some(&dirs));
        assert!(result.cleaned.is_empty());
        assert_eq!(result.skipped.len(), 1);
        assert!(legacy.exists(), "unsafe directory must not be deleted");
    }

    #[test]
    fn cleanup_result_default() {
        let r = CleanupResult::default();
        assert!(r.cleaned.is_empty());
        assert!(r.skipped.is_empty());
        assert!(r.errors.is_empty());
    }
}
