use crate::workflow_contract::{RepositoryIdentity, RepositoryProvider};

pub fn provider_from_remote_url(remote_url: Option<&str>) -> RepositoryProvider {
    provider_from_remote_parts(remote_url.and_then(remote_url_parts))
}

fn provider_from_remote_parts(parts: Option<RemoteUrlParts<'_>>) -> RepositoryProvider {
    match parts.map(|parts| parts.host) {
        Some(host)
            if host_matches_provider_domain(host, "dev.azure.com")
                || host_matches_provider_domain(host, "visualstudio.com") =>
        {
            RepositoryProvider::AzureDevOps
        }
        Some(host) if host_matches_provider_domain(host, "github.com") => {
            RepositoryProvider::GitHub
        }
        _ => RepositoryProvider::Manual,
    }
}

pub fn repository_identity_from_remote_url(
    remote_url: Option<&str>,
    fallback_name: &str,
) -> (RepositoryProvider, RepositoryIdentity) {
    let parts = remote_url.and_then(remote_url_parts);
    let provider = provider_from_remote_parts(parts);
    let (owner, name) = repository_owner_name(provider, parts, fallback_name);
    (
        provider,
        RepositoryIdentity {
            remote_url: remote_url.map(redact_remote_url),
            owner,
            name,
            default_base: "main".into(),
        },
    )
}

pub fn redact_remote_url(remote_url: &str) -> String {
    let Some((scheme, after_scheme)) = remote_url.split_once("://") else {
        return remote_url.to_string();
    };
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let (authority, rest) = after_scheme.split_at(authority_end);
    let Some((_, host)) = authority.rsplit_once('@') else {
        return remote_url.to_string();
    };

    format!("{scheme}://[redacted]@{host}{rest}")
}

#[derive(Debug, Clone, Copy)]
struct RemoteUrlParts<'a> {
    host: &'a str,
    path: &'a str,
}

fn remote_url_parts(remote_url: &str) -> Option<RemoteUrlParts<'_>> {
    let trimmed = remote_url.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(after_scheme) = trimmed.split_once("://").map(|(_, rest)| rest) {
        let authority_end = after_scheme
            .find(['/', '?', '#'])
            .unwrap_or(after_scheme.len());
        let (authority, path) = after_scheme.split_at(authority_end);
        let authority = authority
            .split(['/', '?', '#'])
            .next()
            .filter(|value| !value.is_empty())?;
        let host_port = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let host = host_port
            .strip_prefix('[')
            .and_then(|value| value.split_once(']').map(|(host, _)| host))
            .unwrap_or_else(|| {
                host_port
                    .split_once(':')
                    .map_or(host_port, |(host, _)| host)
            });
        return (!host.is_empty()).then_some(RemoteUrlParts {
            host,
            path: path.trim_start_matches('/'),
        });
    }

    trimmed
        .split_once(':')
        .map(|(host_part, path)| RemoteUrlParts {
            host: host_part
                .rsplit_once('@')
                .map_or(host_part, |(_, host)| host),
            path,
        })
        .filter(|parts| !parts.host.is_empty())
}

fn host_matches_provider_domain(host: &str, provider_domain: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    let host = host.as_bytes();
    let provider_domain = provider_domain.as_bytes();
    if host.len() < provider_domain.len() {
        return false;
    }

    let domain_start = host.len() - provider_domain.len();
    if !host[domain_start..].eq_ignore_ascii_case(provider_domain) {
        return false;
    }

    domain_start == 0 || host[domain_start - 1] == b'.'
}

fn repository_owner_name(
    provider: RepositoryProvider,
    parts: Option<RemoteUrlParts<'_>>,
    fallback_name: &str,
) -> (String, String) {
    let Some(parts) = parts else {
        return ("unknown".into(), fallback_name.into());
    };

    match provider {
        RepositoryProvider::GitHub => owner_name_from_path(parts.path, fallback_name),
        RepositoryProvider::AzureDevOps => azure_owner_name_from_path(parts.path, fallback_name),
        RepositoryProvider::Manual => ("unknown".into(), fallback_name.into()),
    }
}

fn owner_name_from_path(path: &str, fallback_name: &str) -> (String, String) {
    let mut parts = path.trim_matches('/').split('/');
    let owner = parts.next().filter(|value| !value.is_empty());
    let name = parts.next().filter(|value| !value.is_empty());
    match (owner, name) {
        (Some(owner), Some(name)) => (owner.into(), trim_repo_suffix(name)),
        _ => ("unknown".into(), fallback_name.into()),
    }
}

fn azure_owner_name_from_path(path: &str, fallback_name: &str) -> (String, String) {
    let path = path.trim_matches('/');
    if let Some((owner_path, name)) = path.split_once("/_git/") {
        let owner = owner_path.trim_matches('/');
        return if owner.is_empty() || name.trim().is_empty() {
            ("unknown".into(), fallback_name.into())
        } else {
            (owner.into(), trim_repo_suffix(name))
        };
    }

    let mut parts = path.split('/').filter(|value| !value.is_empty());
    if matches!(parts.clone().next(), Some("v3")) {
        parts.next();
    }
    let org = parts.next();
    let project = parts.next();
    let name = parts.next();
    match (org, project, name) {
        (Some(org), Some(project), Some(name)) => {
            (format!("{org}/{project}"), trim_repo_suffix(name))
        }
        _ => ("unknown".into(), fallback_name.into()),
    }
}

fn trim_repo_suffix(name: &str) -> String {
    name.trim_matches('/')
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".git")
        .trim()
        .to_string()
}
