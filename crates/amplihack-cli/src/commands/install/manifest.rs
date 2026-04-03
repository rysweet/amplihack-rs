//! Install manifest read/write operations.

use super::paths::staging_claude_dir;
use super::types::InstallManifest;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(super) fn manifest_path() -> Result<PathBuf> {
    Ok(staging_claude_dir()?
        .join("install")
        .join("amplihack-manifest.json"))
}

/// Validate that a relative path entry contains no path-traversal sequences.
///
/// Rejects entries that:
/// - Are absolute paths
/// - Contain `..` components
/// - Contain non-normal components after normalization
fn validate_manifest_entry(entry: &str) -> Result<()> {
    let path = Path::new(entry);

    if path.is_absolute() {
        bail!("manifest entry is an absolute path (potential path traversal): {entry}");
    }

    for component in path.components() {
        match component {
            Component::ParentDir => {
                bail!("manifest entry contains '..' (potential path traversal): {entry}");
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!(
                    "manifest entry contains root/prefix component (potential path traversal): {entry}"
                );
            }
            Component::Normal(_) | Component::CurDir => {}
        }
    }

    Ok(())
}

/// Validate all path entries in a manifest, returning an error if any contain
/// path-traversal sequences.
fn validate_manifest_paths(manifest: &InstallManifest) -> Result<()> {
    for entry in &manifest.files {
        validate_manifest_entry(entry)
            .with_context(|| format!("invalid file entry in manifest: {entry}"))?;
    }
    for entry in &manifest.dirs {
        validate_manifest_entry(entry)
            .with_context(|| format!("invalid dir entry in manifest: {entry}"))?;
    }
    Ok(())
}

pub(super) fn read_manifest(path: &Path) -> Result<InstallManifest> {
    if !path.exists() {
        return Ok(InstallManifest::default());
    }
    let Ok(raw) = fs::read_to_string(path) else {
        tracing::debug!(
            "could not read manifest at {}: returning empty",
            path.display()
        );
        return Ok(InstallManifest::default());
    };
    // A corrupt manifest is treated as an empty one, triggering a clean reinstall.
    // The inspect_err call surfaces the parse error at debug log level so it is
    // visible in tracing output without failing the caller.
    let manifest = serde_json::from_str::<InstallManifest>(&raw)
        .inspect_err(|e| {
            tracing::debug!(
                "corrupt manifest at {}: {e} — treating as empty",
                path.display()
            )
        })
        .unwrap_or_default();

    validate_manifest_paths(&manifest).with_context(|| {
        format!(
            "manifest at {} contains path-traversal entries",
            path.display()
        )
    })?;

    Ok(manifest)
}

pub(super) fn write_manifest(path: &Path, manifest: &InstallManifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(manifest)?)
        .with_context(|| format!("failed to write {}", path.display()))
}
