//! Auto-update checking and upgrade orchestration.
//!
//! Checks GitHub releases for newer versions with local file-based caching,
//! prompts the user interactively, and delegates to the Rust CLI binary or
//! falls back to `uv tool upgrade`.

use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

const GITHUB_REPO: &str = "rysweet/amplihack-rs";
const CACHE_FILE_NAME: &str = "update_cache.json";
const DEFAULT_CHECK_INTERVAL_HOURS: u64 = 24;
const DEFAULT_TIMEOUT_SECONDS: u64 = 10;

/// Result of comparing the current version against the latest release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    /// Currently running version.
    pub current_version: String,
    /// Latest version available on GitHub.
    pub latest_version: String,
    /// `true` when `latest_version` is strictly newer than `current_version`.
    pub is_newer: bool,
    /// URL to the GitHub release page.
    pub release_url: String,
}

/// Persistent cache for the last update check timestamp and result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCache {
    /// ISO-8601 timestamp of the last check.
    pub last_check: String,
    /// Version string returned by the last check.
    pub latest_version: String,
    /// How many hours between automatic checks.
    #[serde(default = "default_interval")]
    pub check_interval_hours: u64,
}

fn default_interval() -> u64 {
    DEFAULT_CHECK_INTERVAL_HOURS
}

impl UpdateCache {
    /// Returns `true` when enough time has elapsed since the last check.
    pub fn is_expired(&self) -> bool {
        let Ok(last) = chrono::DateTime::parse_from_rfc3339(&self.last_check) else {
            return true;
        };
        let now = chrono::Utc::now();
        let elapsed = now.signed_duration_since(last);
        elapsed.num_hours() as u64 >= self.check_interval_hours
    }
}

/// Check GitHub for a newer release, respecting the local cache.
///
/// Returns `Some(result)` when a check was performed (regardless of whether
/// a newer version exists). Returns `None` when the cache is still fresh or
/// on network/parse errors.
pub fn check_for_updates(
    current_version: &str,
    cache_dir: &Path,
    check_interval_hours: Option<u64>,
    timeout_seconds: Option<u64>,
) -> Option<UpdateCheckResult> {
    let interval = check_interval_hours.unwrap_or(DEFAULT_CHECK_INTERVAL_HOURS);
    let timeout = timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS);

    // Check cache first
    if let Some(cache) = _load_cache(cache_dir)
        && !cache.is_expired()
    {
        debug!("Update cache still fresh, skipping check");
        let is_newer = _compare_versions(current_version, &cache.latest_version);
        return Some(UpdateCheckResult {
            current_version: current_version.to_string(),
            latest_version: cache.latest_version.clone(),
            is_newer,
            release_url: format!("https://github.com/{GITHUB_REPO}/releases/latest"),
        });
    }

    let latest = _fetch_latest_version(timeout)?;
    let is_newer = _compare_versions(current_version, &latest);

    let result = UpdateCheckResult {
        current_version: current_version.to_string(),
        latest_version: latest.clone(),
        is_newer,
        release_url: format!("https://github.com/{GITHUB_REPO}/releases/latest"),
    };

    let cache = UpdateCache {
        last_check: chrono::Utc::now().to_rfc3339(),
        latest_version: latest,
        check_interval_hours: interval,
    };
    _save_cache(cache_dir, &cache);

    Some(result)
}

/// Prompt the user and perform an upgrade if they agree.
///
/// Returns `true` if the user accepted the upgrade (regardless of outcome).
pub fn prompt_and_upgrade(update_info: &UpdateCheckResult, cli_args: &[String]) -> bool {
    if !update_info.is_newer {
        return false;
    }

    eprintln!(
        "amplihack: update available {} → {}",
        update_info.current_version, update_info.latest_version
    );
    eprintln!("  Release: {}", update_info.release_url);

    // In non-interactive mode, just notify
    if std::env::var("AMPLIHACK_NONINTERACTIVE").is_ok() {
        return false;
    }

    eprint!("Upgrade now? [y/N] ");
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }

    if !input.trim().eq_ignore_ascii_case("y") {
        return false;
    }

    let exit_code = run_update_command();
    if exit_code == 0 {
        info!("Upgrade successful, restarting…");
        _restart_cli(cli_args);
    } else {
        warn!(exit_code, "Upgrade command failed");
    }

    true
}

/// Run the upgrade command, returning the process exit code.
///
/// Tries the Rust CLI self-update first, falling back to `uv tool upgrade`.
pub fn run_update_command() -> i32 {
    if let Some(rust_cli) = _find_rust_cli() {
        info!(cli = %rust_cli.display(), "Running Rust CLI self-update");
        return _run_upgrade(&rust_cli, &["update", "--self"]);
    }

    info!("Falling back to uv tool upgrade");
    let uv = PathBuf::from("uv");
    _run_upgrade(&uv, &["tool", "upgrade", "amplihack"])
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Fetch the latest release version tag from the GitHub API.
fn _fetch_latest_version(timeout_secs: u64) -> Option<String> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build();

    let response = agent
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "amplihack-rs")
        .call()
        .ok()?;

    let body: serde_json::Value = response.into_json().ok()?;
    let tag = body.get("tag_name")?.as_str()?;
    let trimmed = tag.trim_start_matches('v');
    debug!(version = trimmed, "Fetched latest release");
    Some(trimmed.to_string())
}

/// Compare two semver strings, returning `true` if `latest > current`.
fn _compare_versions(current: &str, latest: &str) -> bool {
    let Ok(cur) = Version::parse(current.trim_start_matches('v')) else {
        return false;
    };
    let Ok(lat) = Version::parse(latest.trim_start_matches('v')) else {
        return false;
    };
    lat > cur
}

/// Load the update cache from disk.
fn _load_cache(cache_dir: &Path) -> Option<UpdateCache> {
    let path = cache_dir.join(CACHE_FILE_NAME);
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Persist the update cache to disk.
fn _save_cache(cache_dir: &Path, cache: &UpdateCache) {
    let path = cache_dir.join(CACHE_FILE_NAME);
    if let Err(e) = std::fs::create_dir_all(cache_dir) {
        debug!(error = %e, "Failed to create cache dir");
        return;
    }
    match serde_json::to_string_pretty(cache) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                debug!(error = %e, "Failed to write update cache");
            }
        }
        Err(e) => debug!(error = %e, "Failed to serialize update cache"),
    }
}

/// Run an upgrade subprocess and return its exit code.
fn _run_upgrade(binary: &Path, args: &[&str]) -> i32 {
    match Command::new(binary).args(args).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            warn!(error = %e, binary = %binary.display(), "Failed to run upgrade command");
            1
        }
    }
}

/// Locate the Rust CLI binary on `PATH`.
fn _find_rust_cli() -> Option<PathBuf> {
    which_binary("amplihack")
}

/// Restart the CLI by exec-ing with the same arguments.
fn _restart_cli(args: &[String]) {
    if let Some(exe) = _find_rust_cli() {
        info!(exe = %exe.display(), "Restarting CLI");
        let mut cmd = Command::new(&exe);
        cmd.args(args);
        // On Unix we can exec to replace the process
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = cmd.exec();
            warn!(error = %err, "exec failed, continuing");
        }
        #[cfg(not(unix))]
        {
            let _ = cmd.status();
        }
    }
}

/// Search `PATH` for a binary by name.
fn which_binary(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(name);
            if candidate.is_file() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_versions_newer() {
        assert!(_compare_versions("0.1.0", "0.2.0"));
        assert!(_compare_versions("1.0.0", "2.0.0"));
        assert!(_compare_versions("0.7.23", "0.7.24"));
    }

    #[test]
    fn compare_versions_same_or_older() {
        assert!(!_compare_versions("1.0.0", "1.0.0"));
        assert!(!_compare_versions("2.0.0", "1.0.0"));
    }

    #[test]
    fn compare_versions_with_v_prefix() {
        assert!(_compare_versions("v0.1.0", "v0.2.0"));
        assert!(!_compare_versions("v1.0.0", "v0.9.0"));
    }

    #[test]
    fn compare_versions_invalid_returns_false() {
        assert!(!_compare_versions("not-a-version", "1.0.0"));
        assert!(!_compare_versions("1.0.0", "garbage"));
    }

    #[test]
    fn cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = UpdateCache {
            last_check: chrono::Utc::now().to_rfc3339(),
            latest_version: "1.2.3".to_string(),
            check_interval_hours: 24,
        };
        _save_cache(dir.path(), &cache);

        let loaded = _load_cache(dir.path()).expect("cache should load");
        assert_eq!(loaded.latest_version, "1.2.3");
        assert_eq!(loaded.check_interval_hours, 24);
    }

    #[test]
    fn cache_expired_when_old_timestamp() {
        let cache = UpdateCache {
            last_check: "2020-01-01T00:00:00+00:00".to_string(),
            latest_version: "0.1.0".to_string(),
            check_interval_hours: 24,
        };
        assert!(cache.is_expired());
    }

    #[test]
    fn cache_not_expired_when_recent() {
        let cache = UpdateCache {
            last_check: chrono::Utc::now().to_rfc3339(),
            latest_version: "0.1.0".to_string(),
            check_interval_hours: 24,
        };
        assert!(!cache.is_expired());
    }

    #[test]
    fn cache_expired_on_invalid_timestamp() {
        let cache = UpdateCache {
            last_check: "not-a-date".to_string(),
            latest_version: "0.1.0".to_string(),
            check_interval_hours: 24,
        };
        assert!(cache.is_expired());
    }

    #[test]
    fn load_cache_missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(_load_cache(dir.path()).is_none());
    }

    #[test]
    fn update_check_result_serialization() {
        let result = UpdateCheckResult {
            current_version: "0.7.23".into(),
            latest_version: "0.8.0".into(),
            is_newer: true,
            release_url: "https://example.com".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: UpdateCheckResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.current_version, "0.7.23");
        assert!(deser.is_newer);
    }
}
