//! Repository checkout utilities.
//!
//! Matches Python `amplihack/launcher/repo_checkout.py`:
//! - Parse GitHub URIs (owner/repo, HTTPS, SSH)
//! - Clone repositories via `git clone`

use std::path::{Path, PathBuf};
use std::process::Command;

/// Parse any GitHub URI format to `owner/repo`.
///
/// Supports:
/// - `owner/repo`
/// - `https://github.com/owner/repo.git`
/// - `git@github.com:owner/repo.git`
pub fn parse_github_uri(uri: &str) -> anyhow::Result<String> {
    if uri.is_empty() {
        anyhow::bail!("Empty GitHub URI");
    }

    // Already in owner/repo format
    let simple_re = regex::Regex::new(r"^[a-zA-Z0-9._-]+/[a-zA-Z0-9._-]+$").unwrap();
    if simple_re.is_match(uri) {
        return Ok(uri.to_string());
    }

    // HTTPS URL
    let https_re =
        regex::Regex::new(r"^https://github\.com/([a-zA-Z0-9._-]+/[a-zA-Z0-9._-]+?)(?:\.git)?$")
            .unwrap();
    if let Some(caps) = https_re.captures(uri) {
        return Ok(caps[1].to_string());
    }

    // SSH URL
    let ssh_re =
        regex::Regex::new(r"^git@github\.com:([a-zA-Z0-9._-]+/[a-zA-Z0-9._-]+?)(?:\.git)?$")
            .unwrap();
    if let Some(caps) = ssh_re.captures(uri) {
        return Ok(caps[1].to_string());
    }

    anyhow::bail!("Invalid GitHub URI: {uri}")
}

/// Checkout a GitHub repository.
///
/// Returns the path to the cloned repository, or `None` if cloning failed.
pub fn checkout_repository(repo_uri: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    let owner_repo = match parse_github_uri(repo_uri) {
        Ok(or) => or,
        Err(e) => {
            tracing::warn!("Checkout error: {e}");
            return None;
        }
    };

    let parts: Vec<&str> = owner_repo.splitn(2, '/').collect();
    if parts.len() != 2 {
        return None;
    }
    let dir_name = format!("{}-{}", parts[0], parts[1]);

    let base = base_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("claude-checkouts"));
    std::fs::create_dir_all(&base).ok()?;

    let target_dir = base.join(&dir_name);

    // Use existing if valid
    if target_dir.exists() && target_dir.join(".git").exists() {
        tracing::info!("Using existing repository: {}", target_dir.display());
        return Some(target_dir);
    }

    // Remove invalid directory
    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir).ok()?;
    }

    let clone_url = format!("https://github.com/{owner_repo}.git");
    let result = Command::new("git")
        .args(["clone", &clone_url, &target_dir.to_string_lossy()])
        .output()
        .ok()?;

    if result.status.success() {
        tracing::info!("Cloned repository to: {}", target_dir.display());
        Some(target_dir)
    } else {
        tracing::warn!("Clone failed: {}", String::from_utf8_lossy(&result.stderr));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_owner_repo() {
        assert_eq!(parse_github_uri("owner/repo").unwrap(), "owner/repo");
    }

    #[test]
    fn parse_https_url() {
        assert_eq!(
            parse_github_uri("https://github.com/owner/repo").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn parse_https_url_with_git() {
        assert_eq!(
            parse_github_uri("https://github.com/owner/repo.git").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn parse_ssh_url() {
        assert_eq!(
            parse_github_uri("git@github.com:owner/repo").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn parse_ssh_url_with_git() {
        assert_eq!(
            parse_github_uri("git@github.com:owner/repo.git").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn parse_empty_fails() {
        assert!(parse_github_uri("").is_err());
    }

    #[test]
    fn parse_invalid_url() {
        assert!(parse_github_uri("https://gitlab.com/owner/repo").is_err());
    }

    #[test]
    fn parse_with_dots_and_hyphens() {
        assert_eq!(
            parse_github_uri("my-org/my.project-name").unwrap(),
            "my-org/my.project-name"
        );
    }

    #[test]
    fn parse_ssh_with_dots() {
        assert_eq!(
            parse_github_uri("git@github.com:my-org/my.project.git").unwrap(),
            "my-org/my.project"
        );
    }

    #[test]
    fn checkout_invalid_uri_returns_none() {
        assert!(checkout_repository("", None).is_none());
    }

    #[test]
    fn checkout_existing_repo() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("owner-repo");
        std::fs::create_dir_all(target.join(".git")).unwrap();

        let result = checkout_repository("owner/repo", Some(dir.path()));
        assert!(result.is_some());
        assert_eq!(result.unwrap(), target);
    }
}
