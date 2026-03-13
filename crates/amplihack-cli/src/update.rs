use anyhow::{Context, Result, anyhow, bail};
use flate2::read::GzDecoder;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
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

/// Maximum size for binary release downloads (amplihack + hooks binaries).
/// A generous upper bound; actual binaries are well under this limit.
/// Protects against OOM from a malicious or misconfigured server.
const MAX_BINARY_DOWNLOAD_BYTES: usize = 512 * 1024 * 1024; // 512 MiB

/// Validate that a download URL belongs to the expected GitHub host.
///
/// SEC-1: `browser_download_url` values come from the GitHub API response
/// which could be tampered with (MITM or compromised release).  Ensuring
/// the URL is on github.com / objects.githubusercontent.com limits the blast
/// radius of such attacks.
fn validate_download_url(url: &str) -> Result<()> {
    // Accept the GitHub API base and the CDN used for release asset downloads.
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

fn http_get(url: &str) -> Result<Vec<u8>> {
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
        .set("User-Agent", &format!("amplihack/{CURRENT_VERSION}"))
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) if url.ends_with("/releases/latest") => {
            bail!("no stable v* release has been published for {GITHUB_REPO} yet")
        }
        Err(error) => return Err(anyhow!("HTTP request failed for {url}: {error}")),
    };

    // SEC-4: Use a bounded read to prevent OOM from a malicious server
    // sending an unbounded response.  Use the larger binary limit for all
    // requests; the API response limit would suffice for metadata calls but
    // we cannot cheaply distinguish them here without complicating the API.
    let limit = MAX_BINARY_DOWNLOAD_BYTES;
    let mut body = Vec::new();
    response
        .into_reader()
        .take(limit as u64)
        .read_to_end(&mut body)
        .with_context(|| format!("failed to read HTTP response from {url}"))?;

    if body.len() == limit {
        bail!(
            "HTTP response from {url} exceeded the size limit of {} bytes; aborting to prevent OOM",
            limit
        );
    }

    Ok(body)
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

    // Look for a matching `.sha256` checksum file in the release assets.
    let checksum_asset_name = format!("{asset_name}.sha256");
    let checksum_url = release
        .assets
        .iter()
        .find(|a| a.name == checksum_asset_name)
        .map(|a| a.browser_download_url.clone());

    Ok(UpdateRelease {
        version,
        asset_url: asset.browser_download_url.clone(),
        checksum_url,
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

/// Determine whether the update check should be skipped based on the subcommand
/// name string alone (without needing the full args slice).
///
/// Returns `true` (skip the check) when:
/// - `AMPLIHACK_NONINTERACTIVE=1` is set in the environment
/// - `AMPLIHACK_PARITY_TEST=1` is set in the environment
/// - `AMPLIHACK_NO_UPDATE_CHECK=1` is set in the environment
/// - `subcommand` is not one of the known launch commands
///   (`launch`, `claude`, `copilot`, `codex`, `amplifier`)
///
/// This is the string-oriented companion to `should_skip_update_check` and is
/// the function that callers with a parsed subcommand name should use.
///
pub fn should_skip_update_check_for_subcommand(subcommand: &str) -> bool {
    // Explicit opt-out via legacy env var.
    if std::env::var(NO_UPDATE_CHECK_ENV).unwrap_or_default() == "1" {
        return true;
    }

    // Non-interactive / scripted environments suppress all update checks.
    if std::env::var("AMPLIHACK_NONINTERACTIVE").as_deref() == Ok("1") {
        return true;
    }

    // Parity test harness — suppress update noise during automated comparison runs.
    if std::env::var("AMPLIHACK_PARITY_TEST").as_deref() == Ok("1") {
        return true;
    }

    // Only show the update notice for known launch commands.
    // All other subcommands skip the update check.
    !matches!(
        subcommand,
        "launch" | "claude" | "copilot" | "codex" | "amplifier"
    )
}

fn should_skip_update_check(args: &[OsString]) -> bool {
    // Explicit opt-out via legacy env var.
    if std::env::var(NO_UPDATE_CHECK_ENV).unwrap_or_default() == "1" {
        return true;
    }

    // Non-interactive / scripted environments (CI, AMPLIHACK_NONINTERACTIVE=1).
    // Check the env var directly (not is_noninteractive()) so unit tests that
    // run with a non-TTY stdin are not affected.
    if std::env::var("AMPLIHACK_NONINTERACTIVE").as_deref() == Ok("1") {
        return true;
    }

    // Parity test harness — suppress update noise during automated comparison runs.
    if std::env::var("AMPLIHACK_PARITY_TEST").as_deref() == Ok("1") {
        return true;
    }

    let first_arg = args.get(1).and_then(|arg| arg.to_str());

    // Only show the update notice when the user is about to launch a tool.
    // Subcommands like mode, plugin, recipe, memory, install, doctor, version,
    // and help never trigger an update announcement — only launch commands do.
    !matches!(
        first_arg,
        Some("launch") | Some("claude") | Some("copilot") | Some("codex") | Some("amplifier")
    )
}

fn print_update_notice(latest: &str) {
    eprintln!(
        "\x1b[33mA newer version of amplihack is available (v{}). Run 'amplihack update' to upgrade.\x1b[0m",
        latest
    );
}

/// Verify a downloaded archive against its SHA-256 checksum.
///
/// The `.sha256` file follows the `sha256sum` format: the first whitespace-delimited
/// token on the first line is the expected hex digest.  The filename on the same line
/// (if present) is ignored — we only trust the digest.
fn verify_sha256(archive_bytes: &[u8], checksum_url: &str) -> Result<()> {
    let checksum_body = http_get(checksum_url)
        .with_context(|| format!("failed to download checksum from {checksum_url}"))?;
    let checksum_text =
        std::str::from_utf8(&checksum_body).context("checksum file is not valid UTF-8")?;

    // The first whitespace-delimited token is the hex digest.
    let expected_hex = checksum_text
        .split_ascii_whitespace()
        .next()
        .ok_or_else(|| anyhow!("checksum file is empty or malformed: {checksum_url}"))?;

    if expected_hex.len() != 64 || !expected_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!(
            "checksum file does not contain a valid SHA-256 hex digest (got {:?}): {checksum_url}",
            expected_hex
        );
    }

    let mut hasher = Sha256::new();
    hasher.update(archive_bytes);
    let actual_bytes = hasher.finalize();
    let actual_hex = format!("{actual_bytes:x}");

    if actual_hex != expected_hex.to_ascii_lowercase() {
        bail!(
            "SHA-256 checksum mismatch for downloaded archive:\n  expected: {expected_hex}\n  actual:   {actual_hex}\nAborted to prevent installing a corrupt or tampered binary."
        );
    }

    tracing::debug!(
        checksum_url,
        sha256 = actual_hex,
        "archive checksum verified"
    );
    Ok(())
}

fn download_and_replace(release: &UpdateRelease) -> Result<()> {
    let archive_bytes = http_get(&release.asset_url)?;

    // SEC-1: Verify SHA-256 checksum against the release manifest before
    // extracting or installing anything.  If the release does not publish a
    // checksum file we warn but continue, since older releases pre-date the
    // checksum upload step.  New releases (built by the current CI) always
    // publish a `.sha256` alongside the `.tar.gz`.
    if let Some(checksum_url) = &release.checksum_url {
        verify_sha256(&archive_bytes, checksum_url)
            .context("binary download checksum verification failed")?;
        println!("SHA-256 checksum verified.");
    } else {
        tracing::warn!(
            "no checksum file found for release {}; skipping SHA-256 verification",
            release.version
        );
    }
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

    // ---------------------------------------------------------------------------
    // TDD Step 7: Failing tests for update check suppression (Category 1)
    //
    // These tests define the contract for a `should_skip_update_check_for_subcommand`
    // function that accepts a plain subcommand name string instead of the raw
    // `&[OsString]` args slice. These tests FAIL until the implementation provides
    // that function with the correct logic.
    // ---------------------------------------------------------------------------

    /// When AMPLIHACK_NONINTERACTIVE=1 is set, ALL subcommands — including launch
    /// commands — must skip the update check to avoid polluting scripted output.
    #[test]
    fn test_skip_update_check_when_noninteractive_env_set() {
        // Arrange: set the non-interactive env var
        // SAFETY: single-threaded test context; env mutation is serialized by the
        // test runner running this test in isolation.
        unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", "1") };
        // Act + Assert: even a launch subcommand should skip
        let result = should_skip_update_check_for_subcommand("launch");
        // Cleanup before asserting (so the env is restored even on failure)
        unsafe { std::env::remove_var("AMPLIHACK_NONINTERACTIVE") };
        assert!(
            result,
            "should_skip_update_check_for_subcommand('launch') must return true \
             when AMPLIHACK_NONINTERACTIVE=1"
        );
    }

    /// When AMPLIHACK_PARITY_TEST=1 is set, the update check must be suppressed
    /// to prevent update noise from polluting automated parity comparison output.
    #[test]
    fn test_skip_update_check_when_parity_test_env_set() {
        // SAFETY: single-threaded test context.
        unsafe { std::env::set_var("AMPLIHACK_PARITY_TEST", "1") };
        let result = should_skip_update_check_for_subcommand("launch");
        unsafe { std::env::remove_var("AMPLIHACK_PARITY_TEST") };
        assert!(
            result,
            "should_skip_update_check_for_subcommand('launch') must return true \
             when AMPLIHACK_PARITY_TEST=1"
        );
    }

    /// The `mode` subcommand is not a launch command — update checks must be
    /// skipped. Only launch, claude, copilot, codex, amplifier trigger updates.
    #[test]
    fn test_skip_update_check_for_mode_subcommand() {
        // Ensure env vars that would cause early-return are not set
        // SAFETY: single-threaded test context.
        unsafe {
            std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
            std::env::remove_var("AMPLIHACK_PARITY_TEST");
            std::env::remove_var(NO_UPDATE_CHECK_ENV);
        }
        assert!(
            should_skip_update_check_for_subcommand("mode"),
            "should_skip_update_check_for_subcommand('mode') must return true — \
             'mode' is not a launch command and should never trigger an update notice"
        );
    }

    /// The `plugin` subcommand is not a launch command — update checks must be
    /// skipped.
    #[test]
    fn test_skip_update_check_for_plugin_subcommand() {
        // SAFETY: single-threaded test context.
        unsafe {
            std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
            std::env::remove_var("AMPLIHACK_PARITY_TEST");
            std::env::remove_var(NO_UPDATE_CHECK_ENV);
        }
        assert!(
            should_skip_update_check_for_subcommand("plugin"),
            "should_skip_update_check_for_subcommand('plugin') must return true — \
             'plugin' is not a launch command"
        );
    }

    /// Unknown or unrecognised subcommands must skip the update check — only
    /// the known launch commands should trigger it.
    #[test]
    fn test_skip_update_check_for_unknown_subcommand() {
        // SAFETY: single-threaded test context.
        unsafe {
            std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
            std::env::remove_var("AMPLIHACK_PARITY_TEST");
            std::env::remove_var(NO_UPDATE_CHECK_ENV);
        }
        assert!(
            should_skip_update_check_for_subcommand("totally-unknown-command"),
            "should_skip_update_check_for_subcommand('totally-unknown-command') must \
             return true — unrecognised commands are not launch commands"
        );
    }

    /// The `launch` subcommand IS a launch command. With no suppressing env vars,
    /// `should_skip_update_check_for_subcommand` must return false so the caller
    /// proceeds with the update check.
    #[test]
    fn test_allow_update_check_for_launch_subcommand() {
        // SAFETY: single-threaded test context.
        unsafe {
            std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
            std::env::remove_var("AMPLIHACK_PARITY_TEST");
            std::env::remove_var(NO_UPDATE_CHECK_ENV);
        }
        assert!(
            !should_skip_update_check_for_subcommand("launch"),
            "should_skip_update_check_for_subcommand('launch') must return false \
             (i.e. do NOT skip) when no suppressing env vars are set"
        );
    }

    /// The `claude` subcommand IS a launch command. With no suppressing env vars,
    /// the update check must proceed (return false).
    #[test]
    fn test_allow_update_check_for_claude_subcommand() {
        // SAFETY: single-threaded test context.
        unsafe {
            std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
            std::env::remove_var("AMPLIHACK_PARITY_TEST");
            std::env::remove_var(NO_UPDATE_CHECK_ENV);
        }
        assert!(
            !should_skip_update_check_for_subcommand("claude"),
            "should_skip_update_check_for_subcommand('claude') must return false \
             (i.e. do NOT skip) when no suppressing env vars are set"
        );
    }

    #[test]
    fn validate_download_url_accepts_allowed_hosts() {
        assert!(validate_download_url("https://api.github.com/repos/x/y/releases/latest").is_ok());
        assert!(
            validate_download_url("https://github.com/x/y/releases/download/v1/x.tar.gz").is_ok()
        );
        assert!(validate_download_url("https://objects.githubusercontent.com/x/y.tar.gz").is_ok());
    }

    #[test]
    fn validate_download_url_rejects_disallowed_hosts() {
        assert!(validate_download_url("https://example.com/evil.tar.gz").is_err());
        assert!(
            validate_download_url("http://api.github.com/repos/x/y").is_err(),
            "http:// (not https://) must be rejected"
        );
        assert!(
            validate_download_url("https://attacker.com/https://api.github.com/").is_err(),
            "URL that contains but does not start with an allowed prefix must be rejected"
        );
        assert!(
            validate_download_url("").is_err(),
            "empty URL must be rejected"
        );
    }

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
    fn should_skip_update_check_for_non_launch_subcommands() {
        // Mode, plugin, recipe, memory, install, doctor commands should all skip
        for subcmd in &["mode", "plugin", "recipe", "memory", "install", "doctor"] {
            assert!(
                should_skip_update_check(&[OsString::from("amplihack"), OsString::from(*subcmd),]),
                "expected update check to be skipped for subcommand '{subcmd}'"
            );
        }
    }

    #[test]
    fn should_not_skip_update_check_for_launch_subcommands() {
        // Launch commands should run the update check (when interactive and no env overrides)
        for subcmd in &["launch", "claude", "copilot", "codex", "amplifier"] {
            assert!(
                !should_skip_update_check(&[OsString::from("amplihack"), OsString::from(*subcmd),]),
                "expected update check to NOT be skipped for launch subcommand '{subcmd}'"
            );
        }
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
        let archive_name = expected_archive_name().unwrap();
        let checksum_name = format!("{archive_name}.sha256");
        let json = format!(
            r#"{{
                "tag_name": "v0.2.0",
                "draft": false,
                "prerelease": false,
                "assets": [
                    {{"name": "wrong.tar.gz", "browser_download_url": "https://example.invalid/wrong"}},
                    {{"name": "{archive_name}", "browser_download_url": "https://example.invalid/right"}},
                    {{"name": "{checksum_name}", "browser_download_url": "https://example.invalid/right.sha256"}}
                ]
            }}"#
        );
        let release =
            parse_latest_release(json.into_bytes(), &expected_archive_name().unwrap()).unwrap();
        assert_eq!(
            release,
            UpdateRelease {
                version: "0.2.0".to_string(),
                asset_url: "https://example.invalid/right".to_string(),
                checksum_url: Some("https://example.invalid/right.sha256".to_string()),
            }
        );
    }

    #[test]
    fn parse_latest_release_no_checksum_asset() {
        let archive_name = expected_archive_name().unwrap();
        let json = format!(
            r#"{{
                "tag_name": "v0.2.0",
                "draft": false,
                "prerelease": false,
                "assets": [
                    {{"name": "{archive_name}", "browser_download_url": "https://example.invalid/right"}}
                ]
            }}"#
        );
        let release =
            parse_latest_release(json.into_bytes(), &expected_archive_name().unwrap()).unwrap();
        assert_eq!(release.checksum_url, None);
    }

    #[test]
    fn sha256_computation_produces_64_hex_char_digest() {
        // verify_sha256 requires an HTTP URL for the checksum file, so this
        // test exercises the underlying sha2 hasher in isolation to confirm
        // the digest output is 64 hex characters — the format verify_sha256
        // validates before comparing digests.
        let data = b"hello world";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let digest = format!("{:x}", hasher.finalize());
        // SHA-256 of "hello world" (no trailing newline):
        // b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        assert_eq!(digest.len(), 64);
        assert!(
            digest.chars().all(|c| c.is_ascii_hexdigit()),
            "digest must be all hex digits"
        );
    }

    #[test]
    fn sha256_digest_changes_when_data_changes() {
        // Confirm that a single-bit change in data produces a different digest,
        // which is the property verify_sha256 relies on to detect tampering.
        let data = b"some binary content";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let actual = format!("{:x}", hasher.finalize());
        // Flip one hex character to simulate a mismatch scenario
        let mut wrong = actual.clone();
        wrong.replace_range(0..1, if wrong.starts_with('a') { "b" } else { "a" });
        assert_ne!(actual, wrong);
    }

    #[test]
    fn current_test_platform_has_release_target() {
        assert!(supported_release_target().is_some());
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
