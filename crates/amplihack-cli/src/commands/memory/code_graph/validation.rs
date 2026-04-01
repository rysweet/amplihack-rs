//! Security and validation functions for the code-graph subsystem.

use anyhow::{Context, Result, bail};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Validate that `path` is safe to use as a project root or input path.
///
/// Contract:
/// - Canonicalize the path (resolve symlinks / `..` components).
/// - Return `Err` if the resolved path starts with `/proc`, `/sys`, or `/dev`.
/// - Return `Ok(canonical_path)` for all other paths.
///
/// Security note (P2-PATH): callers must use the *returned* canonical path,
/// not the original input, to prevent TOCTOU races.
pub(crate) fn validate_index_path(path: &Path) -> Result<PathBuf> {
    for blocked in [Path::new("/proc"), Path::new("/sys"), Path::new("/dev")] {
        if path.starts_with(blocked) {
            bail!("blocked unsafe path prefix: {}", blocked.display());
        }
    }

    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", path.display()))?;
    for blocked in [Path::new("/proc"), Path::new("/sys"), Path::new("/dev")] {
        if canonical.starts_with(blocked) {
            bail!("blocked unsafe path prefix: {}", blocked.display());
        }
    }
    Ok(canonical)
}

/// Assert that the graph DB directory has restrictive Unix permissions.
///
/// Contract (P1-PERM, Unix only):
/// - The DB *parent* directory must be mode `0o700`.
/// - If the backend created a DB *file* (not a directory), that file must be `0o600`.
/// - On non-Unix platforms this is a no-op (returns `Ok(())`).
///
/// Must be called after the code-graph DB has been initialised so the path
/// exists on disk.
#[cfg_attr(not(unix), allow(unused_variables))]
pub(crate) fn enforce_db_permissions(db_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        if let Some(parent) = db_path.parent()
            && parent.exists()
        {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("failed to secure {}", parent.display()))?;
        }

        if db_path.exists() {
            let mode = if db_path.is_dir() { 0o700 } else { 0o600 };
            fs::set_permissions(db_path, fs::Permissions::from_mode(mode))
                .with_context(|| format!("failed to secure {}", db_path.display()))?;
        }
    }
    Ok(())
}

/// Guard against deserialising a pathologically large `blarify.json`.
///
/// Contract (P2-SIZE):
/// - If the file at `path` is larger than `max_bytes`, return `Err` with a
///   message containing "size" or "large".
/// - If the file does not exist, return `Err` (caller decides how to handle).
/// - If the file is within the limit, return `Ok(())`.
///
/// The production limit is 500 MiB (`500 * 1024 * 1024`).  Tests may pass a
/// smaller limit to exercise the guard without writing 500 MB of data.
pub(crate) fn validate_blarify_json_size(path: &Path, max_bytes: u64) -> Result<()> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.len() > max_bytes {
        bail!(
            "blarify JSON size {} exceeds configured limit {} bytes",
            metadata.len(),
            max_bytes
        );
    }
    Ok(())
}
