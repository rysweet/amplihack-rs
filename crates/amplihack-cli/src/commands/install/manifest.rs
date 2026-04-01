//! Install manifest read/write operations.

use super::paths::staging_claude_dir;
use super::types::InstallManifest;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn manifest_path() -> Result<PathBuf> {
    Ok(staging_claude_dir()?
        .join("install")
        .join("amplihack-manifest.json"))
}

pub(super) fn read_manifest(path: &Path) -> Result<InstallManifest> {
    // TODO(hardening): validate that manifest path entries contain no path-traversal
    // sequences (e.g. "../../../etc") before use; file a follow-up issue for this.
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
