//! Bundle distributor — publishes packaged bundles to distribution targets.
//!
//! Ported from `amplihack/bundle_generator/distributor.py`.
//! Provides [`Distributor`] for GitHub release creation (via `gh` CLI) and
//! local filesystem distribution.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use super::error::BundleError;
use super::models::{DistributionPlatform, DistributionResult, PackagedBundle};
use super::packager::sha256_file;

/// Options for a distribution operation.
#[derive(Debug, Clone, Default)]
pub struct DistributionOptions {
    /// Whether to create a GitHub release (only for GitHub platform).
    pub create_release: bool,
    /// Release tag override (defaults to "v1.0.0").
    pub release_tag: Option<String>,
    /// Extra notes appended to release body.
    pub extra_notes: Option<String>,
    /// Branch name for the push (defaults to "main").
    pub branch: Option<String>,
}

/// Distributes packaged bundles to various targets.
pub struct Distributor {
    /// GitHub organization or user.
    pub organization: Option<String>,
    /// Default branch name.
    pub default_branch: String,
}

impl Distributor {
    /// Create a new distributor. `organization` is required for GitHub
    /// distribution.
    pub fn new(organization: Option<String>, default_branch: Option<String>) -> Self {
        Self {
            organization,
            default_branch: default_branch.unwrap_or_else(|| "main".to_string()),
        }
    }

    /// Distribute `package` to `platform`.
    pub fn distribute(
        &self,
        package: &PackagedBundle,
        platform: DistributionPlatform,
        repository: &str,
        options: &DistributionOptions,
    ) -> DistributionResult {
        let start = Instant::now();
        let mut result = DistributionResult::ok(platform);

        match platform {
            DistributionPlatform::Github => {
                match self.distribute_github(package, repository, options) {
                    Ok(info) => {
                        result.url = Some(info.url);
                        result.release_tag = info.release_tag;
                        result.assets = info.assets;
                    }
                    Err(e) => {
                        result.success = false;
                        result.errors.push(e.message.clone());
                    }
                }
            }
            DistributionPlatform::Local => {
                match self.distribute_local(package, repository) {
                    Ok(path) => {
                        result.url = Some(format!("file://{}", path.display()));
                    }
                    Err(e) => {
                        result.success = false;
                        result.errors.push(e.message.clone());
                    }
                }
            }
            DistributionPlatform::Pypi => {
                result.success = false;
                result
                    .errors
                    .push("PyPI distribution not yet supported".to_string());
            }
        }

        result.distribution_time_seconds = start.elapsed().as_secs_f64();
        result.timestamp = chrono::Utc::now().to_rfc3339();
        result
    }

    // -- GitHub distribution -----------------------------------------------

    fn distribute_github(
        &self,
        package: &PackagedBundle,
        repository: &str,
        options: &DistributionOptions,
    ) -> Result<GhDistInfo, BundleError> {
        if !Self::has_gh_cli() {
            return Err(BundleError::distribution(
                "GitHub CLI (gh) is not installed or not in PATH",
            ));
        }

        let org = self.organization.as_deref().ok_or_else(|| {
            BundleError::distribution("organization is required for GitHub distribution")
        })?;

        let full_repo = if repository.contains('/') {
            repository.to_string()
        } else {
            format!("{org}/{repository}")
        };

        let branch = options
            .branch
            .as_deref()
            .unwrap_or(&self.default_branch);

        // Ensure repo exists (create if needed).
        self.ensure_repo(&full_repo)?;

        // Upload the package file.
        let asset_path = &package.path;
        let upload_url = self.upload_to_repo(&full_repo, asset_path, branch)?;

        let mut info = GhDistInfo {
            url: upload_url,
            release_tag: None,
            assets: vec![asset_path.display().to_string()],
        };

        // Optionally create a release.
        if options.create_release {
            let tag = options
                .release_tag
                .clone()
                .unwrap_or_else(|| "v1.0.0".to_string());
            let notes = self.build_release_notes(package, options);
            match self.create_release(&full_repo, &tag, &notes, asset_path) {
                Ok(release_url) => {
                    info.release_tag = Some(tag);
                    info.url = release_url;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        Ok(info)
    }

    /// Check whether `gh` CLI is available.
    fn has_gh_cli() -> bool {
        Command::new("gh")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Ensure the repository exists, create it if not.
    fn ensure_repo(&self, full_repo: &str) -> Result<(), BundleError> {
        let status = Command::new("gh")
            .args(["repo", "view", full_repo, "--json", "name"])
            .output()
            .map_err(|e| BundleError::distribution(format!("gh repo view failed: {e}")))?;

        if !status.status.success() {
            let out = Command::new("gh")
                .args([
                    "repo",
                    "create",
                    full_repo,
                    "--public",
                    "--description",
                    "Amplihack agent bundle",
                ])
                .output()
                .map_err(|e| {
                    BundleError::distribution(format!("gh repo create failed: {e}"))
                })?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(BundleError::distribution(format!(
                    "failed to create repository {full_repo}: {stderr}"
                )));
            }
        }
        Ok(())
    }

    /// Push the package asset to the repository.
    fn upload_to_repo(
        &self,
        full_repo: &str,
        asset: &Path,
        branch: &str,
    ) -> Result<String, BundleError> {
        // Clone into a temp work-tree, copy asset, commit and push.
        let work = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".dist-work");
        let _ = fs::remove_dir_all(&work);

        let clone_out = Command::new("gh")
            .args(["repo", "clone", full_repo, work.to_string_lossy().as_ref()])
            .output()
            .map_err(|e| BundleError::distribution(format!("clone failed: {e}")))?;

        if !clone_out.status.success() {
            let stderr = String::from_utf8_lossy(&clone_out.stderr);
            return Err(BundleError::distribution(format!(
                "clone failed: {stderr}"
            )));
        }

        // Copy asset into the work-tree.
        let asset_name = asset
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("bundle");
        let dest = work.join(asset_name);
        fs::copy(asset, &dest).map_err(|e| {
            BundleError::distribution(format!("copy asset failed: {e}"))
        })?;

        // Git add + commit + push.
        let run_git = |args: &[&str]| -> Result<(), BundleError> {
            let out = Command::new("git")
                .args(args)
                .current_dir(&work)
                .output()
                .map_err(|e| BundleError::distribution(format!("git {}: {e}", args[0])))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(BundleError::distribution(format!(
                    "git {} failed: {stderr}",
                    args[0]
                )));
            }
            Ok(())
        };

        run_git(&["add", asset_name])?;
        run_git(&["commit", "-m", &format!("Add bundle {asset_name}")])?;
        run_git(&["push", "origin", branch])?;

        // Clean up.
        let _ = fs::remove_dir_all(&work);

        Ok(format!("https://github.com/{full_repo}"))
    }

    /// Create a GitHub release with the given tag.
    fn create_release(
        &self,
        full_repo: &str,
        tag: &str,
        notes: &str,
        asset: &Path,
    ) -> Result<String, BundleError> {
        let out = Command::new("gh")
            .args([
                "release",
                "create",
                tag,
                "--repo",
                full_repo,
                "--title",
                &format!("Release {tag}"),
                "--notes",
                notes,
                asset.to_string_lossy().as_ref(),
            ])
            .output()
            .map_err(|e| BundleError::distribution(format!("gh release create: {e}")))?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(BundleError::distribution(format!(
                "release creation failed: {stderr}"
            )));
        }

        let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(if url.is_empty() {
            format!("https://github.com/{full_repo}/releases/tag/{tag}")
        } else {
            url
        })
    }

    fn build_release_notes(&self, package: &PackagedBundle, options: &DistributionOptions) -> String {
        let mut notes = String::from("## Bundle Release\n\n");
        notes.push_str(&format!("- **Format:** {}\n", package.format));
        notes.push_str(&format!("- **Size:** {} bytes\n", package.size_bytes));
        if !package.checksum.is_empty() {
            notes.push_str(&format!("- **SHA-256:** `{}`\n", package.checksum));
        }
        if let Some(extra) = &options.extra_notes {
            notes.push_str(&format!("\n{extra}\n"));
        }
        notes
    }

    // -- Local distribution ------------------------------------------------

    fn distribute_local(
        &self,
        package: &PackagedBundle,
        target: &str,
    ) -> Result<PathBuf, BundleError> {
        let target_path = PathBuf::from(target);
        fs::create_dir_all(&target_path).map_err(|e| {
            BundleError::distribution(format!(
                "cannot create target directory {}: {e}",
                target_path.display()
            ))
        })?;

        let src = &package.path;
        let dest_name = src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("bundle");
        let dest = target_path.join(dest_name);

        if src.is_dir() {
            super::packager::copy_dir_recursive(src, &dest)?;
        } else {
            fs::copy(src, &dest).map_err(|e| {
                BundleError::distribution(format!("copy failed: {e}"))
            })?;
        }

        // Write a manifest alongside the distributed bundle.
        let manifest = serde_json::json!({
            "format": format!("{}", package.format),
            "checksum": package.checksum,
            "size_bytes": package.size_bytes,
            "distributed_at": chrono::Utc::now().to_rfc3339(),
        });
        let manifest_path = target_path.join("distribution_manifest.json");
        fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap_or_default())
            .map_err(|e| {
                BundleError::distribution(format!("manifest write failed: {e}"))
            })?;

        Ok(dest)
    }
}

/// Verify the integrity of a distributed package against its recorded checksum.
pub fn verify_checksum(package_path: &Path, expected: &str) -> Result<bool, BundleError> {
    if expected.is_empty() {
        return Ok(true);
    }
    let actual = sha256_file(package_path)?;
    Ok(actual == expected)
}

// Internal helper used by distribute_github.
struct GhDistInfo {
    url: String,
    release_tag: Option<String>,
    assets: Vec<String>,
}


