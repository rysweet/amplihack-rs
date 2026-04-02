//! Claude CLI binary detection, installation, and version checking.
//!
//! Ported from `amplihack/utils/claude_cli.py`. Provides helpers to locate
//! the `claude` CLI binary, validate that it works, ensure it is installed
//! (via npm), and compare the installed version against the latest published
//! version.
//!
//! Much of the raw PATH-based binary search is delegated to the patterns
//! established in `amplihack-cli/binary_finder.rs`; this module adds the
//! npm-install and version-comparison layers on top.

use crate::process::ProcessManager;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;
use thiserror::Error;

// Errors

/// Errors produced by Claude CLI operations.
#[derive(Debug, Error)]
pub enum ClaudeCliError {
    /// A subprocess operation failed.
    #[error("process error: {0}")]
    Process(#[from] crate::process::ProcessError),

    /// npm is not installed — required for auto-installation.
    #[error("npm is not installed; install Node.js first")]
    NpmNotFound,

    /// The installation command exited with a non-zero status.
    #[error("npm install failed (exit {code:?}): {stderr}")]
    InstallFailed {
        /// Exit code from npm, if available.
        code: Option<i32>,
        /// Captured stderr from the install command.
        stderr: String,
    },

    /// The installed binary could not be validated.
    #[error("claude binary at {path} failed validation: {reason}")]
    ValidationFailed {
        /// Path to the binary that was tested.
        path: String,
        /// Human-readable reason.
        reason: String,
    },
}

// Version status

/// Comparison of the installed Claude CLI version against the latest
/// published version.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VersionStatus {
    /// Installed version is up to date.
    Current(String),
    /// A newer version is available.
    UpdateAvailable {
        /// Currently installed version.
        current: String,
        /// Latest published version.
        latest: String,
    },
    /// Could not determine version information.
    Unknown,
}

// Constants

/// npm package name for Claude Code.
const CLAUDE_NPM_PACKAGE: &str = "@anthropic-ai/claude-code";

/// Default timeout for version-check subprocesses.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for npm install commands.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Regex for extracting a semantic version from a string.
static SEMVER_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(\d+\.\d+\.\d+)").expect("semver regex"));

/// Return the user-local npm prefix directory (`~/.npm-global`).
fn npm_global_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".npm-global"))
}

/// Return the bin directory under the npm global prefix.
fn npm_global_bin() -> Option<PathBuf> {
    npm_global_dir().map(|d| d.join("bin"))
}

/// Resolve `$HOME` portably.
fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
}

// Binary detection

/// Find the claude CLI binary path.
///
/// Search order:
/// 1. `AMPLIHACK_CLAUDE_BINARY_PATH` env var (explicit override).
/// 2. `AMPLIHACK_AGENT_BINARY` env var (runtime launcher override).
/// 3. System `PATH` (via `which`/`where.exe`).
/// 4. Fallback: `~/.npm-global/bin/claude`.
///
/// Returns `None` if the binary is not found in any location.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::claude_cli::get_claude_cli_path;
///
/// if let Some(path) = get_claude_cli_path() {
///     println!("claude binary: {}", path.display());
/// }
/// ```
pub fn get_claude_cli_path() -> Option<PathBuf> {
    // 1. Explicit override
    if let Ok(p) = env::var("AMPLIHACK_CLAUDE_BINARY_PATH") {
        let path = PathBuf::from(&p);
        if path.is_file() {
            return Some(path);
        }
    }

    // 2. Runtime launcher override
    if let Ok(p) = env::var("AMPLIHACK_AGENT_BINARY") {
        let path = PathBuf::from(&p);
        if path.is_file() {
            return Some(path);
        }
    }

    // 3. System PATH search
    if let Some(p) = which_claude() {
        return Some(p);
    }

    // 4. Fallback: ~/.npm-global/bin/claude
    if let Some(bin_dir) = npm_global_bin() {
        let candidate = bin_dir.join("claude");
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

/// Locate `claude` on the system PATH using `which` / `where.exe`.
fn which_claude() -> Option<PathBuf> {
    let mgr = ProcessManager::new();
    let which_cmd = if cfg!(target_os = "windows") {
        "where.exe"
    } else {
        "which"
    };
    let result = mgr
        .run_command(&[which_cmd, "claude"], Some(VERSION_TIMEOUT), None, None)
        .ok()?;
    if result.success() {
        let line = result.stdout.lines().next()?.trim().to_string();
        if line.is_empty() {
            return None;
        }
        let p = PathBuf::from(&line);
        if p.is_file() { Some(p) } else { None }
    } else {
        None
    }
}

/// Validate a candidate binary by running `<binary> --version`.
///
/// Returns `true` when the command exits with status 0 within the timeout.
fn validate_binary(path: &Path) -> bool {
    let mgr = ProcessManager::new();
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return false,
    };
    mgr.run_command(&[path_str, "--version"], Some(VERSION_TIMEOUT), None, None)
        .map(|r| r.success())
        .unwrap_or(false)
}

// Installation

/// Ensure the claude CLI is installed and return its path.
///
/// If the binary is already present and passes validation, its path is
/// returned immediately. Otherwise an npm user-local install is attempted.
///
/// # Errors
///
/// Returns [`ClaudeCliError`] if npm is not available, the install command
/// fails, or the installed binary cannot be validated.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::claude_cli::ensure_claude_cli;
///
/// let path = ensure_claude_cli().expect("claude should be installable");
/// println!("claude ready at {}", path.display());
/// ```
pub fn ensure_claude_cli() -> Result<PathBuf, ClaudeCliError> {
    // Already installed?
    if let Some(p) = get_claude_cli_path() {
        if validate_binary(&p) {
            return Ok(p);
        }
        tracing::warn!(path = %p.display(), "claude binary found but failed validation");
    }

    // Ensure npm is available.
    let mgr = ProcessManager::new();
    let npm_check = mgr.run_command(&["npm", "--version"], Some(VERSION_TIMEOUT), None, None);
    match npm_check {
        Ok(r) if r.success() => {}
        _ => return Err(ClaudeCliError::NpmNotFound),
    }

    // Create user-local npm prefix if needed.
    if let Some(global_dir) = npm_global_dir() {
        let _ = std::fs::create_dir_all(&global_dir);
    }

    // Run npm install with --ignore-scripts for supply-chain safety.
    let mut npm_args = vec![
        "npm",
        "install",
        "-g",
        "--ignore-scripts",
        CLAUDE_NPM_PACKAGE,
    ];

    // Set user-local prefix so we don't need sudo.
    let prefix_flag;
    if let Some(global_dir) = npm_global_dir() {
        prefix_flag = format!("--prefix={}", global_dir.display());
        npm_args.insert(2, &prefix_flag);
    }

    tracing::info!(
        package = CLAUDE_NPM_PACKAGE,
        "installing claude CLI via npm"
    );
    let result = mgr.run_command(
        &npm_args.iter().map(|s| s.as_ref()).collect::<Vec<&str>>(),
        Some(INSTALL_TIMEOUT),
        None,
        None,
    )?;

    if !result.success() {
        return Err(ClaudeCliError::InstallFailed {
            code: result.exit_code,
            stderr: result.stderr,
        });
    }

    // Re-detect after installation.
    let installed_path = get_claude_cli_path().ok_or_else(|| ClaudeCliError::ValidationFailed {
        path: "claude".into(),
        reason: "binary not found after npm install".into(),
    })?;

    if !validate_binary(&installed_path) {
        return Err(ClaudeCliError::ValidationFailed {
            path: installed_path.display().to_string(),
            reason: "binary failed --version check after install".into(),
        });
    }

    Ok(installed_path)
}

// Version checking

/// Extract a semantic version string from command output.
///
/// Looks for the first `\d+\.\d+\.\d+` match in `text`.
fn parse_semver(text: &str) -> Option<String> {
    SEMVER_RE.captures(text).map(|c| c[1].to_string())
}

/// Get the installed version of the claude binary at `binary`.
fn get_installed_version(binary: &Path) -> Option<String> {
    let mgr = ProcessManager::new();
    let path_str = binary.to_str()?;
    let result = mgr
        .run_command(&[path_str, "--version"], Some(VERSION_TIMEOUT), None, None)
        .ok()?;
    if !result.success() {
        return None;
    }
    parse_semver(&result.stdout)
}

/// Query npm for the latest published version of the Claude Code package.
fn get_latest_published_version() -> Option<String> {
    let mgr = ProcessManager::new();
    let result = mgr
        .run_command(
            &["npm", "view", CLAUDE_NPM_PACKAGE, "version"],
            Some(VERSION_TIMEOUT),
            None,
            None,
        )
        .ok()?;
    if !result.success() {
        return None;
    }
    parse_semver(&result.stdout)
}

/// Compare two semantic version strings.
///
/// Returns `true` when `latest` is strictly newer than `current`.
fn is_newer(current: &str, latest: &str) -> bool {
    let parse = |v: &str| -> Option<(u64, u64, u64)> {
        let v = v.strip_prefix('v').unwrap_or(v);
        let parts: Vec<&str> = v.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        ))
    };
    match (parse(current), parse(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// Check whether the installed claude version is up to date.
///
/// Queries the installed binary for its version and compares against the
/// latest version published on npm.
///
/// # Errors
///
/// Returns [`ClaudeCliError::Process`] if subprocess execution fails.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::claude_cli::{check_claude_version, VersionStatus};
/// use std::path::Path;
///
/// match check_claude_version(Path::new("/usr/local/bin/claude")) {
///     Ok(VersionStatus::Current(v)) => println!("up to date: {v}"),
///     Ok(VersionStatus::UpdateAvailable { current, latest }) => {
///         println!("update available: {current} → {latest}");
///     }
///     Ok(VersionStatus::Unknown) => println!("could not determine version"),
///     Err(e) => eprintln!("error: {e}"),
/// }
/// ```
pub fn check_claude_version(binary: &Path) -> Result<VersionStatus, ClaudeCliError> {
    let current = match get_installed_version(binary) {
        Some(v) => v,
        None => return Ok(VersionStatus::Unknown),
    };

    let latest = match get_latest_published_version() {
        Some(v) => v,
        None => {
            // Cannot determine latest — assume current is fine.
            return Ok(VersionStatus::Current(current));
        }
    };

    if is_newer(&current, &latest) {
        Ok(VersionStatus::UpdateAvailable { current, latest })
    } else {
        Ok(VersionStatus::Current(current))
    }
}

// Tests

#[cfg(test)]
#[path = "tests/claude_cli_tests.rs"]
mod tests;
