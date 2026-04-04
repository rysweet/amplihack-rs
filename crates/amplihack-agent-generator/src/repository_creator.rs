//! GitHub repository creation for agent bundles.
//!
//! Uses the `gh` CLI for all repository operations: create, delete, and
//! existence checks.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Result of a repository operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RepositoryResult {
    fn ok(url: &str, repository: &str) -> Self {
        Self {
            success: true,
            url: Some(url.into()),
            repository: Some(repository.into()),
            error: None,
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            url: None,
            repository: None,
            error: Some(msg.into()),
        }
    }
}

/// Manages GitHub repositories for agent bundles via the `gh` CLI.
pub struct RepositoryCreator {
    gh_available: bool,
}

impl RepositoryCreator {
    /// Create a new creator, verifying that `gh` is installed and authenticated.
    ///
    /// # Errors
    /// Returns `Err` if `gh` is missing or not authenticated.
    pub fn new() -> Result<Self, String> {
        check_gh_cli()?;
        Ok(Self { gh_available: true })
    }

    /// Create a new creator without verifying `gh` (for testing).
    #[cfg(test)]
    fn new_unchecked() -> Self {
        Self {
            gh_available: false,
        }
    }

    /// Create a GitHub repository for the bundle at `bundle_path`.
    pub fn create_repository(
        &self,
        bundle_path: &Path,
        repo_name: Option<&str>,
        private: bool,
        push: bool,
        organization: Option<&str>,
    ) -> RepositoryResult {
        if !bundle_path.exists() || !bundle_path.is_dir() {
            return RepositoryResult::err(format!(
                "Bundle path does not exist or is not a directory: {}",
                bundle_path.display()
            ));
        }

        let manifest_path = bundle_path.join("manifest.json");
        if !manifest_path.exists() {
            return RepositoryResult::err("No manifest.json found in bundle");
        }

        let manifest: serde_json::Value = match std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(v) => v,
            None => return RepositoryResult::err("Invalid manifest.json"),
        };

        let bundle_name = manifest["bundle"]["name"]
            .as_str()
            .unwrap_or("agent-bundle");
        let bundle_desc = manifest["bundle"]["description"]
            .as_str()
            .unwrap_or("Agent bundle");

        let final_name = repo_name.unwrap_or(bundle_name);

        // Ensure git is initialized
        if !bundle_path.join(".git").exists()
            && (run_git(&["init"], bundle_path).is_err()
                || run_git(&["add", "."], bundle_path).is_err()
                || run_git(&["commit", "-m", "Initial commit"], bundle_path).is_err())
        {
            return RepositoryResult::err("Failed to initialize git repository");
        }

        if !self.gh_available {
            return RepositoryResult::err("gh CLI not available");
        }

        // Build gh repo create command
        let visibility = if private { "--private" } else { "--public" };
        let qualified_name = match organization {
            Some(org) => format!("{org}/{final_name}"),
            None => final_name.to_string(),
        };

        let bundle_path_str = bundle_path.to_string_lossy();
        let mut args = vec![
            "repo",
            "create",
            &qualified_name,
            "--source",
            &bundle_path_str,
            "--description",
            bundle_desc,
            visibility,
        ];
        if push {
            args.push("--push");
        }

        let output = match Command::new("gh").args(&args).output() {
            Ok(o) => o,
            Err(e) => return RepositoryResult::err(format!("gh failed: {e}")),
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return RepositoryResult::err(stderr.trim().to_string());
        }

        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        RepositoryResult::ok(&url, &qualified_name)
    }

    /// Delete a GitHub repository. `confirm` must be `true`.
    pub fn delete_repository(&self, repository: &str, confirm: bool) -> RepositoryResult {
        if !confirm {
            return RepositoryResult::err("Must set confirm=true to delete repository");
        }
        if !repository.contains('/') {
            return RepositoryResult::err("Repository must be in format 'owner/repo'");
        }

        let output = match Command::new("gh")
            .args(["repo", "delete", repository, "--yes"])
            .output()
        {
            Ok(o) => o,
            Err(e) => return RepositoryResult::err(e.to_string()),
        };

        if output.status.success() {
            RepositoryResult {
                success: true,
                url: None,
                repository: Some(repository.into()),
                error: None,
            }
        } else {
            RepositoryResult::err(String::from_utf8_lossy(&output.stderr).trim())
        }
    }

    /// Check if a repository exists.
    pub fn check_repository_exists(&self, repository: &str) -> bool {
        Command::new("gh")
            .args(["repo", "view", repository])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

fn check_gh_cli() -> Result<(), String> {
    let version_ok = Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !version_ok {
        return Err("GitHub CLI (gh) is not installed".into());
    }

    let auth_ok = Command::new("gh")
        .args(["auth", "status"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !auth_ok {
        return Err("GitHub CLI is not authenticated. Run: gh auth login".into());
    }

    Ok(())
}

fn run_git(args: &[&str], cwd: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_ok_fields() {
        let r = RepositoryResult::ok("https://github.com/a/b", "a/b");
        assert!(r.success);
        assert_eq!(r.url.as_deref(), Some("https://github.com/a/b"));
        assert_eq!(r.repository.as_deref(), Some("a/b"));
        assert!(r.error.is_none());
    }

    #[test]
    fn result_err_fields() {
        let r = RepositoryResult::err("bad");
        assert!(!r.success);
        assert_eq!(r.error.as_deref(), Some("bad"));
    }

    #[test]
    fn delete_requires_confirm() {
        let creator = RepositoryCreator::new_unchecked();
        let r = creator.delete_repository("owner/repo", false);
        assert!(!r.success);
        assert!(r.error.as_deref().unwrap().contains("confirm"));
    }

    #[test]
    fn delete_requires_slash() {
        let creator = RepositoryCreator::new_unchecked();
        let r = creator.delete_repository("noslash", true);
        assert!(!r.success);
        assert!(r.error.as_deref().unwrap().contains("owner/repo"));
    }

    #[test]
    fn create_missing_path() {
        let creator = RepositoryCreator::new_unchecked();
        let r = creator.create_repository(Path::new("/nonexistent/path"), None, true, false, None);
        assert!(!r.success);
    }

    #[test]
    fn create_missing_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let creator = RepositoryCreator::new_unchecked();
        let r = creator.create_repository(dir.path(), None, true, false, None);
        assert!(!r.success);
        assert!(r.error.as_deref().unwrap().contains("manifest.json"));
    }
}
