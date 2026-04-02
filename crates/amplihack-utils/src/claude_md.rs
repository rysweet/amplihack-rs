//! `CLAUDE.md` preservation and version management.
//!
//! Ported from `amplihack/utils/claude_md_preserver.py`. Handles deployment
//! and preservation of amplihack's `CLAUDE.md` with smart version detection,
//! backup logic, and content hashing.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced by CLAUDE.md handling operations.
#[derive(Debug, Error)]
pub enum ClaudeMdError {
    /// An I/O error occurred during file operations.
    #[error("claude_md I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The source `CLAUDE.md` file could not be found.
    #[error("source CLAUDE.md not found: {path}")]
    SourceNotFound {
        /// Path to the missing source.
        path: String,
    },
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Observed state of the `CLAUDE.md` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeState {
    /// No `CLAUDE.md` file exists in the target directory.
    Missing,
    /// The file contains the current amplihack version marker.
    Default,
    /// The file contains user-written content (no amplihack marker).
    CustomClean,
    /// The file is an older amplihack version.
    CustomDirty,
    /// The file state could not be determined (e.g. unreadable).
    Unknown,
}

/// Desired handling behavior for `CLAUDE.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleMode {
    /// Preserve custom content by backing it up before overwriting.
    Preserve,
    /// Overwrite the file unconditionally.
    Overwrite,
    /// Merge amplihack content with existing user content.
    Merge,
}

/// Action that was actually taken during handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTaken {
    /// Deployed a fresh `CLAUDE.md` (was missing).
    Deployed,
    /// Backed up existing content and replaced the file.
    BackedUpAndReplaced,
    /// No write was performed.
    Skipped,
    /// State was checked but no action was taken.
    CheckOnly,
}

// ---------------------------------------------------------------------------
// Result struct
// ---------------------------------------------------------------------------

/// Outcome of a [`handle_claude_md`] call.
#[derive(Debug, Clone)]
pub struct ClaudeHandlerResult {
    /// What action was performed.
    pub action: HandleMode,
    /// SHA-256 hash of the written or existing content, if available.
    pub content_hash: Option<String>,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Version marker prefix embedded in amplihack-managed `CLAUDE.md` files.
const CLAUDE_VERSION_MARKER: &str = "<!-- amplihack-version:";

/// Current version of the amplihack `CLAUDE.md` template.
const CURRENT_VERSION: &str = "0.9.0";

/// Start marker for preserved content sections inside `PROJECT.md`.
const BEGIN_MARKER: &str = "<!-- BEGIN AMPLIHACK-PRESERVED-CONTENT";

/// End marker for preserved content sections inside `PROJECT.md`.
const END_MARKER: &str = "<!-- END AMPLIHACK-PRESERVED-CONTENT -->";

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute a SHA-256 content hash, normalizing whitespace.
///
/// Trailing spaces on each line are stripped, and leading/trailing blank
/// lines are removed before hashing. This ensures minor formatting changes
/// do not alter the hash.
///
/// # Examples
///
/// ```
/// use amplihack_utils::claude_md::compute_content_hash;
///
/// let h1 = compute_content_hash("hello\nworld\n");
/// let h2 = compute_content_hash("hello  \nworld  \n\n");
/// assert_eq!(h1, h2);
/// ```
pub fn compute_content_hash(content: &str) -> String {
    let mut lines: Vec<&str> = content.lines().map(|l| l.trim_end()).collect();

    // Strip leading blank lines.
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }

    // Strip trailing blank lines.
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }

    let normalized = lines.join("\n");
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Detect the state of `CLAUDE.md` in `project_dir`.
///
/// Reads `CLAUDE.md` at the root of `project_dir` and classifies it based on
/// the presence and version of the amplihack marker.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::claude_md::detect_claude_state;
/// use std::path::Path;
///
/// let state = detect_claude_state(Path::new("/my/project"));
/// ```
pub fn detect_claude_state(project_dir: &Path) -> ClaudeState {
    let md_path = project_dir.join("CLAUDE.md");

    if !md_path.exists() {
        return ClaudeState::Missing;
    }

    // Reject symlinks as a security precaution.
    if md_path.is_symlink() {
        tracing::warn!(path = %md_path.display(), "CLAUDE.md is a symlink — treating as custom");
        return ClaudeState::CustomClean;
    }

    let content = match std::fs::read_to_string(&md_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %md_path.display(), error = %e, "could not read CLAUDE.md");
            return ClaudeState::Unknown;
        }
    };

    parse_claude_state(&content)
}

/// Handle `CLAUDE.md` deployment/preservation in `target_dir`.
///
/// `source_claude` points to the canonical amplihack `CLAUDE.md` to deploy.
///
/// # Errors
///
/// Returns [`ClaudeMdError`] on I/O failures or if the source file is missing.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::claude_md::{handle_claude_md, HandleMode};
/// use std::path::Path;
///
/// let result = handle_claude_md(
///     Path::new("/amplihack/CLAUDE.md"),
///     Path::new("/my/project"),
///     HandleMode::Preserve,
/// )?;
/// # Ok::<(), amplihack_utils::claude_md::ClaudeMdError>(())
/// ```
pub fn handle_claude_md(
    source_claude: &Path,
    target_dir: &Path,
    mode: HandleMode,
) -> Result<ClaudeHandlerResult, ClaudeMdError> {
    let state = detect_claude_state(target_dir);
    let target_path = target_dir.join("CLAUDE.md");

    match mode {
        HandleMode::Overwrite => {
            let source_content = read_source(source_claude)?;
            let hash = compute_content_hash(&source_content);
            write_claude_md(&target_path, &source_content)?;
            Ok(ClaudeHandlerResult {
                action: HandleMode::Overwrite,
                content_hash: Some(hash),
            })
        }
        HandleMode::Preserve => match state {
            ClaudeState::Missing => {
                let source_content = read_source(source_claude)?;
                let hash = compute_content_hash(&source_content);
                write_claude_md(&target_path, &source_content)?;
                Ok(ClaudeHandlerResult {
                    action: HandleMode::Preserve,
                    content_hash: Some(hash),
                })
            }
            ClaudeState::Default => {
                // Already current — nothing to do.
                let content = std::fs::read_to_string(&target_path)?;
                Ok(ClaudeHandlerResult {
                    action: HandleMode::Preserve,
                    content_hash: Some(compute_content_hash(&content)),
                })
            }
            ClaudeState::CustomDirty => {
                // Outdated amplihack version — safe to replace.
                let source_content = read_source(source_claude)?;
                let hash = compute_content_hash(&source_content);
                write_claude_md(&target_path, &source_content)?;
                Ok(ClaudeHandlerResult {
                    action: HandleMode::Preserve,
                    content_hash: Some(hash),
                })
            }
            ClaudeState::CustomClean | ClaudeState::Unknown => {
                // User content — back up before replacing.
                let existing = std::fs::read_to_string(&target_path).unwrap_or_default();
                backup_to_preserved(target_dir, &existing)?;
                backup_to_project_md(target_dir, &existing)?;

                let source_content = read_source(source_claude)?;
                let hash = compute_content_hash(&source_content);
                write_claude_md(&target_path, &source_content)?;
                Ok(ClaudeHandlerResult {
                    action: HandleMode::Preserve,
                    content_hash: Some(hash),
                })
            }
        },
        HandleMode::Merge => {
            // Merge mode: append amplihack content below existing content.
            let source_content = read_source(source_claude)?;
            let existing = if target_path.is_file() {
                std::fs::read_to_string(&target_path).unwrap_or_default()
            } else {
                String::new()
            };

            let merged = if existing.is_empty() {
                source_content.clone()
            } else {
                format!("{existing}\n\n---\n\n{source_content}")
            };

            let hash = compute_content_hash(&merged);
            write_claude_md(&target_path, &merged)?;
            Ok(ClaudeHandlerResult {
                action: HandleMode::Merge,
                content_hash: Some(hash),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse the content of a `CLAUDE.md` file and return its state.
fn parse_claude_state(content: &str) -> ClaudeState {
    // Look for the version marker.
    let Some(marker_pos) = content.find(CLAUDE_VERSION_MARKER) else {
        return ClaudeState::CustomClean;
    };

    let after_marker = &content[marker_pos + CLAUDE_VERSION_MARKER.len()..];
    let version_end = match after_marker.find("-->") {
        Some(pos) => pos,
        None => return ClaudeState::CustomClean,
    };

    let version = after_marker[..version_end].trim();

    if version == CURRENT_VERSION {
        ClaudeState::Default
    } else {
        ClaudeState::CustomDirty
    }
}

/// Read the source `CLAUDE.md` template file.
fn read_source(source_claude: &Path) -> Result<String, ClaudeMdError> {
    if !source_claude.is_file() {
        return Err(ClaudeMdError::SourceNotFound {
            path: source_claude.display().to_string(),
        });
    }
    Ok(std::fs::read_to_string(source_claude)?)
}

/// Write content to a `CLAUDE.md` path, creating parent directories.
fn write_claude_md(target: &Path, content: &str) -> Result<(), ClaudeMdError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, content)?;
    Ok(())
}

/// Create a `.preserved` backup of the existing `CLAUDE.md`.
fn backup_to_preserved(target_dir: &Path, content: &str) -> Result<PathBuf, ClaudeMdError> {
    let preserved_dir = target_dir.join(".claude").join("context");
    std::fs::create_dir_all(&preserved_dir)?;

    let preserved_path = preserved_dir.join("CLAUDE.md.preserved");
    let timestamp = chrono::Utc::now().to_rfc3339();
    let header = format!(
        "# Preserved CLAUDE.md content\n\
         # Backed up: {timestamp}\n\
         # This file was created by amplihack to preserve your custom CLAUDE.md\n\n"
    );
    let full = format!("{header}{content}");
    std::fs::write(&preserved_path, full)?;

    tracing::info!(path = %preserved_path.display(), "backed up CLAUDE.md to preserved file");
    Ok(preserved_path)
}

/// Append preserved content as a marked section in `PROJECT.md`.
fn backup_to_project_md(target_dir: &Path, content: &str) -> Result<PathBuf, ClaudeMdError> {
    let project_md = target_dir
        .join(".claude")
        .join("context")
        .join("PROJECT.md");
    std::fs::create_dir_all(project_md.parent().expect("PROJECT.md has a parent dir"))?;

    let timestamp = chrono::Utc::now().to_rfc3339();

    if project_md.is_file() {
        let existing = std::fs::read_to_string(&project_md)?;
        // Idempotent: do not duplicate if already preserved.
        if existing.contains(BEGIN_MARKER) {
            tracing::debug!("PROJECT.md already contains preserved section — skipping");
            return Ok(project_md);
        }
        let section = format!(
            "\n\n---\n{BEGIN_MARKER} {timestamp} -->\n\
             Preserved CLAUDE.md content:\n\n\
             {content}\n\
             {END_MARKER}\n---\n"
        );
        std::fs::write(&project_md, format!("{existing}{section}"))?;
    } else {
        let full = format!(
            "# Project Context\n\n\
             {BEGIN_MARKER} {timestamp} -->\n\
             Preserved CLAUDE.md content:\n\n\
             {content}\n\
             {END_MARKER}\n"
        );
        std::fs::write(&project_md, full)?;
    }

    tracing::info!(path = %project_md.display(), "preserved CLAUDE.md content in PROJECT.md");
    Ok(project_md)
}

#[cfg(test)]
#[path = "tests/claude_md_tests.rs"]
mod tests;
