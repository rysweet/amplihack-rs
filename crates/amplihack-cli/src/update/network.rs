use super::gh::GhCommand;
use super::*;
use std::io::Read;
use std::time::{Duration, Instant};

pub(super) const NETWORK_TIMEOUT_SECS: u64 = 5;

/// Maximum size for binary release downloads (amplihack + hooks binaries).
pub(super) const MAX_BINARY_DOWNLOAD_BYTES: usize = 512 * 1024 * 1024; // 512 MiB

/// Maximum size of a release tag tarball (issue #1955, SEC-3).
pub(crate) const MAX_TAG_TARBALL_BYTES: u64 = 256 * 1024 * 1024;

/// Maximum number of attempts for retryable HTTP requests.
const HTTP_MAX_ATTEMPTS: u32 = 3;
/// Initial backoff delay in milliseconds (doubles on each retry).
const HTTP_INITIAL_BACKOFF_MS: u64 = 500;

#[derive(Debug, thiserror::Error)]
enum HttpClientError {
    #[error("no stable v* release has been published for {repo} yet")]
    LatestReleaseNotFound { repo: &'static str },
    #[error("{message}")]
    Status {
        status: u16,
        url: String,
        message: String,
    },
    #[error("HTTP request failed for {url}: {source}")]
    Transport {
        url: String,
        source: Box<ureq::Error>,
    },
    #[error("failed to read HTTP response from {url}: {source}")]
    Read { url: String, source: std::io::Error },
}

impl HttpClientError {
    fn is_retryable(&self) -> bool {
        match self {
            Self::LatestReleaseNotFound { .. } => false,
            Self::Status { status, .. } => *status >= 500,
            Self::Transport { .. } | Self::Read { .. } => true,
        }
    }
}

pub(super) fn fetch_latest_release() -> Result<UpdateRelease> {
    if let Some(release) = fake_latest_release()? {
        return Ok(release);
    }
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    fetch_latest_release_with(&GhCommand::default(), &url, http_get_with_retry)
}

/// Issue #625 test hook: when AMPLIHACK_TEST_FAKE_LATEST_VERSION is set to a
/// non-empty value, return a synthetic UpdateRelease without any network call
/// or gh probe. This enables deterministic integration tests of the
/// startup-update prompt path under fixed-version conditions.
///
/// Empty value falls through to the normal production path so an
/// exported-but-empty env var (`export AMPLIHACK_TEST_FAKE_LATEST_VERSION=`)
/// does not silently disable real update checks.
fn fake_latest_release() -> Result<Option<UpdateRelease>> {
    let Some(raw) = std::env::var_os("AMPLIHACK_TEST_FAKE_LATEST_VERSION") else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let tag = raw.to_string_lossy().into_owned();
    let version = normalize_tag(&tag).with_context(|| {
        format!("AMPLIHACK_TEST_FAKE_LATEST_VERSION is not valid semver: {tag}")
    })?;
    Ok(Some(UpdateRelease {
        tag: format!("v{version}"),
        version,
        asset_url: String::new(),
        checksum_url: None,
    }))
}

/// Issue #2957: looks the latest release up through the user's own gh when
/// `gh auth status` succeeds, and with the anonymous request otherwise. Once
/// gh is usable, a gh failure is reported as it is: falling back would hide
/// an authenticated failure behind the anonymous allowance that is already
/// exhausted.
pub(super) fn fetch_latest_release_with(
    gh: &GhCommand,
    api_url: &str,
    fetch: impl Fn(&str) -> Result<Vec<u8>>,
) -> Result<UpdateRelease> {
    let asset_name = expected_archive_name()?;
    let body = if gh.usable() {
        latest_release_via_gh(gh)?
    } else {
        fetch(api_url)
            .with_context(|| format!("failed to query latest stable release from {GITHUB_REPO}"))?
    };
    parse_latest_release(body, &asset_name)
}

fn latest_release_via_gh(gh: &GhCommand) -> Result<Vec<u8>> {
    let response = gh
        .latest_release_json()
        .context("failed to query latest stable release through gh")?;
    let url = format!("gh api repos/{GITHUB_REPO}/releases/latest");
    match response.status {
        200..=299 => Ok(response.body),
        404 => Err(HttpClientError::LatestReleaseNotFound { repo: GITHUB_REPO }.into()),
        status @ (403 | 429) => {
            let github = json_message(&response.body);
            Err(HttpClientError::Status {
                status,
                url,
                message: refusal_message(status, github.as_deref(), response.reset),
            }
            .into())
        }
        status => {
            let github = json_message(&response.body)
                .map(|m| sanitise_remote_text(&m))
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "GitHub gave no message".to_string());
            Err(HttpClientError::Status {
                status,
                url,
                message: format!("gh api returned HTTP {status}: {github}"),
            }
            .into())
        }
    }
}

/// GitHub's own `message` from a JSON error body, if there is one.
fn json_message(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value.get("message")?.as_str().map(str::to_string)
}

/// Makes text from GitHub or gh safe to show: the first line only, control
/// characters (including ESC) removed, at most 300 characters, and no trailing
/// full stop, so it can sit inside a sentence.
pub(crate) fn sanitise_remote_text(text: &str) -> String {
    let first = text.trim().lines().next().unwrap_or_default();
    let cleaned: String = first
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect();
    let cleaned = cleaned.trim();
    cleaned.strip_suffix('.').unwrap_or(cleaned).to_string()
}

/// Issue #2957: the message for a 403 or 429 from GitHub, on either path. It
/// gives GitHub's own message and when the limit resets, and does not suggest
/// waiting or retrying.
pub(crate) fn refusal_message(status: u16, github: Option<&str>, reset: Option<i64>) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    refusal_message_at(status, github, reset, now)
}

pub(super) fn refusal_message_at(
    status: u16,
    github: Option<&str>,
    reset: Option<i64>,
    now: i64,
) -> String {
    let github = github.map(sanitise_remote_text).filter(|m| !m.is_empty());
    let mut message = match &github {
        Some(text) => format!("GitHub refused the request (HTTP {status}): {text}."),
        None => format!("GitHub refused the request (HTTP {status})."),
    };
    let reset_at =
        reset.and_then(|secs| chrono::DateTime::from_timestamp(secs, 0).map(|at| (secs, at)));
    match reset_at {
        Some((secs, at)) => {
            let seconds_left = secs.saturating_sub(now);
            let when = if seconds_left < 60 {
                "in under a minute".to_string()
            } else if seconds_left < 120 {
                "in 1 minute".to_string()
            } else {
                format!("in {} minutes", seconds_left / 60)
            };
            message.push_str(&format!(
                " The limit resets at {} UTC ({when}).",
                at.format("%H:%M")
            ));
        }
        None => message.push_str(" GitHub did not give a reset time."),
    }
    let mentions_rate_limit = github
        .as_deref()
        .is_some_and(|m| m.to_ascii_lowercase().contains("rate limit"));
    if !mentions_rate_limit {
        message.push_str(" This is usually GitHub's API rate limit.");
    }
    message.push_str(" Set AMPLIHACK_NO_UPDATE_CHECK=1 to skip the update check at launch.");
    message
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

/// Issue #1955 (SEC-1): the hosts a release tag tarball may come from, checked
/// on the first URL and on the final one after redirects. GitHub redirects a
/// tag tarball to `codeload.github.com`, which only this download may use.
pub(crate) fn validate_bounded_download_url(url: &str) -> Result<()> {
    let allowed_hosts = ["https://github.com/", "https://codeload.github.com/"];
    if allowed_hosts.iter().any(|prefix| url.starts_with(prefix)) {
        return Ok(());
    }
    bail!(
        "download URL is not from an allowed host: {url}. Only https://github.com/ and \
        https://codeload.github.com/ are trusted for a release tag download."
    )
}

/// HTTPS download with an overall time limit shared across its retries, and a
/// check of the final URL before any of the body is read (issue #1955).
pub(crate) fn http_get_bounded(url: &str, budget: Duration) -> Result<Vec<u8>> {
    validate_bounded_download_url(url)?;
    let deadline = Instant::now() + budget;
    let mut delay = Duration::from_millis(HTTP_INITIAL_BACKOFF_MS);
    for attempt in 1..=HTTP_MAX_ATTEMPTS {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("the time limit ran out before {url} finished downloading");
        }
        match http_get_bounded_once(url, remaining) {
            Ok(body) => return Ok(body),
            Err(err) => {
                let time_left = deadline.saturating_duration_since(Instant::now());
                if !is_retryable_http_get_error(&err)
                    || attempt == HTTP_MAX_ATTEMPTS
                    || delay >= time_left
                {
                    return Err(err);
                }
                tracing::debug!(attempt, url, error = %err, "bounded download failed; retrying");
                std::thread::sleep(delay);
                delay *= 2;
            }
        }
    }
    unreachable!("loop exits via return inside the body")
}

/// Not [`github_error_message`]: its advice is about update checks, and this
/// text ends up in an install's warning after the retries have run.
pub(super) fn bounded_status_message(status: u16, url: &str) -> String {
    format!("HTTP {status} from {url}")
}

fn http_get_bounded_once(url: &str, timeout: Duration) -> Result<Vec<u8>> {
    let response = match ureq::AgentBuilder::new()
        .https_only(true)
        // Overall: connection, redirects and reading the body.
        .timeout(timeout)
        .build()
        .get(url)
        .set("User-Agent", &format!("amplihack/{}", crate::VERSION))
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(status, _)) => {
            return Err(HttpClientError::Status {
                status,
                url: url.to_string(),
                message: bounded_status_message(status, url),
            }
            .into());
        }
        Err(error) => {
            return Err(HttpClientError::Transport {
                url: url.to_string(),
                source: Box::new(error),
            }
            .into());
        }
    };
    validate_bounded_download_url(response.get_url())
        .context("a release tag download redirected to a host that isn't allowed")?;

    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_TAG_TARBALL_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|source| HttpClientError::Read {
            url: url.to_string(),
            source,
        })?;
    if body.len() as u64 > MAX_TAG_TARBALL_BYTES {
        bail!("the download from {url} exceeded the limit of {MAX_TAG_TARBALL_BYTES} bytes");
    }
    Ok(body)
}

/// Returns `true` when the error is transient and the request should be retried.
pub(super) fn is_retryable_error(error: &ureq::Error) -> bool {
    match error {
        ureq::Error::Status(status, _) => *status >= 500,
        ureq::Error::Transport(_) => true,
    }
}

pub(super) fn is_retryable_http_get_error(error: &anyhow::Error) -> bool {
    if let Some(error) = error.downcast_ref::<HttpClientError>() {
        return error.is_retryable();
    }
    if let Some(error) = error.downcast_ref::<ureq::Error>() {
        return is_retryable_error(error);
    }
    false
}

/// User-friendly message for GitHub-specific HTTP error codes.
pub(super) fn github_error_message(status: u16, url: &str) -> String {
    match status {
        403 | 429 => refusal_message(status, None, None),
        500..=599 => format!(
            "Transient server error ({status}) from {url}. \
            The request will be retried automatically."
        ),
        _ => format!("HTTP {status} from {url}"),
    }
}

/// Calls [`http_get`] with exponential backoff for transient errors.
pub(crate) fn http_get_with_retry(url: &str) -> Result<Vec<u8>> {
    retry_with(url, http_get)
}

/// [`http_get_with_retry`] without the host check, so tests can use a loopback
/// stand-in API. Test builds only; production always validates the host.
#[cfg(test)]
pub(super) fn http_get_with_retry_unvalidated(url: &str) -> Result<Vec<u8>> {
    retry_with(url, http_get_unvalidated)
}

fn retry_with(url: &str, get: fn(&str) -> Result<Vec<u8>>) -> Result<Vec<u8>> {
    let mut delay_ms = HTTP_INITIAL_BACKOFF_MS;
    for attempt in 1..=HTTP_MAX_ATTEMPTS {
        match get(url) {
            Ok(body) => return Ok(body),
            Err(err) => {
                let is_retryable = is_retryable_http_get_error(&err);

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

fn http_get(url: &str) -> Result<Vec<u8>> {
    // SEC-1: Reject URLs from unexpected hosts before making any network call.
    validate_download_url(url)?;
    http_get_unvalidated(url)
}

fn http_get_unvalidated(url: &str) -> Result<Vec<u8>> {
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
            return Err(HttpClientError::LatestReleaseNotFound { repo: GITHUB_REPO }.into());
        }
        // Issue #2957: report GitHub's own message and the reset time.
        Err(ureq::Error::Status(status @ (403 | 429), response)) => {
            let reset = response
                .header("x-ratelimit-reset")
                .and_then(|v| v.trim().parse::<i64>().ok());
            let github = response
                .into_string()
                .ok()
                .and_then(|body| json_message(body.as_bytes()));
            return Err(HttpClientError::Status {
                status,
                url: url.to_string(),
                message: refusal_message(status, github.as_deref(), reset),
            }
            .into());
        }
        Err(ureq::Error::Status(status, _)) => {
            return Err(HttpClientError::Status {
                status,
                url: url.to_string(),
                message: github_error_message(status, url),
            }
            .into());
        }
        Err(error) => {
            return Err(HttpClientError::Transport {
                url: url.to_string(),
                source: Box::new(error),
            }
            .into());
        }
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
        .map_err(|source| HttpClientError::Read {
            url: url.to_string(),
            source,
        })?;

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
        tag: release.tag_name.clone(),
        version,
        asset_url: asset.browser_download_url.clone(),
        checksum_url,
    })
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    #[test]
    fn retry_classifier_preserves_structured_http_status_semantics() {
        for (status, retryable) in [(403, false), (404, false), (429, false), (500, true)] {
            let url = "https://api.github.com/repos/rysweet/amplihack-rs/releases/latest";
            let err: anyhow::Error = HttpClientError::Status {
                status,
                url: url.to_string(),
                message: github_error_message(status, url),
            }
            .into();
            assert_eq!(
                is_retryable_http_get_error(&err),
                retryable,
                "unexpected retry classification for HTTP {status}"
            );
        }
    }

    #[test]
    fn retry_classifier_does_not_retry_permanent_local_errors() {
        let err: anyhow::Error =
            HttpClientError::LatestReleaseNotFound { repo: GITHUB_REPO }.into();
        assert!(!is_retryable_http_get_error(&err));

        let err = anyhow!("download URL is not from an allowed GitHub host");
        assert!(!is_retryable_http_get_error(&err));
    }
}
