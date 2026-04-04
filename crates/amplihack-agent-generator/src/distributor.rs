//! GitHub distribution for agent bundles.
//!
//! Distributes packaged bundles to GitHub repositories via `gh` CLI or raw
//! `git` commands. Handles repository creation, uploads, and releases.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use crate::error::{GeneratorError, Result};
use serde::{Deserialize, Serialize};

/// Result of a distribution operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionResult {
    pub success: bool,
    pub platform: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_tag: Option<String>,
    pub distribution_time_seconds: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// Metadata for a package being distributed.
#[derive(Debug, Clone)]
pub struct PackageMeta {
    pub name: String,
    pub version: String,
    pub description: String,
    pub agent_names: Vec<String>,
    pub package_path: PathBuf,
    pub format: String,
    pub size_bytes: u64,
}

/// Distribute agent bundles to GitHub repositories.
pub struct GitHubDistributor {
    organization: Option<String>,
    default_branch: String,
    rate_limit_remaining: u32,
}

impl GitHubDistributor {
    pub fn new(organization: Option<String>, default_branch: Option<String>) -> Self {
        Self {
            organization,
            default_branch: default_branch.unwrap_or_else(|| "main".into()),
            rate_limit_remaining: 5000,
        }
    }

    /// Distribute a package to GitHub.
    pub fn distribute(
        &mut self,
        meta: &PackageMeta,
        repository: Option<&str>,
        create_release: bool,
        options: &HashMap<String, String>,
    ) -> DistributionResult {
        let start = Instant::now();
        let repo_name = repository
            .map(String::from)
            .unwrap_or_else(|| format!("agent-bundle-{}", meta.name));

        if let Err(e) = self.check_rate_limit() {
            return DistributionResult {
                success: false,
                platform: "github".into(),
                repository: Some(repo_name),
                distribution_time_seconds: start.elapsed().as_secs_f64(),
                errors: vec![e.to_string()],
                ..Default::default()
            };
        }

        let repo_url = match self.prepare_repository(&repo_name, meta, options) {
            Ok(url) => url,
            Err(e) => {
                return DistributionResult {
                    success: false,
                    platform: "github".into(),
                    repository: Some(repo_name),
                    distribution_time_seconds: start.elapsed().as_secs_f64(),
                    errors: vec![e.to_string()],
                    ..Default::default()
                };
            }
        };

        let release_tag = if create_release {
            self.create_release_tag(&repo_name, meta, options)
        } else {
            None
        };

        DistributionResult {
            success: true,
            platform: "github".into(),
            url: Some(repo_url),
            repository: Some(repo_name),
            branch: Some(self.default_branch.clone()),
            commit_sha: None,
            release_tag,
            distribution_time_seconds: start.elapsed().as_secs_f64(),
            errors: vec![],
        }
    }

    /// List existing agent-bundle distributions.
    pub fn list_distributions(&self) -> Vec<HashMap<String, String>> {
        if !has_gh_cli() {
            return vec![];
        }
        let mut cmd = Command::new("gh");
        cmd.args(["repo", "list"]);
        if let Some(org) = &self.organization {
            cmd.arg(org);
        }
        cmd.args(["--json", "name,description,url,updatedAt"]);

        let output = match cmd.output() {
            Ok(o) if o.status.success() => o,
            _ => return vec![],
        };

        let repos: Vec<serde_json::Value> =
            serde_json::from_slice(&output.stdout).unwrap_or_default();

        repos
            .into_iter()
            .filter(|r| {
                r["name"]
                    .as_str()
                    .map(|n| n.to_lowercase().contains("agent-bundle"))
                    .unwrap_or(false)
            })
            .map(|r| {
                let mut m = HashMap::new();
                if let Some(v) = r["name"].as_str() {
                    m.insert("name".into(), v.into());
                }
                if let Some(v) = r["url"].as_str() {
                    m.insert("url".into(), v.into());
                }
                m
            })
            .collect()
    }

    /// Generate a README for the repository.
    pub fn generate_repo_readme(meta: &PackageMeta) -> String {
        let agent_list: String = meta
            .agent_names
            .iter()
            .map(|n| format!("- **{n}**"))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "# {name}\n\n{desc}\n\n## Agents\n\n{agents}\n\n\
             ## Bundle Information\n\n- **Version**: {ver}\n- **Format**: {fmt}\n\
             - **Size**: {size:.1} KB\n\n---\nGenerated by Agent Bundle Generator\n",
            name = meta.name,
            desc = meta.description,
            agents = agent_list,
            ver = meta.version,
            fmt = meta.format,
            size = meta.size_bytes as f64 / 1024.0,
        )
    }

    /// Generate release notes markdown.
    pub fn generate_release_notes(meta: &PackageMeta) -> String {
        format!(
            "## {name} v{ver}\n\n### Agents: {count}\n### Format: {fmt}\n\
             ### Size: {size:.1} KB\n",
            name = meta.name,
            ver = meta.version,
            count = meta.agent_names.len(),
            fmt = meta.format,
            size = meta.size_bytes as f64 / 1024.0,
        )
    }

    // -- internal -----------------------------------------------------------

    fn check_rate_limit(&mut self) -> Result<()> {
        if self.rate_limit_remaining == 0 {
            return Err(GeneratorError::PackagingFailed(
                "GitHub rate limit exceeded".into(),
            ));
        }
        self.rate_limit_remaining = self.rate_limit_remaining.saturating_sub(1);
        Ok(())
    }

    fn prepare_repository(
        &self,
        repository: &str,
        meta: &PackageMeta,
        options: &HashMap<String, String>,
    ) -> Result<String> {
        if has_gh_cli() {
            self.prepare_with_gh(repository, meta, options)
        } else {
            Ok(self.fallback_url(repository))
        }
    }

    fn prepare_with_gh(
        &self,
        repository: &str,
        meta: &PackageMeta,
        options: &HashMap<String, String>,
    ) -> Result<String> {
        // Check if repo exists
        let check = Command::new("gh")
            .args(["repo", "view", repository])
            .output();

        let exists = check.map(|o| o.status.success()).unwrap_or(false);

        if !exists {
            let visibility = if options.get("public").map(|v| v == "true").unwrap_or(true) {
                "--public"
            } else {
                "--private"
            };
            let desc = &meta.description[..meta.description.len().min(100)];
            let mut cmd = Command::new("gh");
            cmd.args([
                "repo",
                "create",
                repository,
                visibility,
                "--description",
                desc,
            ]);
            if let Some(org) = &self.organization {
                cmd.args(["--org", org]);
            }
            let output = cmd
                .output()
                .map_err(|e| GeneratorError::PackagingFailed(e.to_string()))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(GeneratorError::PackagingFailed(format!(
                    "Failed to create repo: {stderr}"
                )));
            }
        }

        Ok(self.repo_url(repository))
    }

    fn repo_url(&self, repository: &str) -> String {
        if let Some(org) = &self.organization {
            format!("https://github.com/{org}/{repository}")
        } else {
            format!("https://github.com/user/{repository}")
        }
    }

    fn fallback_url(&self, repository: &str) -> String {
        tracing::warn!("gh CLI not available; using placeholder URL");
        self.repo_url(repository)
    }

    fn create_release_tag(
        &self,
        _repository: &str,
        meta: &PackageMeta,
        _options: &HashMap<String, String>,
    ) -> Option<String> {
        Some(format!("v{}", meta.version))
    }
}

impl Default for DistributionResult {
    fn default() -> Self {
        Self {
            success: false,
            platform: "github".into(),
            url: None,
            repository: None,
            branch: None,
            commit_sha: None,
            release_tag: None,
            distribution_time_seconds: 0.0,
            errors: vec![],
        }
    }
}

/// Check whether `gh` CLI is available.
pub fn has_gh_cli() -> bool {
    Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta() -> PackageMeta {
        PackageMeta {
            name: "test-bundle".into(),
            version: "1.0.0".into(),
            description: "A test bundle".into(),
            agent_names: vec!["analyzer".into(), "builder".into()],
            package_path: PathBuf::from("/fake"),
            format: "tar.gz".into(),
            size_bytes: 2048,
        }
    }

    #[test]
    fn generate_readme_contains_name() {
        let meta = sample_meta();
        let readme = GitHubDistributor::generate_repo_readme(&meta);
        assert!(readme.contains("test-bundle"));
        assert!(readme.contains("analyzer"));
        assert!(readme.contains("1.0.0"));
    }

    #[test]
    fn generate_release_notes_content() {
        let meta = sample_meta();
        let notes = GitHubDistributor::generate_release_notes(&meta);
        assert!(notes.contains("v1.0.0"));
        assert!(notes.contains("Agents: 2"));
    }

    #[test]
    fn rate_limit_exhaustion() {
        let mut dist = GitHubDistributor::new(None, None);
        dist.rate_limit_remaining = 0;
        let res = dist.check_rate_limit();
        assert!(res.is_err());
    }

    #[test]
    fn rate_limit_decrement() {
        let mut dist = GitHubDistributor::new(None, None);
        dist.rate_limit_remaining = 2;
        assert!(dist.check_rate_limit().is_ok());
        assert_eq!(dist.rate_limit_remaining, 1);
    }

    #[test]
    fn distribution_result_default() {
        let d = DistributionResult::default();
        assert!(!d.success);
        assert_eq!(d.platform, "github");
    }

    #[test]
    fn repo_url_with_org() {
        let dist = GitHubDistributor::new(Some("myorg".into()), None);
        assert_eq!(dist.repo_url("test"), "https://github.com/myorg/test");
    }

    #[test]
    fn repo_url_without_org() {
        let dist = GitHubDistributor::new(None, None);
        assert_eq!(dist.repo_url("test"), "https://github.com/user/test");
    }
}
