//! Repository checkout utilities for `--checkout-repo` flag support.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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
    let mut command = amplihack_git::command();
    command
        .args(["clone", &clone_url, target_dir_arg.as_ref()])
        .stdin(Stdio::null());
    amplihack_utils::litellm_proxy::scrub_inference_environment(&mut command);
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
    if parts.next().is_some()
        || owner.starts_with('-')
        || !is_valid_github_segment(owner)
        || !is_valid_github_segment(name)
    {
        bail!("invalid GitHub repository URI: {repo_uri}");
    }

    Ok((owner.to_string(), name.to_string()))
}

/// `.` and `..` are rejected because they name another path once resolved
/// (issue #1642).
fn is_valid_github_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

#[cfg(test)]
mod issue_1642_tests {
    use super::parse_github_repo_uri;

    /// Issue #1642 (D4): `parse_github_repo_uri` has no userinfo branch, so
    /// the hostile origins from the issue, whose userinfo before
    /// `@github.com/` holds '/', '?', '#' or a backslash, must stay errors.
    /// Pins that behaviour so a later "accept tokens" change cannot reopen it.
    #[test]
    fn rejects_issue_1642_hostile_origins() {
        for uri in [
            "https://evil.example/x@github.com/a/b",
            "https://evil.example#@github.com/o/r",
            "https://evil.example?@github.com/o/r",
            r"https://evil.example\@github.com/o/r",
            "https://a@github.com/x@github.com/o/r",
        ] {
            assert!(
                parse_github_repo_uri(uri).is_err(),
                "{uri:?} must not parse as a GitHub repository"
            );
        }
    }

    /// Path shape and segments: exactly `owner/repo`, never `.` or `..`, and
    /// an owner that does not start with `-` (it reaches `git clone` and `gh`
    /// as an argument).
    #[test]
    fn rejects_path_shape_and_invalid_segments() {
        for uri in [
            "https://github.com/o/r/extra",
            "https://github.com/o/r/../../x/y",
            "https://github.com/o/r/pull/5",
            "git@github.com:o/r/tree/main",
            "o/r/extra",
            "https://github.com/./r",
            "https://github.com/../r",
            "https://github.com/o/..",
            "https://github.com/o/.",
            "https://github.com/o/..git",
            "https://github.com/-o/r",
            "git@github.com:-o/r",
            "git@github.com:../r",
            "../r",
            "-o/r",
        ] {
            assert!(
                parse_github_repo_uri(uri).is_err(),
                "{uri:?} must not parse as a GitHub repository"
            );
        }
    }

    #[test]
    fn real_forms_still_parse_to_owner_repo() {
        for uri in [
            "https://github.com/o/r",
            "https://github.com/o/r.git",
            "git@github.com:o/r",
            "git@github.com:o/r.git",
            "o/r",
        ] {
            let (owner, name) = parse_github_repo_uri(uri).expect(uri);
            assert_eq!((owner.as_str(), name.as_str()), ("o", "r"), "{uri:?}");
        }
    }
}
