//! Tool detection and installation guidance.
//!
//! Ported from `amplihack/utils/prerequisites.py`. Provides helpers to
//! detect whether required CLI tools (git, node, npm, etc.) are installed,
//! retrieve their versions, and generate platform-specific installation
//! hints when a tool is missing.

use crate::process::{CommandResult, ProcessError, ProcessManager};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced by prerequisite checking operations.
#[derive(Debug, Error)]
pub enum PrerequisiteError {
    /// A process operation failed.
    #[error(transparent)]
    Process(#[from] ProcessError),

    /// No tools were supplied to check.
    #[error("empty tool list")]
    EmptyToolList,
}

// ---------------------------------------------------------------------------
// Platform detection
// ---------------------------------------------------------------------------

/// Host platform as seen by prerequisite checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Platform {
    /// Native Linux.
    Linux,
    /// macOS / Darwin.
    MacOs,
    /// Windows (including MSYS2/Git-Bash).
    Windows,
    /// Windows Subsystem for Linux.
    Wsl,
    /// Could not determine the platform.
    Unknown,
}

/// Detect the current host platform, including WSL detection on Linux.
pub fn detect_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "linux") {
        if is_wsl() {
            Platform::Wsl
        } else {
            Platform::Linux
        }
    } else {
        Platform::Unknown
    }
}

/// Check `/proc/version` for the `microsoft` token that indicates WSL.
fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| v.to_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

/// Descriptor for a tool that should be checked.
struct ToolSpec {
    name: &'static str,
    version_arg: &'static str,
    required: bool,
}

/// The canonical set of tools checked by [`check_prerequisites`].
const REQUIRED_TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "git",
        version_arg: "--version",
        required: true,
    },
    ToolSpec {
        name: "node",
        version_arg: "--version",
        required: true,
    },
    ToolSpec {
        name: "npm",
        version_arg: "--version",
        required: true,
    },
    ToolSpec {
        name: "uv",
        version_arg: "--version",
        required: false,
    },
    ToolSpec {
        name: "rg",
        version_arg: "--version",
        required: false,
    },
    ToolSpec {
        name: "tmux",
        version_arg: "-V",
        required: false,
    },
];

// ---------------------------------------------------------------------------
// Check result
// ---------------------------------------------------------------------------

/// Result of checking a single tool.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCheckResult {
    /// Tool name (e.g. `"git"`).
    pub name: String,
    /// Whether the tool was found on the system.
    pub found: bool,
    /// Version string extracted from stdout (first line, trimmed).
    pub version: Option<String>,
    /// Absolute path to the binary, if located.
    pub path: Option<PathBuf>,
    /// Human-readable installation hint for the current platform.
    pub install_hint: String,
    /// Whether this tool is required for normal operation.
    pub required: bool,
}

// ---------------------------------------------------------------------------
// Install hints
// ---------------------------------------------------------------------------

/// Return a platform-specific installation hint for `tool`.
pub fn install_hint(tool: &str, platform: Platform) -> String {
    match platform {
        Platform::MacOs => install_hint_macos(tool),
        Platform::Linux | Platform::Wsl => install_hint_linux(tool),
        Platform::Windows => install_hint_windows(tool),
        Platform::Unknown => format!("Install {tool} manually — see the project docs."),
    }
}

fn install_hint_macos(tool: &str) -> String {
    match tool {
        "git" => "brew install git".into(),
        "node" | "npm" => "brew install node".into(),
        "uv" => "brew install uv".into(),
        "rg" => "brew install ripgrep".into(),
        "tmux" => "brew install tmux".into(),
        other => format!("brew install {other}"),
    }
}

fn install_hint_linux(tool: &str) -> String {
    match tool {
        "git" => "sudo apt install git  # or: dnf install git".into(),
        "node" | "npm" => "curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash - && \
             sudo apt install -y nodejs"
            .into(),
        "uv" => "curl -LsSf https://astral.sh/uv/install.sh | sh".into(),
        "rg" => "sudo apt install ripgrep  # or: cargo install ripgrep".into(),
        "tmux" => "sudo apt install tmux".into(),
        other => format!("sudo apt install {other}"),
    }
}

fn install_hint_windows(tool: &str) -> String {
    match tool {
        "git" => "winget install Git.Git".into(),
        "node" | "npm" => "winget install OpenJS.NodeJS.LTS".into(),
        "uv" => "powershell -c \"irm https://astral.sh/uv/install.ps1 | iex\"".into(),
        "rg" => "winget install BurntSushi.ripgrep.MSVC".into(),
        "tmux" => "tmux is not natively available on Windows; use WSL instead.".into(),
        other => format!("winget install {other}"),
    }
}

// ---------------------------------------------------------------------------
// Core check functions
// ---------------------------------------------------------------------------

/// Default timeout for version-check subprocess calls.
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Run a subprocess safely with timeout, capturing output.
///
/// Thin convenience wrapper around [`ProcessManager`] that constructs a
/// manager, runs the command with the given timeout, and returns the
/// [`CommandResult`].
///
/// # Errors
///
/// Returns [`PrerequisiteError::Process`] when the subprocess cannot be
/// spawned or an I/O error occurs.
pub fn safe_subprocess_call(
    args: &[&str],
    timeout_secs: u64,
) -> Result<CommandResult, PrerequisiteError> {
    let mgr = ProcessManager::new();
    let result = mgr.run_command(args, Some(Duration::from_secs(timeout_secs)), None, None)?;
    Ok(result)
}

/// Check whether a single tool is available.
///
/// Attempts to locate the binary on `PATH` via `which` (Unix) or
/// `where.exe` (Windows), then runs its version flag to extract the
/// version string.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::prerequisites::check_tool;
///
/// let result = check_tool("git");
/// if result.found {
///     println!("git {} at {:?}", result.version.as_deref().unwrap_or("?"), result.path);
/// }
/// ```
pub fn check_tool(name: &str) -> ToolCheckResult {
    let platform = detect_platform();
    let hint = install_hint(name, platform);
    let spec = REQUIRED_TOOLS.iter().find(|s| s.name == name);
    let version_arg = spec.map_or("--version", |s| s.version_arg);
    let required = spec.is_some_and(|s| s.required);

    let path = locate_binary(name);

    let (found, version) = match &path {
        Some(p) => {
            let ver = extract_version(p.to_str().unwrap_or(name), version_arg);
            (true, ver)
        }
        None => (false, None),
    };

    ToolCheckResult {
        name: name.to_string(),
        found,
        version,
        path,
        install_hint: hint,
        required,
    }
}

/// Check all required and optional prerequisites.
///
/// Returns one [`ToolCheckResult`] per tool in [`REQUIRED_TOOLS`].
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::prerequisites::check_prerequisites;
///
/// for r in check_prerequisites() {
///     let status = if r.found { "✓" } else { "✗" };
///     println!("{status} {} {}", r.name, r.version.as_deref().unwrap_or(""));
/// }
/// ```
pub fn check_prerequisites() -> Vec<ToolCheckResult> {
    REQUIRED_TOOLS
        .iter()
        .map(|spec| check_tool(spec.name))
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Attempt to locate a binary by name on the system `PATH`.
///
/// Uses `which` on Unix and `where.exe` on Windows.
fn locate_binary(name: &str) -> Option<PathBuf> {
    let mgr = ProcessManager::new();
    let which_cmd = if cfg!(target_os = "windows") {
        "where.exe"
    } else {
        "which"
    };
    let result = mgr
        .run_command(&[which_cmd, name], Some(VERSION_CHECK_TIMEOUT), None, None)
        .ok()?;

    if result.success() {
        let line = result.stdout.lines().next()?.trim().to_string();
        if line.is_empty() {
            return None;
        }
        let p = PathBuf::from(&line);
        if p.exists() { Some(p) } else { None }
    } else {
        None
    }
}

/// Run a binary with its version flag and return the first line of stdout.
fn extract_version(binary: &str, version_arg: &str) -> Option<String> {
    let mgr = ProcessManager::new();
    let result = mgr
        .run_command(
            &[binary, version_arg],
            Some(VERSION_CHECK_TIMEOUT),
            None,
            None,
        )
        .ok()?;

    if !result.success() {
        return None;
    }

    let first_line = result
        .stdout
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if first_line.is_empty() {
        None
    } else {
        Some(first_line)
    }
}

/// Return the names of tools that are required but missing.
///
/// Convenience function that filters [`check_prerequisites`] to only the
/// required tools that were not found.
pub fn missing_required() -> Vec<String> {
    check_prerequisites()
        .into_iter()
        .filter(|r| r.required && !r.found)
        .map(|r| r.name)
        .collect()
}

/// Produce a human-readable summary of all prerequisite checks.
///
/// Each line is prefixed with `✓` or `✗` followed by the tool name,
/// version, and install hint when relevant.
pub fn summary_string(results: &[ToolCheckResult]) -> String {
    let mut buf = String::new();
    for r in results {
        let marker = if r.found { "✓" } else { "✗" };
        let ver = r.version.as_deref().unwrap_or("");
        buf.push_str(&format!("{marker} {:<6} {ver}", r.name));
        if !r.found {
            buf.push_str(&format!("  → {}", r.install_hint));
        }
        buf.push('\n');
    }
    buf
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/prerequisites_tests.rs"]
mod tests;
