use super::*;
use std::io::Read;
use std::time::Duration;

pub(super) const NETWORK_TIMEOUT_SECS: u64 = 5;

/// Maximum size for binary release downloads (amplihack + hooks binaries).
const MAX_BINARY_DOWNLOAD_BYTES: usize = 512 * 1024 * 1024; // 512 MiB

/// Maximum number of attempts for retryable HTTP requests.
const HTTP_MAX_ATTEMPTS: u32 = 3;
/// Initial backoff delay in milliseconds (doubles on each retry).
const HTTP_INITIAL_BACKOFF_MS: u64 = 500;

/// Fetch the current HEAD commit SHA for `<owner>/<repo>` on `branch`.
///
/// Uses the GitHub commits API — the full object is large, so we ask for
/// just one commit from the branch ref and extract the sha field. Returns
/// the full 40-character hex SHA on success.
///
/// Used by the "is my framework/recipe-runner up to date?" checks to
/// detect upstream-side changes without reaching for the heavier releases
/// endpoint (neither repo publishes tagged releases we can rely on).
pub(crate) fn fetch_branch_head_sha(owner_repo: &str, branch: &str) -> Result<String> {
    let url = format!("https://api.github.com/repos/{owner_repo}/commits/{branch}");
    let body = http_get(&url).with_context(|| format!("failed to query {url}"))?;
    let value: serde_json::Value =
        serde_json::from_slice(&body).with_context(|| format!("invalid JSON from {url}"))?;
    let sha = value
        .get("sha")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("response from {url} is missing a sha field"))?;
    if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("response from {url} returned a malformed sha: {sha}");
    }
    Ok(sha.to_string())
}

pub(super) fn fetch_latest_release() -> Result<UpdateRelease> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let asset_name = expected_archive_name()?;
    let response = http_get(&url)
        .with_context(|| format!("failed to query latest stable release from {GITHUB_REPO}"))?;
    parse_latest_release(response, &asset_name)
}

/// SEC-1: Validate that a download URL belongs to the expected GitHub host.
pub(crate) fn validate_download_url(url: &str) -> Result<()> {
    let allowed_hosts = [
        "https://api.github.com/",
        "https://github.com/",
        "https://objects.githubusercontent.com/",
    ];
    if allowed_hosts.iter().any(|prefix| url.starts_with(prefix)) {
        return Ok(());
    }
    bail!(
        "download URL is not from an allowed GitHub host: {url}. \
        Only https://api.github.com/, https://github.com/, and \
        https://objects.githubusercontent.com/ are trusted."
    )
}

/// Returns `true` when the error is transient and the request should be retried.
pub(super) fn is_retryable_error(error: &ureq::Error) -> bool {
    match error {
        ureq::Error::Status(status, _) => *status == 429 || *status >= 500,
        ureq::Error::Transport(_) => true,
    }
}

/// User-friendly message for GitHub-specific HTTP error codes.
pub(super) fn github_error_message(status: u16, url: &str) -> String {
    match status {
        403 => format!(
            "GitHub returned 403 Forbidden for {url}. \
            This usually means the API rate limit has been exceeded. \
            Set AMPLIHACK_NO_UPDATE_CHECK=1 to skip update checks, \
            or wait ~60 minutes for the rate limit to reset."
        ),
        429 => format!(
            "GitHub rate limit exceeded for {url}. \
            Please wait a few minutes before retrying."
        ),
        500..=599 => format!(
            "Transient server error ({status}) from {url}. \
            The request will be retried automatically."
        ),
        _ => format!("HTTP {status} from {url}"),
    }
}

/// Calls [`http_get`] with exponential backoff for transient errors.
pub(crate) fn http_get_with_retry(url: &str) -> Result<Vec<u8>> {
    let mut delay_ms = HTTP_INITIAL_BACKOFF_MS;
    for attempt in 1..=HTTP_MAX_ATTEMPTS {
        match http_get(url) {
            Ok(body) => return Ok(body),
            Err(err) => {
                let is_retryable = err
                    .downcast_ref::<ureq::Error>()
                    .map(is_retryable_error)
                    .unwrap_or(true);

                if !is_retryable || attempt == HTTP_MAX_ATTEMPTS {
                    return Err(err);
                }

                tracing::debug!(
                    attempt,
                    delay_ms,
                    url,
                    error = %err,
                    "HTTP request failed; retrying after backoff"
                );
                std::thread::sleep(Duration::from_millis(delay_ms));
                delay_ms *= 2;
            }
        }
    }
    unreachable!("loop exits via return inside the body")
}

pub(crate) fn http_get(url: &str) -> Result<Vec<u8>> {
    // SEC-1: Reject URLs from unexpected hosts before making any network call.
    validate_download_url(url)?;

    let timeout = Duration::from_secs(NETWORK_TIMEOUT_SECS);
    let response = match ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .build()
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", &format!("amplihack/{}", crate::VERSION))
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) if url.ends_with("/releases/latest") => {
            bail!("no stable v* release has been published for {GITHUB_REPO} yet")
        }
        Err(ureq::Error::Status(status @ (403 | 429), _)) => {
            bail!("{}", github_error_message(status, url))
        }
        Err(error) => return Err(anyhow!("HTTP request failed for {url}: {error}")),
    };

    let limit = MAX_BINARY_DOWNLOAD_BYTES;
    let capacity = response
        .header("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(64 * 1024)
        .min(limit);
    let mut body = Vec::with_capacity(capacity);
    response
        .into_reader()
        .take(limit as u64)
        .read_to_end(&mut body)
        .with_context(|| format!("failed to read HTTP response from {url}"))?;

    if body.len() == limit {
        bail!(
            "HTTP response from {url} exceeded the size limit of {limit} bytes; aborting to prevent OOM"
        );
    }

    Ok(body)
}

pub(super) fn parse_latest_release(body: Vec<u8>, asset_name: &str) -> Result<UpdateRelease> {
    let release: GithubRelease =
        serde_json::from_slice(&body).context("failed to parse GitHub release JSON")?;

    if release.draft {
        bail!("latest release is unexpectedly marked as draft");
    }
    if release.prerelease {
        bail!("latest stable release endpoint returned a prerelease");
    }

    let version = normalize_tag(&release.tag_name)?;

    let checksum_asset_name = format!("{asset_name}.sha256");
    let mut found_asset: Option<&GithubAsset> = None;
    let mut checksum_url: Option<String> = None;
    for a in &release.assets {
        if a.name == asset_name {
            found_asset = Some(a);
        } else if a.name == checksum_asset_name {
            checksum_url = Some(a.browser_download_url.clone());
        }
        if found_asset.is_some() && checksum_url.is_some() {
            break;
        }
    }
    let asset = found_asset.ok_or_else(|| {
        anyhow!(
            "release {} does not contain asset {}",
            release.tag_name,
            asset_name
        )
    })?;

    Ok(UpdateRelease {
        version,
        asset_url: asset.browser_download_url.clone(),
        checksum_url,
    })
}
