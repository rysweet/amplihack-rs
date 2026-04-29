mod check;
mod install;
mod network;
mod post_install;

pub use check::{
    StartupUpdateOutcome, maybe_print_update_notice_from_args, run_update,
    should_skip_update_check_for_subcommand,
};
pub(crate) use install::extract_archive;
pub(crate) use network::{fetch_branch_head_sha, http_get, validate_download_url};

use anyhow::{Result, anyhow, bail};
use semver::Version;
use serde::Deserialize;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const CURRENT_VERSION: &str = crate::VERSION;
const GITHUB_REPO: &str = "rysweet/amplihack-rs";
const NO_UPDATE_CHECK_ENV: &str = "AMPLIHACK_NO_UPDATE_CHECK";
const UPDATE_CACHE_RELATIVE_PATH: &str = ".config/amplihack/last_update_check";
const UPDATE_CHECK_COOLDOWN_SECS: u64 = 24 * 60 * 60;

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
    /// URL for the `.sha256` checksum file accompanying the archive.
    /// When present, the downloaded archive is verified before installation.
    checksum_url: Option<String>,
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
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("x86_64-pc-windows-msvc")
    } else {
        None
    }
}

fn required_release_target() -> Result<&'static str> {
    supported_release_target().ok_or_else(|| {
        anyhow!(
            "self-update is only supported on published release targets (linux/macos x86_64 and aarch64; windows x86_64)"
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

/// Cache format: `<cached_version>\n<checked_at_unix>\n<exe_mtime_unix>\n`.
///
/// The exe_mtime line is optional for backward compatibility with caches
/// written by older builds. When present, a mismatch against the current
/// binary's mtime signals the binary was swapped (self-update, package
/// manager reinstall, manual replace) and the cache should not be trusted.
fn read_cache(path: &Path) -> Option<(String, u64)> {
    let content = fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    let version = lines.next()?.to_string();
    let timestamp: u64 = lines.next()?.parse().ok()?;
    let cached_exe_mtime: Option<u64> = lines.next().and_then(|line| line.parse().ok());

    // If the binary's mtime has changed since the cache was written, the
    // entry was authored by a different installed version — ignore it.
    if let Some(cached) = cached_exe_mtime
        && let Some(current) = current_exe_mtime_secs()
        && cached != current
    {
        return None;
    }
    Some((version, timestamp))
}

fn write_cache(path: &Path, version: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mtime_line = current_exe_mtime_secs()
        .map(|v| v.to_string())
        .unwrap_or_default();
    fs::write(
        path,
        format!("{}\n{}\n{}\n", version, now_secs(), mtime_line),
    )
    .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Read the mtime of the currently-running executable, in seconds since the
/// UNIX epoch. Returns `None` if the path can't be determined or stat'd —
/// callers treat `None` as "mtime unknown, don't key the cache on it" so we
/// degrade gracefully to the old behavior.
fn current_exe_mtime_secs() -> Option<u64> {
    let exe = std::env::current_exe().ok()?;
    let metadata = fs::metadata(&exe).ok()?;
    let modified = metadata.modified().ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

use anyhow::Context;

#[cfg(test)]
mod tests;
