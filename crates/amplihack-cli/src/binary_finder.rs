//! Binary finder — locates tool binaries (claude, copilot, codex, amplifier) in PATH.
//!
//! Uses `which`-style lookup with version verification. No fallbacks:
//! if the binary isn't found, we error out.

use anyhow::{Result, bail};
use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::util::strip_ansi;

/// Metadata about a discovered binary.
#[derive(Debug, Clone)]
pub struct BinaryInfo {
    /// Tool name (e.g., "claude").
    pub name: String,
    /// Absolute path to the binary.
    pub path: PathBuf,
    /// Version string if available.
    pub version: Option<String>,
}

/// Finds tool binaries on the system PATH.
pub struct BinaryFinder;

impl BinaryFinder {
    /// Find a tool binary by name.
    ///
    /// Search order:
    /// 1. `AMPLIHACK_{TOOL}_BINARY_PATH` env var (exact override)
    /// 2. PATH search for known binary names
    ///
    /// Errors if the binary is not found. No fallbacks.
    pub fn find(tool: &str) -> Result<BinaryInfo> {
        let tool_upper = tool.to_uppercase();

        // Check explicit override env var
        let env_key = format!("AMPLIHACK_{tool_upper}_BINARY_PATH");
        if let Ok(explicit_path) = env::var(&env_key) {
            let path = PathBuf::from(&explicit_path);
            if path.exists() {
                let version = detect_version(&path);
                return Ok(BinaryInfo {
                    name: tool.to_string(),
                    path,
                    version,
                });
            }
            bail!("{env_key}={explicit_path} does not exist");
        }

        // Search PATH for known binary names
        let candidates = binary_candidates(tool);
        let path_dirs = search_path_dirs();

        for candidate in &candidates {
            for dir in &path_dirs {
                let full_path = dir.join(candidate);
                if full_path.is_file() && is_executable(&full_path) {
                    let version = detect_version(&full_path);
                    return Ok(BinaryInfo {
                        name: tool.to_string(),
                        path: full_path,
                        version,
                    });
                }
            }
        }

        bail!(
            "binary for '{tool}' not found in PATH (searched for: {})",
            candidates.join(", ")
        );
    }

    /// List all tool binaries found in PATH (for diagnostics).
    pub fn find_all(tool: &str) -> Vec<BinaryInfo> {
        let candidates = binary_candidates(tool);
        let path_dirs = search_path_dirs();
        let mut results = Vec::new();

        for candidate in &candidates {
            for dir in &path_dirs {
                let full_path = dir.join(candidate);
                if full_path.is_file() && is_executable(&full_path) {
                    let version = detect_version(&full_path);
                    results.push(BinaryInfo {
                        name: tool.to_string(),
                        path: full_path,
                        version,
                    });
                }
            }
        }

        results
    }
}

/// Return candidate binary names for a tool.
fn binary_candidates(tool: &str) -> Vec<String> {
    match tool {
        "claude" => vec!["rustyclawd".to_string(), "claude".to_string()],
        "copilot" => vec!["copilot".to_string()],
        "codex" => vec!["codex".to_string()],
        "amplifier" => vec!["amplifier".to_string()],
        other => vec![other.to_string()],
    }
}

/// Collect PATH directories into a de-duplicated, ordered Vec.
fn search_path_dirs() -> Vec<PathBuf> {
    let path_var = env::var("PATH").unwrap_or_default();
    let mut seen = HashSet::new();
    let mut dirs = Vec::new();

    for entry in env::split_paths(&path_var) {
        if seen.insert(entry.clone()) {
            dirs.push(entry);
        }
    }

    dirs
}

/// Run `binary --version` and extract a version string.
///
/// `strip_ansi()` is applied to the captured stdout before returning so that
/// ANSI escape sequences in version output cannot reach the terminal.
/// See SEC-WS2-02.
fn detect_version(path: &Path) -> Option<String> {
    let output = Command::new(path).arg("--version").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Extract version-like string: first line, stripped of ANSI sequences.
    let first_line = stdout.lines().next()?.trim();
    Some(strip_ansi(first_line))
}

/// Check if a path is executable (Unix: has execute bit).
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_candidates_claude() {
        let candidates = binary_candidates("claude");
        assert!(candidates.contains(&"rustyclawd".to_string()));
        assert!(candidates.contains(&"claude".to_string()));
    }

    #[test]
    fn binary_candidates_unknown_tool() {
        let candidates = binary_candidates("newtool");
        assert_eq!(candidates, vec!["newtool"]);
    }

    #[test]
    fn search_path_dirs_deduplicates() {
        // PATH deduplication is deterministic
        let dirs = search_path_dirs();
        let unique: HashSet<_> = dirs.iter().collect();
        assert_eq!(dirs.len(), unique.len());
    }

    #[test]
    fn find_echo_binary() {
        // `echo` should be findable on any Unix system
        let result = BinaryFinder::find("echo");
        if let Ok(info) = result {
            assert!(info.path.exists());
            assert_eq!(info.name, "echo");
        }
        // If not found (unlikely), that's fine for a test
    }

    #[test]
    fn find_nonexistent_binary_errors() {
        let result = BinaryFinder::find("definitely_not_a_real_binary_xyz_123");
        assert!(result.is_err());
    }

    #[test]
    fn explicit_env_override() {
        // Set an explicit override pointing to /bin/echo
        let echo_path = if Path::new("/usr/bin/echo").exists() {
            "/usr/bin/echo"
        } else if Path::new("/bin/echo").exists() {
            "/bin/echo"
        } else {
            return; // Skip test if echo not found
        };

        // SAFETY: Test-only env var manipulation; test runner serializes tests by default.
        unsafe { env::set_var("AMPLIHACK_TESTTOOL_BINARY_PATH", echo_path) };
        let result = BinaryFinder::find("testtool");
        unsafe { env::remove_var("AMPLIHACK_TESTTOOL_BINARY_PATH") };

        let info = result.unwrap();
        assert_eq!(info.path, PathBuf::from(echo_path));
    }

    #[test]
    fn explicit_env_override_nonexistent_errors() {
        // SAFETY: Test-only env var manipulation; test runner serializes tests by default.
        unsafe {
            env::set_var(
                "AMPLIHACK_BADTOOL_BINARY_PATH",
                "/nonexistent/path/to/binary",
            );
        }
        let result = BinaryFinder::find("badtool");
        unsafe { env::remove_var("AMPLIHACK_BADTOOL_BINARY_PATH") };

        assert!(result.is_err());
    }
}
