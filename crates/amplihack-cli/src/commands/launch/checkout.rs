//! Repository checkout utilities for `--checkout-repo` flag support.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::util::{format_output_diagnostics, run_output_with_timeout};

const GIT_CLONE_TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) fn resolve_checkout_repo(repo_uri: Option<&str>) -> Result<Option<PathBuf>> {
    let Some(repo_uri) = repo_uri else {
        return Ok(None);
    };
    resolve_checkout_repo_in(repo_uri, &std::env::temp_dir().join("claude-checkouts")).map(Some)
}

pub(super) fn resolve_checkout_repo_in(repo_uri: &str, base_dir: &Path) -> Result<PathBuf> {
    let (owner, repo) = parse_github_repo_uri(repo_uri)?;
    let target_dir = base_dir.join(format!("{owner}-{repo}"));

    fs::create_dir_all(base_dir)
        .with_context(|| format!("failed to create checkout directory {}", base_dir.display()))?;

    if target_dir.join(".git").is_dir() {
        println!("Using existing repository: {}", target_dir.display());
        return Ok(target_dir);
    }

    if target_dir.exists() {
        fs::remove_dir_all(&target_dir)
            .with_context(|| format!("failed to remove {}", target_dir.display()))?;
    }

    let clone_url = format!("https://github.com/{owner}/{repo}.git");
    let target_dir_arg = target_dir.to_string_lossy();
    let mut command = Command::new("git");
    command
        .args(["clone", &clone_url, target_dir_arg.as_ref()])
        .stdin(Stdio::null());
    let output = run_output_with_timeout(command, GIT_CLONE_TIMEOUT)
        .context("git clone timed out or failed to execute")?;
    if !output.status.success() {
        bail!(
            "failed to checkout repository {repo_uri}: {}",
            format_output_diagnostics(&output, 400)
        );
    }

    println!("Cloned repository to: {}", target_dir.display());
    Ok(target_dir)
}

pub(super) fn parse_github_repo_uri(repo_uri: &str) -> Result<(String, String)> {
    let trimmed = repo_uri.trim();
    if trimmed.is_empty() {
        bail!("invalid GitHub repository URI: empty value");
    }

    let repo = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("git@github.com:"))
        .unwrap_or(trimmed);
    let repo = repo.strip_suffix(".git").unwrap_or(repo);

    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if parts.next().is_some() || !is_valid_github_segment(owner) || !is_valid_github_segment(name) {
        bail!("invalid GitHub repository URI: {repo_uri}");
    }

    Ok((owner.to_string(), name.to_string()))
}

fn is_valid_github_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}
