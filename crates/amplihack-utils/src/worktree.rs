//! Git worktree detection and shared runtime directory resolution.
//!
//! Ported from `amplihack/worktree/git_utils.py`.
//!
//! Provides [`get_shared_runtime_dir`] to resolve the shared runtime directory
//! that should be used across a main repository and all its worktrees. In
//! worktrees, the function returns the main repo's runtime directory so that
//! power-steering state is shared.
//!
//! ## Security
//!
//! - `AMPLIHACK_RUNTIME_DIR` overrides the computed path but is validated to
//!   reside within the user's home directory or `/tmp`.
//! - Runtime directories are created with `chmod 0o700` (owner-only).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use thiserror::Error;

/// Errors that can occur during worktree resolution.
#[derive(Debug, Error)]
pub enum WorktreeError {
    /// `AMPLIHACK_RUNTIME_DIR` points outside allowed roots.
    #[error(
        "AMPLIHACK_RUNTIME_DIR={raw:?} resolves to {resolved}, \
         which is outside allowed roots ({home} or /tmp). \
         Set AMPLIHACK_RUNTIME_DIR to a path within your home directory or /tmp."
    )]
    InvalidRuntimeDir {
        /// The raw value of the environment variable.
        raw: String,
        /// The resolved (canonicalized) value.
        resolved: String,
        /// The user's home directory.
        home: String,
    },

    /// I/O error creating the runtime directory.
    #[error("failed to create runtime directory {path}: {source}")]
    CreateDir {
        /// The target directory path.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// Permission setting failed.
    #[error("failed to set permissions on {path}: {source}")]
    SetPermissions {
        /// The target path.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },
}

/// Simple LRU-style cache for resolved runtime directories.
///
/// Keyed by canonicalized project root, stores the resolved runtime path.
static CACHE: Mutex<Option<Vec<(PathBuf, String)>>> = Mutex::new(None);

const CACHE_MAX: usize = 128;

/// Return the cached result for `project_root`, if any.
fn cache_get(project_root: &Path) -> Option<String> {
    let guard = CACHE.lock().ok()?;
    let entries = guard.as_ref()?;
    entries
        .iter()
        .find(|(k, _)| k == project_root)
        .map(|(_, v)| v.clone())
}

/// Insert a result into the cache.
fn cache_insert(project_root: PathBuf, value: String) {
    if let Ok(mut guard) = CACHE.lock() {
        let entries = guard.get_or_insert_with(Vec::new);
        // Evict oldest if full.
        if entries.len() >= CACHE_MAX {
            entries.remove(0);
        }
        entries.push((project_root, value));
    }
}

/// Get the shared runtime directory for power-steering state.
///
/// In git worktrees, power-steering state should be shared with the main repo
/// to ensure consistent behavior across all worktrees.
///
/// # Algorithm
///
/// 1. If `AMPLIHACK_RUNTIME_DIR` is set, validate and return it.
/// 2. Run `git rev-parse --git-common-dir` to detect worktree.
/// 3. If in worktree, resolve main repo and return `main_repo/.claude/runtime`.
/// 4. Otherwise return `project_root/.claude/runtime`.
/// 5. Create the resolved directory with `chmod 0o700`.
///
/// # Errors
///
/// Returns [`WorktreeError::InvalidRuntimeDir`] when `AMPLIHACK_RUNTIME_DIR`
/// is set to a path outside the user's home directory or `/tmp`.
///
/// Returns [`WorktreeError::CreateDir`] or [`WorktreeError::SetPermissions`]
/// if directory creation or permission hardening fails.
///
/// # Fail-Open Behaviour
///
/// If `git` commands fail (not a repo, git not installed, timeout), falls back
/// to `project_root/.claude/runtime` without error.
pub fn get_shared_runtime_dir(project_root: &Path) -> Result<String, WorktreeError> {
    // --- P0: Environment variable override ---
    if let Ok(env_val) = std::env::var("AMPLIHACK_RUNTIME_DIR")
        && !env_val.is_empty()
    {
        let env_path = PathBuf::from(&env_val);
        validate_env_runtime_dir(&env_path)?;
        return Ok(env_val);
    }

    // Canonicalize project root (best-effort).
    let project_path =
        std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());

    // --- Cache lookup ---
    if let Some(cached) = cache_get(&project_path) {
        return Ok(cached);
    }

    let default_runtime = project_path.join(".claude").join("runtime");

    let runtime_path = resolve_runtime_path(&project_path, &default_runtime);

    // --- Create directory with owner-only permissions ---
    create_runtime_dir_secure(&runtime_path)?;

    let result = runtime_path.to_string_lossy().into_owned();
    cache_insert(project_path, result.clone());
    Ok(result)
}

/// Resolve the runtime path by probing `git rev-parse --git-common-dir`.
///
/// Returns the default path on any git failure (fail-open).
fn resolve_runtime_path(project_path: &Path, default: &Path) -> PathBuf {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(project_path)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(error = %e, "git worktree detection failed");
            return default.to_path_buf();
        }
    };

    if !output.status.success() {
        return default.to_path_buf();
    }

    let git_common_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if git_common_dir.is_empty() {
        return default.to_path_buf();
    }

    let git_common_path = PathBuf::from(&git_common_dir);

    // Make absolute if relative.
    let git_common_path = if git_common_path.is_absolute() {
        git_common_path
    } else {
        project_path.join(&git_common_path)
    };

    let git_common_resolved =
        std::fs::canonicalize(&git_common_path).unwrap_or_else(|_| git_common_path.clone());

    let expected_main_git = project_path.join(".git");
    let expected_resolved = std::fs::canonicalize(&expected_main_git).unwrap_or(expected_main_git);

    if git_common_resolved != expected_resolved {
        // We are in a worktree — find main repo root.
        let main_repo_root = if git_common_resolved.file_name().is_some_and(|n| n == ".git") {
            git_common_resolved.parent().unwrap_or(&git_common_resolved)
        } else {
            &git_common_resolved
        };
        main_repo_root.join(".claude").join("runtime")
    } else {
        default.to_path_buf()
    }
}

/// Validate that `AMPLIHACK_RUNTIME_DIR` is within an allowed root.
///
/// Allowed roots are the user's home directory and `/tmp`. Path traversal
/// sequences are resolved before the check.
fn validate_env_runtime_dir(path: &Path) -> Result<(), WorktreeError> {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let home = dirs_home().unwrap_or_else(|| PathBuf::from("/nonexistent"));
    let home_resolved = std::fs::canonicalize(&home).unwrap_or_else(|_| home.clone());
    let tmp_resolved = std::fs::canonicalize("/tmp").unwrap_or_else(|_| PathBuf::from("/tmp"));

    if resolved.starts_with(&home_resolved) || resolved.starts_with(&tmp_resolved) {
        return Ok(());
    }

    Err(WorktreeError::InvalidRuntimeDir {
        raw: path.to_string_lossy().into_owned(),
        resolved: resolved.to_string_lossy().into_owned(),
        home: home_resolved.to_string_lossy().into_owned(),
    })
}

/// Return the user's home directory.
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Create the runtime directory with owner-only (0o700) permissions.
fn create_runtime_dir_secure(path: &Path) -> Result<(), WorktreeError> {
    std::fs::create_dir_all(path).map_err(|e| WorktreeError::CreateDir {
        path: path.display().to_string(),
        source: e,
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(path, perms).map_err(|e| WorktreeError::SetPermissions {
            path: path.display().to_string(),
            source: e,
        })?;

        // Harden parent (.claude) — remove world-writable bit.
        if let Some(parent) = path.parent()
            && parent.exists()
            && let Ok(meta) = parent.metadata()
        {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o002 != 0 {
                let _ = std::fs::set_permissions(
                    parent,
                    std::fs::Permissions::from_mode(mode & !0o002),
                );
            }
        }
    }

    Ok(())
}

/// Clear the internal cache. Primarily for testing.
pub fn clear_cache() {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = None;
    }
}

#[cfg(test)]
#[path = "tests/worktree_tests.rs"]
mod tests;
