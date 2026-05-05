//! Claude binary detection and command building.
//!
//! Detects native Claude binaries (`rustyclawd`, `claude`) on `$PATH` or via
//! the `CLAUDE_BINARY_PATH` environment variable and builds launch commands
//! with optional trace-logging flags.

use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;

/// Default trace log file path when none is specified.
const DEFAULT_TRACE_FILE: &str = ".claude/runtime/trace.log";

/// Binary names to search, in priority order.
const BINARY_NAMES: &[&str] = &["rustyclawd", "claude"];

/// Binaries known to support trace logging.
const TRACE_SUPPORTED: &[&str] = &["rustyclawd"];

/// Information about a detected Claude binary.
#[derive(Debug, Clone)]
pub struct BinaryInfo {
    /// Binary name (stem, e.g. `"rustyclawd"`).
    pub name: String,
    /// Full path to the binary.
    pub path: PathBuf,
    /// Detected version string, if any.
    pub version: Option<String>,
    /// Whether the binary supports `--log-file` trace logging.
    pub supports_trace: bool,
}

/// Manages detection and configuration of native Claude binaries.
///
/// Results are cached after first detection; call `invalidate_cache()` to
/// force re-detection.
pub struct ClaudeBinaryManager {
    cached: Option<Option<BinaryInfo>>,
}

impl ClaudeBinaryManager {
    pub fn new() -> Self {
        Self { cached: None }
    }

    /// Detect the best available binary.
    ///
    /// Search order: `CLAUDE_BINARY_PATH` env → `rustyclawd` → `claude`.
    pub fn detect_native_binary(&mut self) -> Option<&BinaryInfo> {
        self.cached.get_or_insert_with(Self::detect_inner).as_ref()
    }

    /// Build command-line arguments for launching the binary.
    ///
    /// # Errors
    /// Returns `Err` if `trace_file` contains a null byte.
    pub fn build_command(
        binary: &BinaryInfo,
        enable_trace: bool,
        trace_file: Option<&str>,
        additional_args: &[String],
    ) -> Result<Vec<String>, String> {
        if let Some(tf) = trace_file
            && tf.contains('\0')
        {
            return Err("Invalid trace file path: contains null bytes".into());
        }

        let mut cmd = vec![binary.path.display().to_string()];

        if enable_trace && binary.supports_trace {
            let file = trace_file.unwrap_or(DEFAULT_TRACE_FILE);
            cmd.extend_from_slice(&["--log-file".into(), file.into()]);
        }

        cmd.extend_from_slice(additional_args);
        Ok(cmd)
    }

    /// Invalidate the detection cache.
    pub fn invalidate_cache(&mut self) {
        self.cached = None;
    }

    // -- internal -----------------------------------------------------------

    fn detect_inner() -> Option<BinaryInfo> {
        // 1. Environment variable override
        if let Ok(env_path) = std::env::var("CLAUDE_BINARY_PATH") {
            let p = PathBuf::from(&env_path);
            if is_executable(&p) {
                return Some(create_binary_info(&p));
            }
        }

        // 2. Search $PATH
        for name in BINARY_NAMES {
            if let Some(found) = which(name)
                && is_executable(&found)
            {
                return Some(create_binary_info(&found));
            }
        }

        None
    }
}

impl Default for ClaudeBinaryManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Detect the version by running `<binary> --version`.
fn detect_version(path: &Path) -> Option<String> {
    let output = Command::new(path).arg("--version").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_version(&stdout)
}

/// Parse a semver-ish string from version output.
fn parse_version(text: &str) -> Option<String> {
    let patterns = [
        r"(\d+\.\d+\.\d+(?:-[a-zA-Z0-9]+)?)",
        r"v(\d+\.\d+\.\d+(?:-[a-zA-Z0-9]+)?)",
        r"(?i)version[:\s]+(\d+\.\d+\.\d+(?:-[a-zA-Z0-9]+)?)",
    ];

    for pat in &patterns {
        let re = Regex::new(pat).ok()?;
        if let Some(caps) = re.captures(text)
            && let Some(m) = caps.get(1)
        {
            return Some(m.as_str().to_string());
        }
    }
    None
}

fn create_binary_info(path: &Path) -> BinaryInfo {
    let name = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let supports_trace = TRACE_SUPPORTED.contains(&name.as_str());
    let version = detect_version(path);
    BinaryInfo {
        name,
        path: path.to_path_buf(),
        version,
        supports_trace,
    }
}

/// Minimal `which` implementation.
fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Check that a path exists and is executable (Unix).
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_semver() {
        assert_eq!(parse_version("1.2.3"), Some("1.2.3".into()));
        assert_eq!(parse_version("v1.0.0-beta"), Some("1.0.0-beta".into()));
        assert_eq!(parse_version("Version: 2.0.1"), Some("2.0.1".into()));
        assert_eq!(parse_version("no version"), None);
    }

    #[test]
    fn build_command_basic() {
        let info = BinaryInfo {
            name: "claude".into(),
            path: "/usr/bin/claude".into(),
            version: None,
            supports_trace: false,
        };
        let cmd = ClaudeBinaryManager::build_command(&info, false, None, &[]).unwrap();
        assert_eq!(cmd, vec!["/usr/bin/claude"]);
    }

    #[test]
    fn build_command_with_trace() {
        let info = BinaryInfo {
            name: "rustyclawd".into(),
            path: "/usr/bin/rustyclawd".into(),
            version: Some("1.0.0".into()),
            supports_trace: true,
        };
        let cmd = ClaudeBinaryManager::build_command(&info, true, Some("/log.txt"), &[]).unwrap();
        assert_eq!(cmd, vec!["/usr/bin/rustyclawd", "--log-file", "/log.txt"]);
    }

    #[test]
    fn build_command_trace_default_file() {
        let info = BinaryInfo {
            name: "rustyclawd".into(),
            path: "/bin/rustyclawd".into(),
            version: None,
            supports_trace: true,
        };
        let cmd = ClaudeBinaryManager::build_command(&info, true, None, &[]).unwrap();
        assert_eq!(cmd[2], DEFAULT_TRACE_FILE);
    }

    #[test]
    fn build_command_trace_not_supported() {
        let info = BinaryInfo {
            name: "claude".into(),
            path: "/bin/claude".into(),
            version: None,
            supports_trace: false,
        };
        let cmd = ClaudeBinaryManager::build_command(&info, true, None, &[]).unwrap();
        assert_eq!(cmd.len(), 1);
    }

    #[test]
    fn build_command_null_byte_rejected() {
        let info = BinaryInfo {
            name: "x".into(),
            path: "/x".into(),
            version: None,
            supports_trace: true,
        };
        let res = ClaudeBinaryManager::build_command(&info, true, Some("a\0b"), &[]);
        assert!(res.is_err());
    }

    #[test]
    fn build_command_with_extra_args() {
        let info = BinaryInfo {
            name: "claude".into(),
            path: "/bin/claude".into(),
            version: None,
            supports_trace: false,
        };
        let cmd = ClaudeBinaryManager::build_command(
            &info,
            false,
            None,
            &["--prompt".into(), "hello".into()],
        )
        .unwrap();
        assert_eq!(cmd, vec!["/bin/claude", "--prompt", "hello"]);
    }

    #[test]
    fn invalidate_cache_resets() {
        let mut mgr = ClaudeBinaryManager::new();
        // First call populates cache
        let _ = mgr.detect_native_binary();
        mgr.invalidate_cache();
        assert!(mgr.cached.is_none());
    }
}
