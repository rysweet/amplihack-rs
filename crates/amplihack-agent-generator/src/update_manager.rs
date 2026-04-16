//! Version update management for agent bundles.
//!
//! Checks for upstream framework updates, detects user customizations via
//! SHA-256 checksums, creates backups, and (in preview) applies updates.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{GeneratorError, Result};

/// Information about available updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub available: bool,
    pub current_version: String,
    pub latest_version: String,
    pub changes: Vec<String>,
}

/// Result of an update operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateResult {
    pub success: bool,
    pub updated_files: Vec<String>,
    pub preserved_files: Vec<String>,
    pub conflicts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Manages bundle updates from the upstream framework.
pub struct UpdateManager {
    framework_repo: PathBuf,
}

impl UpdateManager {
    /// Create a new manager pointing at the framework repo.
    pub fn new(framework_repo: PathBuf) -> Self {
        Self { framework_repo }
    }

    /// Check if updates are available for a bundle.
    pub fn check_for_updates(&self, bundle_path: &Path) -> Result<UpdateInfo> {
        let manifest = read_manifest(bundle_path)?;
        let current = manifest["framework"]["version"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        let latest = self
            .get_framework_version()
            .map_err(|e| GeneratorError::PackagingFailed(format!("version detection: {e}")))?;

        let changes = if current != latest {
            self.get_changelog(&current, &latest).unwrap_or_default()
        } else {
            vec![]
        };

        Ok(UpdateInfo {
            available: current != latest,
            current_version: current,
            latest_version: latest,
            changes,
        })
    }

    /// Update a bundle (currently preview-only — always returns an error).
    pub fn update_bundle(
        &self,
        bundle_path: &Path,
        preserve_edits: bool,
        backup: bool,
    ) -> UpdateResult {
        if backup {
            match self.create_backup(bundle_path) {
                Ok(p) => tracing::info!("Created backup: {}", p.display()),
                Err(e) => {
                    return UpdateResult {
                        success: false,
                        updated_files: vec![],
                        preserved_files: vec![],
                        conflicts: vec![],
                        error: Some(format!("Backup failed: {e}")),
                    };
                }
            }
        }

        if preserve_edits && let Ok(custom) = self.detect_customizations(bundle_path) {
            let count = custom.values().filter(|&&v| v).count();
            if count > 0 {
                tracing::info!("Found {count} user-modified file(s)");
            }
        }

        UpdateResult {
            success: false,
            updated_files: vec![],
            preserved_files: vec![],
            conflicts: vec![],
            error: Some(
                "Update functionality is currently in preview mode. \
                 Use --check-only to check for updates."
                    .into(),
            ),
        }
    }

    /// Detect which files have been customized by the user.
    pub fn detect_customizations(&self, bundle_path: &Path) -> Result<HashMap<String, bool>> {
        let manifest = read_manifest(bundle_path)?;
        let checksums: HashMap<String, String> = manifest["file_checksums"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        let modified = detect_modified_files(bundle_path, &checksums);
        Ok(checksums
            .keys()
            .map(|k| (k.clone(), modified.contains(k)))
            .collect())
    }

    // -- internal -----------------------------------------------------------

    fn get_framework_version(&self) -> std::result::Result<String, String> {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.framework_repo)
            .output()
            .map_err(|e| e.to_string())?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into());
        }

        let full = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(full[..full.len().min(12)].to_string())
    }

    fn get_changelog(&self, old: &str, new: &str) -> std::result::Result<Vec<String>, String> {
        let output = Command::new("git")
            .args(["log", &format!("{old}..{new}"), "--oneline"])
            .current_dir(&self.framework_repo)
            .output()
            .map_err(|e| e.to_string())?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .take(10)
            .map(String::from)
            .collect())
    }

    fn create_backup(&self, bundle_path: &Path) -> std::io::Result<PathBuf> {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let name = bundle_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let backup_path = bundle_path
            .parent()
            .unwrap_or(bundle_path)
            .join(format!("{name}.backup.{timestamp}"));

        copy_dir_all(bundle_path, &backup_path)?;
        Ok(backup_path)
    }
}

/// Compute `sha256:<hex>` checksum of a file.
pub fn compute_checksum(path: &Path) -> std::io::Result<String> {
    let data = fs::read(path)?;
    let hash = Sha256::digest(&data);
    Ok(format!("sha256:{hash:x}"))
}

// -- helpers ----------------------------------------------------------------

fn read_manifest(bundle_path: &Path) -> Result<serde_json::Value> {
    let p = bundle_path.join("manifest.json");
    if !p.exists() {
        return Err(GeneratorError::PackagingFailed(format!(
            "Manifest not found: {}",
            p.display()
        )));
    }
    let text =
        fs::read_to_string(&p).map_err(|e| GeneratorError::PackagingFailed(e.to_string()))?;
    serde_json::from_str(&text).map_err(|e| GeneratorError::PackagingFailed(e.to_string()))
}

fn detect_modified_files(
    bundle_path: &Path,
    checksums: &HashMap<String, String>,
) -> HashSet<String> {
    let mut modified = HashSet::new();
    for (rel_path, original) in checksums {
        let full = bundle_path.join(rel_path);
        if !full.exists() {
            continue;
        }
        if let Ok(current) = compute_checksum(&full)
            && current != *original
        {
            modified.insert(rel_path.clone());
        }
    }
    modified
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    // Same-path guard.
    if let (Ok(src_canon), Ok(dst_canon)) = (src.canonicalize(), dst.canonicalize())
        && src_canon == dst_canon
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "source and destination are the same path: {}",
                src_canon.display()
            ),
        ));
    }

    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let dest = dst.join(&file_name);
        if entry.file_type()?.is_dir() {
            if matches!(
                file_name.to_str(),
                Some("__pycache__" | ".pytest_cache" | "node_modules")
            ) {
                continue;
            }
            copy_dir_all(&entry.path(), &dest)?;
        } else {
            if file_name
                .to_str()
                .map(|s| s.ends_with(".pyc") || s.ends_with(".pyo"))
                .unwrap_or(false)
            {
                continue;
            }
            fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_checksum_works() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.txt");
        fs::write(&p, "hello world").unwrap();

        let ck = compute_checksum(&p).unwrap();
        assert!(ck.starts_with("sha256:"));
        assert!(ck.len() > 10);
    }

    #[test]
    fn compute_checksum_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        fs::write(&p, "data").unwrap();

        let c1 = compute_checksum(&p).unwrap();
        let c2 = compute_checksum(&p).unwrap();
        assert_eq!(c1, c2);
    }

    #[test]
    fn detect_modified_empty() {
        let dir = tempfile::tempdir().unwrap();
        let checksums = HashMap::new();
        let modified = detect_modified_files(dir.path(), &checksums);
        assert!(modified.is_empty());
    }

    #[test]
    fn detect_modified_finds_change() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "original").unwrap();
        let original_ck = compute_checksum(&file).unwrap();

        let mut checksums = HashMap::new();
        checksums.insert("a.txt".into(), original_ck);

        // No change
        let modified = detect_modified_files(dir.path(), &checksums);
        assert!(modified.is_empty());

        // Modify file
        fs::write(&file, "changed!").unwrap();
        let modified = detect_modified_files(dir.path(), &checksums);
        assert!(modified.contains("a.txt"));
    }

    #[test]
    fn read_manifest_missing() {
        let dir = tempfile::tempdir().unwrap();
        let res = read_manifest(dir.path());
        assert!(res.is_err());
    }

    #[test]
    fn read_manifest_ok() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("manifest.json");
        fs::write(&manifest, r#"{"framework":{"version":"abc123"}}"#).unwrap();

        let val = read_manifest(dir.path()).unwrap();
        assert_eq!(val["framework"]["version"], "abc123");
    }

    #[test]
    fn update_bundle_preview_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = UpdateManager::new(dir.path().to_path_buf());
        let bundle_dir = tempfile::tempdir().unwrap();
        let result = mgr.update_bundle(bundle_dir.path(), false, false);
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("preview"));
    }

    #[test]
    fn copy_dir_all_works() {
        let src_dir = tempfile::tempdir().unwrap();
        let sub = src_dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(src_dir.path().join("a.txt"), "a").unwrap();
        fs::write(sub.join("b.txt"), "b").unwrap();

        let dst_dir = tempfile::tempdir().unwrap();
        let dst = dst_dir.path().join("copy");
        copy_dir_all(src_dir.path(), &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "a");
        assert_eq!(
            fs::read_to_string(dst.join("sub").join("b.txt")).unwrap(),
            "b"
        );
    }
}
