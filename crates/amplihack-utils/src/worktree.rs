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
//!   reside outside every registered checkout and Git metadata.
//! - Runtime directories are created with `chmod 0o700` (owner-only).

use std::path::Path;

use thiserror::Error;

/// Errors that can occur during worktree resolution.
#[derive(Debug, Error)]
pub enum WorktreeError {
    /// I/O error creating the runtime directory.
    #[error("failed to create runtime directory {path}: {source}")]
    CreateDir {
        /// The target directory path.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },
}

/// Prepare identity-bound external runtime state. Non-repositories and
/// checkout-contained overrides are rejected before any directory is created.
pub fn get_shared_runtime_dir(project_root: &Path) -> Result<String, WorktreeError> {
    let state = amplihack_git::launcher_state::prepare(project_root).map_err(|e| {
        WorktreeError::CreateDir {
            path: project_root.display().to_string(),
            source: std::io::Error::other(e.to_string()),
        }
    })?;
    Ok(state.shared_directory.to_string_lossy().into_owned())
}

/// Compatibility entry point: resolution no longer caches paths and already
/// observes the current checkout and environment on every call.
pub fn clear_cache() {}

#[cfg(test)]
#[path = "tests/worktree_tests.rs"]
mod tests;
