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
    let (authority, rest) = after_scheme.split_at(authority_end_for_redact(after_scheme));
    // A backslash makes the userinfo/host split ambiguous (issue #1642), so
    // hide the whole authority rather than guess which part is the secret.
    if authority.contains('\\') && authority.contains('@') {
        return format!("{scheme}://[redacted]{rest}");
    }
    let Some((_, host)) = authority.rsplit_once('@') else {
        return remote_url.to_string();
    };

    format!("{scheme}://[redacted]@{host}{rest}")
}

/// Redaction keeps the historical `/?#`-wide authority so it never prints a
/// byte it hid before #1642.
fn authority_end_for_redact(after_scheme: &str) -> usize {
    after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len())
}

/// Parsing ends the authority at `\` too, as git does, so text after a
/// backslash can never be read as the host.
fn authority_end_for_parse(after_scheme: &str) -> usize {
    after_scheme
        .find(['/', '?', '#', '\\'])
        .unwrap_or(after_scheme.len())
}

/// Splits `userinfo@host` at the FIRST `@`. Userinfo, when present, must be
/// non-empty and use only allow-listed bytes, and the host must not contain
/// another `@`; otherwise `None`.
fn split_userinfo(authority: &str) -> Option<&str> {
    let Some((userinfo, host)) = authority.split_once('@') else {
        return Some(authority);
    };
    let valid =
        !userinfo.is_empty() && userinfo.bytes().all(is_userinfo_byte) && !host.contains('@');
    valid.then_some(host)
}

fn is_userinfo_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"._~%!$&'()*+,;=:-".contains(&byte)
}

#[derive(Debug, Clone, Copy)]
struct RemoteUrlParts<'a> {
    host: &'a str,
    path: &'a str,
}

/// No trim (issue #1642): surrounding whitespace is never part of a GitHub
/// origin, so it is rejected as in the bash parser. Callers trim at the I/O
/// boundary.
fn remote_url_parts(remote_url: &str) -> Option<RemoteUrlParts<'_>> {
    if remote_url.is_empty() {
        return None;
    }

    if let Some((scheme, after_scheme)) = remote_url.split_once("://") {
        if !is_scheme(scheme) {
            return None;
        }
        let (authority, path) = after_scheme.split_at(authority_end_for_parse(after_scheme));
        if authority.is_empty() {
            return None;
        }
        let host_port = split_userinfo(authority)?;
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
            // One `/` separates authority and path; a second would be an
            // empty owner segment, so leave it for the shape check.
            path: path.strip_prefix('/').unwrap_or(path),
        });
    }

    let (host_part, path) = remote_url.split_once(':')?;
    // scp form: a leading `-` would reach ssh as an option, and a backslash
    // makes the userinfo/host split ambiguous; neither is ever a provider.
    if host_part.starts_with('-') || host_part.contains('\\') {
        return None;
    }
    let host = split_userinfo(host_part)?;
    (!host.is_empty()).then_some(RemoteUrlParts { host, path })
}

/// RFC 3986: `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`, so leading
/// whitespace or other bytes never pass as a scheme.
fn is_scheme(scheme: &str) -> bool {
    scheme
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
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

/// Mirrors the bash parser: cut `?` and `#`, strip one trailing `/`, require
/// `owner/repo` or `owner/repo/pull/<n>`, strip one `.git` from the repo, and
/// check both segments with no trim. Failure gives `("unknown", fallback)`.
fn owner_name_from_path(path: &str, fallback_name: &str) -> (String, String) {
    let path = path.split(['?', '#']).next().unwrap_or("");
    let path = path.strip_suffix('/').unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts
        .next()
        .filter(|value| is_identity_segment(value) && !value.starts_with('-'));
    let raw_name = parts.next().unwrap_or("");
    // Path shape (issue #1642): only `owner/repo` or `owner/repo/pull/<n>`
    // names a repository; anything else (`extra`, `tree/main`, `../../x/y`,
    // an empty segment) could resolve to another repository once dot
    // segments are removed. `.git` is only a suffix of the whole path, so
    // `r.git/pull/5` is refused.
    let name = match parts.collect::<Vec<_>>().as_slice() {
        [] => Some(raw_name.strip_suffix(".git").unwrap_or(raw_name)),
        ["pull", number] if is_pr_number(number) && !raw_name.ends_with(".git") => Some(raw_name),
        _ => None,
    };
    // Checked after `.git` stripping, so `..git` (-> `.`) is refused while
    // `.git.git` (-> `.git`) is a valid name, as in bash.
    match (owner, name.filter(|value| is_identity_segment(value))) {
        (Some(owner), Some(name)) => (owner.into(), name.into()),
        _ => ("unknown".into(), fallback_name.into()),
    }
}

/// `^[1-9][0-9]{0,9}$`, as in the bash parser.
fn is_pr_number(number: &str) -> bool {
    (1..=10).contains(&number.len())
        && !number.starts_with('0')
        && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// Same rule as the bash parser: `[A-Za-z0-9._-]+`, never `.` or `..`.
fn is_identity_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
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
