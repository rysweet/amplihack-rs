use amplihack_workflows::workflow_contract::{
    RepositoryIdentity, RepositoryProvider, repository_identity_from_remote_url,
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProviderDetection {
    pub(super) provider: RepositoryProvider,
    pub(super) repository: RepositoryIdentity,
    pub(super) warnings: Vec<String>,
}

pub(super) fn detect_provider_from_repo(repo: &Path) -> Result<ProviderDetection> {
    let mut warnings = Vec::new();
    let config = read_git_config(repo)?;
    let remote_url = config.as_deref().and_then(origin_remote_url);
    let fallback_name = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "repository".into());
    let (provider, repository) =
        repository_identity_from_remote_url(remote_url.as_deref(), &fallback_name);

    if config.is_none() {
        warnings.push("No Git config found; provider set to Manual.".into());
    } else if remote_url.is_none() {
        warnings.push("No origin remote URL found; provider set to Manual.".into());
    } else if provider == RepositoryProvider::Manual {
        warnings.push("Remote provider is unknown; provider set to Manual.".into());
    }

    Ok(ProviderDetection {
        provider,
        repository,
        warnings,
    })
}

fn read_git_config(repo: &Path) -> Result<Option<String>> {
    let Some(path) = git_config_path(repo)? else {
        return Ok(None);
    };
    std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read Git config from {}", path.display()))
        .map(Some)
}

fn git_config_path(repo: &Path) -> Result<Option<PathBuf>> {
    let dot_git = repo.join(".git");
    if dot_git.is_dir() {
        let config = dot_git.join("config");
        return Ok(config.is_file().then_some(config));
    }
    if !dot_git.is_file() {
        return Ok(None);
    }

    let marker = std::fs::read_to_string(&dot_git)
        .with_context(|| format!("failed to read Git marker file {}", dot_git.display()))?;
    let git_dir = marker
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("invalid Git marker file {}", dot_git.display()))?;
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        repo.join(git_dir)
    };

    let worktree_config = git_dir.join("config");
    if worktree_config.is_file() {
        return Ok(Some(worktree_config));
    }

    let common_dir_file = git_dir.join("commondir");
    if common_dir_file.is_file() {
        let common_dir = std::fs::read_to_string(&common_dir_file).with_context(|| {
            format!(
                "failed to read Git common-dir marker {}",
                common_dir_file.display()
            )
        })?;
        let common_dir = PathBuf::from(common_dir.trim());
        let common_dir = if common_dir.is_absolute() {
            common_dir
        } else {
            git_dir.join(common_dir)
        };
        let common_config = common_dir.join("config");
        if common_config.is_file() {
            return Ok(Some(common_config));
        }
    }

    Ok(None)
}

fn origin_remote_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    let mut first_remote_url = None;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_origin = trimmed == r#"[remote "origin"]"#;
            continue;
        }
        let Some(url) = trimmed.strip_prefix("url").and_then(|rest| {
            rest.trim_start()
                .strip_prefix('=')
                .map(str::trim)
                .filter(|value| !value.is_empty())
        }) else {
            continue;
        };
        if first_remote_url.is_none() {
            first_remote_url = Some(url.to_string());
        }
        if in_origin {
            return Some(url.to_string());
        }
    }
    first_remote_url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_provider_without_git_config_returns_manual_with_warning() {
        let temp = tempfile::tempdir().expect("tempdir");
        let detection = detect_provider_from_repo(temp.path()).expect("detection should succeed");

        assert_eq!(detection.provider, RepositoryProvider::Manual);
        assert_eq!(
            detection.repository.remote_url, None,
            "unknown repositories must not synthesize a GitHub remote"
        );
        assert!(
            detection
                .warnings
                .iter()
                .any(|warning| warning.contains("provider set to Manual"))
        );
    }

    #[test]
    fn detect_provider_reads_common_config_for_git_worktree_marker() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let git_dir = temp.path().join("main.git").join("worktrees").join("repo");
        let common_dir = temp.path().join("main.git");
        std::fs::create_dir_all(&repo).expect("repo dir");
        std::fs::create_dir_all(&git_dir).expect("git dir");
        std::fs::write(
            repo.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .expect("git marker");
        std::fs::write(git_dir.join("commondir"), "../..\n").expect("commondir");
        std::fs::write(
            common_dir.join("config"),
            r#"
[remote "origin"]
    url = https://dev.azure.com/acme/project/_git/service
"#,
        )
        .expect("config");

        let detection = detect_provider_from_repo(&repo).expect("detection should succeed");

        assert_eq!(detection.provider, RepositoryProvider::AzureDevOps);
        assert_eq!(detection.repository.owner, "acme/project");
        assert_eq!(detection.repository.name, "service");
        assert!(detection.warnings.is_empty());
    }

    #[test]
    fn unknown_remote_returns_manual_not_github() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let git = repo.join(".git");
        std::fs::create_dir_all(&git).expect("git dir");
        std::fs::write(
            git.join("config"),
            r#"
[remote "origin"]
    url = ssh://git.example.invalid/acme/service
"#,
        )
        .expect("config");

        let detection = detect_provider_from_repo(&repo).expect("detection should succeed");

        assert_eq!(detection.provider, RepositoryProvider::Manual);
        assert!(
            detection
                .warnings
                .iter()
                .any(|warning| warning.contains("unknown"))
        );
    }

    #[test]
    fn detect_provider_redacts_credential_bearing_remote_urls() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let git = repo.join(".git");
        std::fs::create_dir_all(&git).expect("git dir");
        std::fs::write(
            git.join("config"),
            r#"
[remote "origin"]
    url = https://user:ghp_secret_token@github.com/acme/service.git
"#,
        )
        .expect("config");

        let detection = detect_provider_from_repo(&repo).expect("detection should succeed");

        assert_eq!(detection.provider, RepositoryProvider::GitHub);
        assert_eq!(
            detection.repository.remote_url.as_deref(),
            Some("https://[redacted]@github.com/acme/service.git")
        );
    }
}
