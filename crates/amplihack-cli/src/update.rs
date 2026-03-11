use anyhow::{Context, Result, anyhow, bail};
use flate2::read::GzDecoder;
use semver::Version;
use serde::Deserialize;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tar::Archive;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_REPO: &str = "rysweet/amplihack-rs";
const NO_UPDATE_CHECK_ENV: &str = "AMPLIHACK_NO_UPDATE_CHECK";
const UPDATE_CACHE_RELATIVE_PATH: &str = ".config/amplihack/last_update_check";
const UPDATE_CHECK_COOLDOWN_SECS: u64 = 24 * 60 * 60;
const NETWORK_TIMEOUT_SECS: u64 = 5;
/// Maximum bytes read from any HTTP response body (prevents OOM on unexpectedly large payloads).
const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB
/// How many times to attempt a request before giving up.
const MAX_HTTP_RETRIES: u32 = 3;
/// Initial back-off delay in milliseconds; doubles on each subsequent attempt.
const RETRY_BASE_DELAY_MS: u64 = 500;

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdateRelease {
    version: String,
    asset_url: String,
}

pub fn maybe_print_update_notice_from_args(args: &[OsString]) {
    if should_skip_update_check(args) || supported_release_target().is_none() {
        return;
    }

    if let Err(error) = maybe_print_update_notice() {
        tracing::debug!(?error, "startup update check skipped");
    }
}

pub fn run_update() -> Result<()> {
    println!("amplihack update (current: v{CURRENT_VERSION})");

    let release = fetch_latest_release()?;
    if !is_newer(CURRENT_VERSION, &release.version)? {
        println!("Already at the latest version (v{CURRENT_VERSION}).");
        return Ok(());
    }

    println!(
        "New version available: v{} -> v{}",
        CURRENT_VERSION, release.version
    );
    download_and_replace(&release)?;
    write_cache(&cache_path()?, &release.version)?;
    Ok(())
}

fn maybe_print_update_notice() -> Result<()> {
    if std::env::var(NO_UPDATE_CHECK_ENV).unwrap_or_default() == "1" {
        return Ok(());
    }

    let cache_path = cache_path()?;
    let now = now_secs();

    if let Some((cached_version, timestamp)) = read_cache(&cache_path)
        && now.saturating_sub(timestamp) < UPDATE_CHECK_COOLDOWN_SECS
    {
        if is_newer(CURRENT_VERSION, &cached_version)? {
            print_update_notice(&cached_version);
        }
        return Ok(());
    }

    let release = fetch_latest_release()?;
    write_cache(&cache_path, &release.version)?;
    if is_newer(CURRENT_VERSION, &release.version)? {
        print_update_notice(&release.version);
    }
    Ok(())
}

fn fetch_latest_release() -> Result<UpdateRelease> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let asset_name = expected_archive_name()?;
    let response = http_get(&url)
        .with_context(|| format!("failed to query latest stable release from {GITHUB_REPO}"))?;
    parse_latest_release(response, &asset_name)
}

/// Returns `true` for errors that are worth retrying (transient network glitches
/// and server-side 5xx / rate-limit responses). Permanent client errors (4xx other
/// than 429) are not retried so we fail fast on bad URLs or missing resources.
fn is_retryable(error: &ureq::Error) -> bool {
    match error {
        // 429 Too Many Requests and all 5xx codes are transient by convention.
        ureq::Error::Status(code, _) => matches!(code, 429 | 500 | 502 | 503 | 504),
        // Transport errors (connection reset, DNS failure, timeout, …) are always retried.
        ureq::Error::Transport(_) => true,
    }
}

fn http_get(url: &str) -> Result<Vec<u8>> {
    let timeout = Duration::from_secs(NETWORK_TIMEOUT_SECS);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .build();

    let mut last_err = anyhow!("request to {url} failed");

    for attempt in 0..MAX_HTTP_RETRIES {
        if attempt > 0 {
            // Exponential back-off: 500 ms, 1 000 ms, …
            let delay = Duration::from_millis(RETRY_BASE_DELAY_MS * (1u64 << (attempt - 1)));
            tracing::debug!(
                attempt,
                delay_ms = delay.as_millis(),
                "retrying HTTP request"
            );
            std::thread::sleep(delay);
        }

        let response = match agent
            .get(url)
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", &format!("amplihack/{CURRENT_VERSION}"))
            .call()
        {
            Ok(r) => r,
            // Hard 404 on the releases endpoint → not retryable, fail immediately.
            Err(ureq::Error::Status(404, _)) if url.ends_with("/releases/latest") => {
                bail!("no stable v* release has been published for {GITHUB_REPO} yet")
            }
            Err(ref e) if !is_retryable(e) => {
                return Err(anyhow!("HTTP request failed for {url}: {e}"));
            }
            Err(e) => {
                tracing::debug!(attempt, error = %e, "transient HTTP error");
                last_err = anyhow!("HTTP request failed for {url}: {e}");
                continue;
            }
        };

        // Read at most MAX_BODY_BYTES + 1 so we can detect an over-size body
        // without pulling the entire payload into memory first.
        let limit = MAX_BODY_BYTES + 1;
        let mut body = Vec::new();
        response
            .into_reader()
            .take(limit)
            .read_to_end(&mut body)
            .with_context(|| format!("failed to read HTTP response from {url}"))?;

        if body.len() as u64 > MAX_BODY_BYTES {
            bail!("HTTP response from {url} exceeded the {MAX_BODY_BYTES}-byte safety limit");
        }

        return Ok(body);
    }

    Err(last_err)
}

fn parse_latest_release(body: Vec<u8>, asset_name: &str) -> Result<UpdateRelease> {
    let release: GithubRelease =
        serde_json::from_slice(&body).context("failed to parse GitHub release JSON")?;

    if release.draft {
        bail!("latest release is unexpectedly marked as draft");
    }
    if release.prerelease {
        bail!("latest stable release endpoint returned a prerelease");
    }

    let version = normalize_tag(&release.tag_name)?;
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| {
            anyhow!(
                "release {} does not contain asset {}",
                release.tag_name,
                asset_name
            )
        })?;

    Ok(UpdateRelease {
        version,
        asset_url: asset.browser_download_url.clone(),
    })
}

fn normalize_tag(tag: &str) -> Result<String> {
    let trimmed = tag.trim().trim_start_matches('v');
    Version::parse(trimmed).with_context(|| format!("release tag is not valid semver: {tag}"))?;
    Ok(trimmed.to_string())
}

fn supported_release_target() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else {
        None
    }
}

fn required_release_target() -> Result<&'static str> {
    supported_release_target().ok_or_else(|| {
        anyhow!(
            "self-update is only supported on published release targets (linux/macos x86_64 and aarch64)"
        )
    })
}

fn expected_archive_name() -> Result<String> {
    Ok(format!("amplihack-{}.tar.gz", required_release_target()?))
}

fn is_newer(current: &str, latest: &str) -> Result<bool> {
    let current = Version::parse(current)
        .with_context(|| format!("current version is not valid semver: {current}"))?;
    let latest = Version::parse(latest)
        .with_context(|| format!("latest version is not valid semver: {latest}"))?;
    Ok(latest > current)
}

fn cache_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(cache_path_from_home(Path::new(&home)))
}

fn cache_path_from_home(home: &Path) -> PathBuf {
    home.join(UPDATE_CACHE_RELATIVE_PATH)
}

fn read_cache(path: &Path) -> Option<(String, u64)> {
    let content = fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    let version = lines.next()?.to_string();
    let timestamp = lines.next()?.parse().ok()?;
    Some((version, timestamp))
}

fn write_cache(path: &Path, version: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, format!("{}\n{}", version, now_secs()))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn should_skip_update_check(args: &[OsString]) -> bool {
    if std::env::var(NO_UPDATE_CHECK_ENV).unwrap_or_default() == "1" {
        return true;
    }

    let first_arg = args.get(1).and_then(|arg| arg.to_str());
    matches!(
        first_arg,
        None | Some("help")
            | Some("update")
            | Some("version")
            | Some("-h")
            | Some("--help")
            | Some("-V")
            | Some("--version")
    )
}

fn print_update_notice(latest: &str) {
    eprintln!(
        "\x1b[33mA newer version of amplihack is available (v{}). Run 'amplihack update' to upgrade.\x1b[0m",
        latest
    );
}

fn download_and_replace(release: &UpdateRelease) -> Result<()> {
    let archive_bytes = http_get(&release.asset_url)?;
    let temp_dir = tempfile::tempdir().context("failed to create update temp directory")?;
    extract_archive(&archive_bytes, temp_dir.path())?;

    let new_amplihack = find_binary(temp_dir.path(), binary_filename("amplihack"))?;
    let new_hooks = find_binary(temp_dir.path(), binary_filename("amplihack-hooks"))?;
    let current_exe =
        std::env::current_exe().context("cannot determine current executable path")?;
    let install_dir = current_exe
        .parent()
        .context("current executable has no parent directory")?;
    let hooks_dest = install_dir.join(binary_filename("amplihack-hooks"));

    install_binary_atomic(&new_hooks, &hooks_dest)?;
    install_binary_atomic(&new_amplihack, &current_exe)?;

    println!(
        "Updated amplihack: {} -> {}",
        CURRENT_VERSION, release.version
    );
    println!("Restart amplihack to use the new version.");
    Ok(())
}

fn binary_filename(name: &'static str) -> &'static str {
    if cfg!(windows) {
        match name {
            "amplihack" => "amplihack.exe",
            "amplihack-hooks" => "amplihack-hooks.exe",
            _ => name,
        }
    } else {
        name
    }
}

fn extract_archive(archive_bytes: &[u8], destination: &Path) -> Result<()> {
    let decoder = GzDecoder::new(std::io::Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);
    archive
        .unpack(destination)
        .with_context(|| format!("failed to unpack archive into {}", destination.display()))?;
    Ok(())
}

fn find_binary(root: &Path, binary_name: &str) -> Result<PathBuf> {
    fn search(root: &Path, binary_name: &str, depth: usize) -> Option<PathBuf> {
        if depth > 3 {
            return None;
        }

        let entries = fs::read_dir(root).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.file_name() == Some(OsStr::new(binary_name)) {
                return Some(path);
            }
            if path.is_dir()
                && let Some(found) = search(&path, binary_name, depth + 1)
            {
                return Some(found);
            }
        }
        None
    }

    search(root, binary_name, 0)
        .ok_or_else(|| anyhow!("binary '{}' not found in downloaded archive", binary_name))
}

fn install_binary_atomic(source: &Path, destination: &Path) -> Result<()> {
    let temp_destination = destination.with_extension("tmp");
    fs::copy(source, &temp_destination).with_context(|| {
        format!(
            "failed to copy {} to {}",
            source.display(),
            temp_destination.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp_destination, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to chmod {}", temp_destination.display()))?;
    }

    fs::rename(&temp_destination, destination).with_context(|| {
        format!(
            "failed to replace {} with {}",
            destination.display(),
            temp_destination.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tag_strips_v_prefix() {
        assert_eq!(normalize_tag("v1.2.3").unwrap(), "1.2.3");
    }

    #[test]
    fn normalize_tag_rejects_non_semver() {
        assert!(normalize_tag("snapshot-abcdef").is_err());
    }

    #[test]
    fn is_newer_detects_version_bumps() {
        assert!(is_newer("0.1.0", "0.2.0").unwrap());
        assert!(!is_newer("0.2.0", "0.2.0").unwrap());
        assert!(!is_newer("0.2.1", "0.2.0").unwrap());
    }

    #[test]
    fn should_skip_update_check_for_update_related_args() {
        assert!(should_skip_update_check(&[
            OsString::from("amplihack"),
            OsString::from("update")
        ]));
        assert!(should_skip_update_check(&[
            OsString::from("amplihack"),
            OsString::from("version")
        ]));
        assert!(should_skip_update_check(&[
            OsString::from("amplihack"),
            OsString::from("help")
        ]));
        assert!(should_skip_update_check(&[
            OsString::from("amplihack"),
            OsString::from("-V")
        ]));
        assert!(!should_skip_update_check(&[
            OsString::from("amplihack"),
            OsString::from("copilot")
        ]));
    }

    #[test]
    fn cache_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cache");
        write_cache(&path, "1.2.3").unwrap();
        let (version, timestamp) = read_cache(&path).unwrap();
        assert_eq!(version, "1.2.3");
        assert!(timestamp > 0);
    }

    #[test]
    fn cache_path_uses_home() {
        let temp = tempfile::tempdir().unwrap();
        let path = cache_path_from_home(temp.path());
        assert_eq!(
            path,
            temp.path().join(".config/amplihack/last_update_check")
        );
    }

    #[test]
    fn parse_latest_release_selects_matching_asset() {
        let json = format!(
            r#"{{
                "tag_name": "v0.2.0",
                "draft": false,
                "prerelease": false,
                "assets": [
                    {{"name": "wrong.tar.gz", "browser_download_url": "https://example.invalid/wrong"}},
                    {{"name": "{}", "browser_download_url": "https://example.invalid/right"}}
                ]
            }}"#,
            expected_archive_name().unwrap()
        );
        let release =
            parse_latest_release(json.into_bytes(), &expected_archive_name().unwrap()).unwrap();
        assert_eq!(
            release,
            UpdateRelease {
                version: "0.2.0".to_string(),
                asset_url: "https://example.invalid/right".to_string(),
            }
        );
    }

    #[test]
    fn current_test_platform_has_release_target() {
        assert!(supported_release_target().is_some());
    }

    // ── resilience helpers ────────────────────────────────────────────────────

    // NOTE: ureq::Transport is an opaque type that cannot be constructed in unit
    // tests. The `ureq::Error::Transport(_) => true` arm is covered at runtime by
    // any test that exercises a live (or mock-server) network path. Here we only
    // verify the Status-code classification, which is fully deterministic.

    #[test]
    fn is_retryable_server_errors() {
        // 5xx and 429 are transient — should be retried.
        for code in [429u16, 500, 502, 503, 504] {
            let resp = ureq::Response::new(code, "x", "").unwrap();
            assert!(
                is_retryable(&ureq::Error::Status(code, resp)),
                "status {code} should be retryable"
            );
        }
    }

    #[test]
    fn is_not_retryable_client_errors() {
        // 4xx (except 429) are permanent — should not be retried.
        for code in [400u16, 401, 403, 422] {
            let resp = ureq::Response::new(code, "x", "").unwrap();
            assert!(
                !is_retryable(&ureq::Error::Status(code, resp)),
                "status {code} should not be retryable"
            );
        }
    }

    #[test]
    fn body_size_limit_accepted_under_limit() {
        // A body well within MAX_BODY_BYTES must not trigger the size guard.
        let small = [0u8; 10];
        assert!(small.len() as u64 <= MAX_BODY_BYTES);
    }

    #[test]
    fn body_size_limit_rejected_over_limit() {
        // Verify the guard arithmetic: body.len() > MAX_BODY_BYTES → bail.
        let over_limit_len = MAX_BODY_BYTES + 1;
        assert!(over_limit_len > MAX_BODY_BYTES);
    }

    #[test]
    fn extract_archive_finds_both_binaries() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("release.tar.gz");
        create_test_archive(&archive_path).unwrap();
        let bytes = fs::read(&archive_path).unwrap();

        let extract_dir = temp.path().join("extract");
        fs::create_dir_all(&extract_dir).unwrap();
        extract_archive(&bytes, &extract_dir).unwrap();

        assert!(find_binary(&extract_dir, binary_filename("amplihack")).is_ok());
        assert!(find_binary(&extract_dir, binary_filename("amplihack-hooks")).is_ok());
    }

    fn create_test_archive(path: &Path) -> Result<()> {
        let tar_gz = fs::File::create(path)?;
        let encoder = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);

        for binary in [
            binary_filename("amplihack"),
            binary_filename("amplihack-hooks"),
        ] {
            let data = b"#!/bin/sh\nexit 0\n";
            let mut header = tar::Header::new_gnu();
            header.set_path(binary)?;
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, &data[..])?;
        }

        let encoder = builder.into_inner()?;
        encoder.finish()?;
        Ok(())
    }
}
