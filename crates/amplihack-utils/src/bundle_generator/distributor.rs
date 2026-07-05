//! GitHub distribution for agent bundles via the `gh` CLI.

use super::error::BundleGeneratorError;
use super::models::{DistributionPlatform, DistributionResult, PackagedBundle};

/// Truncate a string to at most `max_bytes` bytes without splitting a
/// multi-byte UTF-8 character.
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Distributes agent bundles to GitHub repositories using the `gh` CLI.
pub struct GitHubDistributor {
    /// GitHub personal access token (passed to `gh` via env).
    token: String,
}

impl GitHubDistributor {
    /// Create a new distributor with a GitHub token.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    /// Create a GitHub repository.
    ///
    /// When `public` is `true` the repo is created with `--public`,
    /// otherwise `--private`.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Distribution`] on failure.
    pub fn create_repository(
        &self,
        repo_name: &str,
        description: &str,
        public: bool,
    ) -> Result<String, BundleGeneratorError> {
        let visibility = if public { "--public" } else { "--private" };
        let desc_truncated = truncate_to_char_boundary(description, 100);

        let output = std::process::Command::new("gh")
            .args(["repo", "create", repo_name, visibility])
            .arg("--description")
            .arg(desc_truncated)
            .env("GH_TOKEN", &self.token)
            .output()
            .map_err(|e| BundleGeneratorError::Distribution {
                message: format!("failed to run gh: {e}"),
                platform: Some("github".into()),
                http_status: None,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BundleGeneratorError::Distribution {
                message: format!("gh repo create failed: {stderr}"),
                platform: Some("github".into()),
                http_status: None,
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Push a bundle file to a GitHub repository using the Contents API.
    ///
    /// Writes the JSON body to a temp file and uses `gh api --input` to
    /// avoid CLI argument length limits. Fetches the existing file SHA
    /// first so updates are idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Distribution`] on failure.
    pub fn push_bundle(
        &self,
        repo: &str,
        path: &str,
        content: &[u8],
        message: &str,
    ) -> Result<(), BundleGeneratorError> {
        use base64::{Engine, engine::general_purpose::STANDARD};

        // GET existing file SHA for idempotent update (contents API)
        let existing_sha = self.get_file_sha(repo, path);

        let encoded = STANDARD.encode(content);
        let mut body = serde_json::json!({
            "message": message,
            "content": encoded,
        });
        if let Some(sha) = existing_sha {
            body["sha"] = serde_json::Value::String(sha);
        }

        // Write JSON body to a temp file to avoid E2BIG on large bundles
        let tmp =
            tempfile::NamedTempFile::new().map_err(|e| BundleGeneratorError::Distribution {
                message: format!("failed to create temp file: {e}"),
                platform: Some("github".into()),
                http_status: None,
            })?;
        std::fs::write(tmp.path(), serde_json::to_vec(&body).unwrap_or_default()).map_err(|e| {
            BundleGeneratorError::Distribution {
                message: format!("failed to write temp file: {e}"),
                platform: Some("github".into()),
                http_status: None,
            }
        })?;

        let api_path = format!("repos/{repo}/contents/{path}");
        let output = std::process::Command::new("gh")
            .args(["api", "-X", "PUT", &api_path, "--input"])
            .arg(tmp.path())
            .env("GH_TOKEN", &self.token)
            .output()
            .map_err(|e| BundleGeneratorError::Distribution {
                message: format!("failed to run gh api: {e}"),
                platform: Some("github".into()),
                http_status: None,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BundleGeneratorError::Distribution {
                message: format!("gh api PUT failed: {stderr}"),
                platform: Some("github".into()),
                http_status: None,
            });
        }

        Ok(())
    }

    /// Fetch the SHA of an existing file, or `None` if it does not exist.
    fn get_file_sha(&self, repo: &str, path: &str) -> Option<String> {
        let api_path = format!("repos/{repo}/contents/{path}");
        let output = std::process::Command::new("gh")
            .args(["api", &api_path, "--jq", ".sha"])
            .env("GH_TOKEN", &self.token)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if sha.is_empty() { None } else { Some(sha) }
    }

    /// Distribute a packaged bundle to GitHub.
    ///
    /// Creates the repository (if needed), pushes the bundle content, and
    /// returns a [`DistributionResult`].
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Distribution`] on failure.
    pub fn distribute(
        &self,
        bundle: &PackagedBundle,
        repo_name: &str,
    ) -> Result<DistributionResult, BundleGeneratorError> {
        self.distribute_with_options(bundle, repo_name, true)
    }

    /// Distribute with explicit visibility control.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Distribution`] on failure.
    pub fn distribute_with_options(
        &self,
        bundle: &PackagedBundle,
        repo_name: &str,
        public: bool,
    ) -> Result<DistributionResult, BundleGeneratorError> {
        let desc = truncate_to_char_boundary(&bundle.bundle.description, 100);
        let _repo_url = self.create_repository(repo_name, desc, public)?;

        let bundle_bytes = std::fs::read(&bundle.package_path).map_err(|e| {
            BundleGeneratorError::Distribution {
                message: format!("failed to read bundle: {e}"),
                platform: Some("github".into()),
                http_status: None,
            }
        })?;

        let file_name = bundle
            .package_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("bundle.tar.gz");

        self.push_bundle(
            repo_name,
            file_name,
            &bundle_bytes,
            &format!("Upload bundle {}", bundle.bundle.name),
        )?;

        Ok(DistributionResult {
            success: true,
            platform: DistributionPlatform::Github,
            url: Some(format!("https://github.com/{repo_name}")),
            repository: Some(repo_name.to_string()),
            branch: Some("main".into()),
            commit_sha: None,
            release_tag: None,
            errors: vec![],
            warnings: vec![],
            distribution_time_seconds: 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::models::{AgentBundle, BundleStatus, PackageFormat};
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn github_distributor_new_stores_token() {
        let d = GitHubDistributor::new("ghp_test123");
        assert_eq!(d.token, "ghp_test123");
    }

    #[test]
    fn distribute_fails_without_gh() {
        let d = GitHubDistributor::new("fake-token");
        let bundle = PackagedBundle {
            bundle: AgentBundle {
                id: "test-id".into(),
                name: "test-bundle".into(),
                version: "1.0.0".into(),
                description: "a test bundle".into(),
                agents: vec![],
                manifest: HashMap::new(),
                metadata: HashMap::new(),
                status: BundleStatus::Pending,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            package_path: PathBuf::from("/nonexistent/bundle.tar.gz"),
            format: PackageFormat::TarGz,
            size_bytes: 0,
            checksum: String::new(),
            created_at: Utc::now(),
        };
        let result = d.distribute(&bundle, "test/repo");
        assert!(result.is_err());
    }

    #[test]
    fn truncate_char_boundary_ascii() {
        assert_eq!(truncate_to_char_boundary("hello world", 5), "hello");
    }

    #[test]
    fn truncate_char_boundary_multibyte() {
        // 'café' — 'é' is 2 bytes. Byte 4 would split 'é'.
        let s = "café";
        let t = truncate_to_char_boundary(s, 4);
        assert!(t.len() <= 4);
        assert!(t.is_char_boundary(t.len()));
    }

    #[test]
    fn truncate_char_boundary_emoji() {
        // '🦀' = 4 bytes
        let t = truncate_to_char_boundary("🦀rust", 2);
        assert!(t.is_empty() || t.len() <= 2);
    }

    #[test]
    fn truncate_char_boundary_beyond_len() {
        assert_eq!(truncate_to_char_boundary("hi", 100), "hi");
    }

    #[test]
    fn truncate_char_boundary_empty() {
        assert!(truncate_to_char_boundary("", 10).is_empty());
    }

    #[test]
    fn truncate_char_boundary_zero() {
        assert!(truncate_to_char_boundary("hello", 0).is_empty());
    }

    #[test]
    fn push_bundle_json_structure() {
        use base64::{Engine, engine::general_purpose::STANDARD};

        let content = b"test bundle content";
        let encoded = STANDARD.encode(content);
        let mut body = serde_json::json!({
            "message": "Upload bundle",
            "content": encoded,
        });
        // Simulate idempotent update
        body["sha"] = serde_json::Value::String("abc123".into());

        let json_str = serde_json::to_string(&body).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["message"], "Upload bundle");
        assert_eq!(parsed["sha"], "abc123");
        let decoded = STANDARD
            .decode(parsed["content"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, content);
    }

    #[test]
    fn base64_crate_roundtrip() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let data = b"Hello GitHub distributor!";
        let encoded = STANDARD.encode(data);
        let decoded = STANDARD.decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }
}
