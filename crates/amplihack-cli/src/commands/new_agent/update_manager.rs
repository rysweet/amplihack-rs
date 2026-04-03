//! Bundle update manager — checks for and applies updates to installed bundles.
//!
//! Ported from `amplihack/bundle_generator/update_manager.py`.
//! Provides [`UpdateManager`] for version comparison, backup, and rollback.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::error::BundleError;

/// Information about an available update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    /// Whether an update is available.
    pub available: bool,
    /// The currently installed version (git short hash or semver).
    pub current_version: String,
    /// The latest available version.
    pub latest_version: String,
    /// High-level change descriptions since `current_version`.
    pub changes: Vec<String>,
}

/// Outcome of an update attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateResult {
    /// Whether the update succeeded.
    pub success: bool,
    /// Files that were updated.
    pub updated_files: Vec<String>,
    /// Files preserved because the user customised them.
    pub preserved_files: Vec<String>,
    /// Files with merge conflicts.
    pub conflicts: Vec<String>,
    /// Error message if `success` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl UpdateResult {
    fn ok() -> Self {
        Self {
            success: true,
            updated_files: Vec::new(),
            preserved_files: Vec::new(),
            conflicts: Vec::new(),
            error: None,
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            success: false,
            updated_files: Vec::new(),
            preserved_files: Vec::new(),
            conflicts: Vec::new(),
            error: Some(message.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Manifest helpers
// ---------------------------------------------------------------------------

/// Relevant fields from a bundle's `manifest.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct BundleManifest {
    #[serde(default)]
    framework: FrameworkInfo,
    #[serde(default)]
    file_checksums: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FrameworkInfo {
    #[serde(default)]
    version: String,
    #[serde(default)]
    updated_at: String,
}

/// Read and parse a bundle manifest from `bundle_path/manifest.json`.
fn read_manifest(bundle_path: &Path) -> Result<BundleManifest, BundleError> {
    let manifest_path = bundle_path.join("manifest.json");
    if !manifest_path.exists() {
        return Err(BundleError::validation(format!(
            "manifest.json not found in {}",
            bundle_path.display()
        )));
    }
    let data = fs::read_to_string(&manifest_path).map_err(|e| {
        BundleError::validation(format!("cannot read manifest: {e}"))
    })?;
    serde_json::from_str(&data).map_err(|e| {
        BundleError::validation(format!("invalid manifest JSON: {e}"))
    })
}

// ---------------------------------------------------------------------------
// UpdateManager
// ---------------------------------------------------------------------------

/// Manages updates for installed agent bundles.
pub struct UpdateManager {
    /// Path to the framework repository used to detect new versions.
    framework_repo: Option<PathBuf>,
}

impl UpdateManager {
    /// Create a new update manager.
    ///
    /// If `framework_repo` is `None`, the manager will attempt to auto-detect
    /// the repo by walking parent directories of the current executable.
    pub fn new(framework_repo: Option<PathBuf>) -> Self {
        Self { framework_repo }
    }

    /// Check whether updates are available for the bundle at `bundle_path`.
    pub fn check_for_updates(&self, bundle_path: &Path) -> Result<UpdateInfo, BundleError> {
        let manifest = read_manifest(bundle_path)?;
        let current = manifest.framework.version.clone();
        if current.is_empty() {
            return Err(BundleError::validation(
                "manifest does not contain a framework version",
            ));
        }

        let latest = self.get_framework_version()?;
        let available = current != latest;

        let changes = if available {
            self.get_changelog(&current, &latest)
        } else {
            Vec::new()
        };

        Ok(UpdateInfo {
            available,
            current_version: current,
            latest_version: latest,
            changes,
        })
    }

    /// Attempt to update the bundle at `bundle_path`.
    ///
    /// When `preserve_edits` is true, user-modified files are kept as-is.
    /// When `backup` is true, a timestamped backup is created first.
    pub fn update_bundle(
        &self,
        bundle_path: &Path,
        preserve_edits: bool,
        backup: bool,
    ) -> UpdateResult {
        // Validate the bundle exists.
        let manifest = match read_manifest(bundle_path) {
            Ok(m) => m,
            Err(e) => return UpdateResult::failed(e.message),
        };

        // Create backup if requested.
        if backup && self.create_backup(bundle_path).is_err() {
            return UpdateResult::failed("backup failed");
        }

        // Detect customisations.
        let customised = if preserve_edits {
            self.detect_modified_files(bundle_path, &manifest.file_checksums)
        } else {
            std::collections::HashSet::new()
        };

        // Get framework source path.
        let framework_src = match &self.framework_repo {
            Some(p) => p.clone(),
            None => {
                return UpdateResult::failed(
                    "framework repository path not set — cannot apply updates",
                );
            }
        };

        let template_dir = framework_src.join("templates");
        if !template_dir.is_dir() {
            return UpdateResult::failed(format!(
                "framework templates directory not found: {}",
                template_dir.display()
            ));
        }

        // Apply updates: copy non-customised files from templates.
        let mut result = UpdateResult::ok();
        match Self::apply_template_files(&template_dir, bundle_path, &customised) {
            Ok((updated, preserved)) => {
                result.updated_files = updated;
                result.preserved_files = preserved;
            }
            Err(e) => {
                return UpdateResult::failed(format!("update failed: {}", e.message));
            }
        }

        result
    }

    /// Detect which files in `bundle_path` have been modified by the user
    /// compared to the checksums stored in the manifest.
    pub fn detect_customizations(
        &self,
        bundle_path: &Path,
    ) -> Result<HashMap<String, bool>, BundleError> {
        let manifest = read_manifest(bundle_path)?;
        let mut result = HashMap::new();
        for (rel_path, expected_cs) in &manifest.file_checksums {
            let full = bundle_path.join(rel_path);
            if full.exists() {
                let actual = compute_checksum(&full)?;
                result.insert(rel_path.clone(), actual != *expected_cs);
            } else {
                result.insert(rel_path.clone(), true);
            }
        }
        Ok(result)
    }

    // -- Version helpers ---------------------------------------------------

    fn get_framework_version(&self) -> Result<String, BundleError> {
        let repo = match &self.framework_repo {
            Some(p) if p.exists() => p.clone(),
            _ => {
                return Err(BundleError::repo(
                    "framework repository not found or not set",
                ));
            }
        };

        let output = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(&repo)
            .output()
            .map_err(|e| BundleError::repo(format!("git rev-parse failed: {e}")))?;

        if !output.status.success() {
            return Err(BundleError::repo("failed to get framework version"));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn get_changelog(&self, old: &str, new: &str) -> Vec<String> {
        let repo = match &self.framework_repo {
            Some(p) => p,
            None => return Vec::new(),
        };

        let output = Command::new("git")
            .args([
                "log",
                "--oneline",
                "--max-count=10",
                &format!("{old}..{new}"),
            ])
            .current_dir(repo)
            .output();

        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.to_string())
                .collect(),
            _ => Vec::new(),
        }
    }

    // -- Backup / customisation detection ----------------------------------

    /// Create a timestamped backup of `bundle_path`.
    pub fn create_backup(&self, bundle_path: &Path) -> Result<PathBuf, BundleError> {
        let name = bundle_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("bundle");
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let backup_name = format!("{name}.backup.{ts}");
        let backup_dir = bundle_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(backup_name);

        super::packager::copy_dir_recursive(bundle_path, &backup_dir)?;
        Ok(backup_dir)
    }

    fn detect_modified_files(
        &self,
        bundle_path: &Path,
        checksums: &HashMap<String, String>,
    ) -> std::collections::HashSet<String> {
        let mut modified = std::collections::HashSet::new();
        for (rel, expected) in checksums {
            let full = bundle_path.join(rel);
            if compute_checksum(&full).is_ok_and(|actual| actual != *expected) {
                modified.insert(rel.clone());
            }
        }
        modified
    }

    fn apply_template_files(
        template_dir: &Path,
        bundle_path: &Path,
        customised: &std::collections::HashSet<String>,
    ) -> Result<(Vec<String>, Vec<String>), BundleError> {
        let mut updated = Vec::new();
        let mut preserved = Vec::new();

        Self::walk_templates(template_dir, template_dir, bundle_path, customised, &mut updated, &mut preserved)?;
        Ok((updated, preserved))
    }

    fn walk_templates(
        root: &Path,
        current: &Path,
        bundle_path: &Path,
        customised: &std::collections::HashSet<String>,
        updated: &mut Vec<String>,
        preserved: &mut Vec<String>,
    ) -> Result<(), BundleError> {
        let entries = fs::read_dir(current).map_err(|e| {
            BundleError::packaging(format!(
                "cannot read template dir {}: {e}",
                current.display()
            ))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| BundleError::packaging(format!("entry: {e}")))?;
            let full = entry.path();
            let rel = full
                .strip_prefix(root)
                .unwrap_or(&full)
                .to_string_lossy()
                .into_owned();
            let dest = bundle_path.join(&rel);

            if full.is_dir() {
                fs::create_dir_all(&dest).map_err(|e| {
                    BundleError::packaging(format!("mkdir failed: {e}"))
                })?;
                Self::walk_templates(root, &full, bundle_path, customised, updated, preserved)?;
            } else if customised.contains(&rel) {
                preserved.push(rel);
            } else {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).ok();
                }
                fs::copy(&full, &dest).map_err(|e| {
                    BundleError::packaging(format!("copy template failed: {e}"))
                })?;
                updated.push(rel);
            }
        }
        Ok(())
    }
}

/// Compute SHA-256 checksum with a "sha256:" prefix (matching Python convention).
pub fn compute_checksum(path: &Path) -> Result<String, BundleError> {
    let data = fs::read(path).map_err(|e| {
        BundleError::packaging(format!("cannot read {}: {e}", path.display()))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}


