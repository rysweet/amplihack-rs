//! Binary finder — locates tool binaries (claude, copilot, codex, amplifier) in PATH.
//!
//! Uses `which`-style lookup with version verification. No fallbacks:
//! if the binary isn't found, we error out.

use crate::util::strip_ansi;
use anyhow::{Result, bail};
use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    /// 2. `{TOOL}_BINARY_PATH` env var (Python parity, e.g. CLAUDE_BINARY_PATH)
    /// 3. PATH search for known binary names
    ///
    /// Errors if the binary is not found. No fallbacks.
    pub fn find(tool: &str) -> Result<BinaryInfo> {
        let tool_upper = tool.to_uppercase();

        // Check explicit override env var (amplihack-prefixed)
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

        // Check tool-native env var (Python parity, e.g. CLAUDE_BINARY_PATH)
        let native_env_key = format!("{tool_upper}_BINARY_PATH");
        if let Ok(explicit_path) = env::var(&native_env_key) {
            let path = PathBuf::from(&explicit_path);
            if path.exists() {
                let version = detect_version(&path);
                return Ok(BinaryInfo {
                    name: tool.to_string(),
                    path,
                    version,
                });
            }
            bail!("{native_env_key}={explicit_path} does not exist");
        }

        // Search PATH for known binary names, then fall back to the
        // directories where `amplihack install_tool` actually writes
        // binaries. Without the fallback we re-install npm/cargo tools on
        // every launch when the user's shell PATH hasn't been updated yet
        // (e.g. a pre-existing tmux or ssh session whose PATH was captured
        // before `persist_path_hint` wrote to `.bashrc`).
        let candidates = binary_candidates(tool);
        let mut search_dirs = search_path_dirs();
        let fallback_dirs = install_fallback_dirs();
        for dir in &fallback_dirs {
            if !search_dirs.contains(dir) {
                search_dirs.push(dir.clone());
            }
        }

        for candidate in &candidates {
            for dir in &search_dirs {
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
            "binary for '{tool}' not found in PATH or known install dirs (searched for: {})",
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

/// Known locations where `amplihack install_tool` writes binaries, regardless
/// of whether those directories are on the shell's `$PATH`.
///
/// Keeps binary discovery working for users whose `.bashrc` / `.zshrc` PATH
/// update hasn't been sourced yet (persistent tmux sessions, SSH sessions
/// started before the first amplihack install, Docker shells that inherit a
/// minimal PATH, etc.).
fn install_fallback_dirs() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let mut dirs = Vec::new();
    if let Some(home) = home {
        // npm global prefix set by `install_npm_package`.
        dirs.push(home.join(".npm-global").join("bin"));
        // `cargo install` default.
        dirs.push(home.join(".cargo").join("bin"));
        // `uv tool install` + legacy Python amplihack install target.
        dirs.push(home.join(".local").join("bin"));
    }
    dirs
}

/// Maximum number of characters to retain from a detected version string.
const MAX_VERSION_LEN: usize = 200;

/// Run `binary --version` and extract a version string.
fn detect_version(path: &Path) -> Option<String> {
    let output = Command::new(path).arg("--version").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Extract version-like string: first line, stripped of ANSI sequences.
    let first_line = stdout.lines().next()?.trim();
    let version = strip_ansi(first_line);
    // Truncate to avoid unbounded strings from malicious or misbehaving binaries.
    if version.len() > MAX_VERSION_LEN {
        Some(version[..MAX_VERSION_LEN].to_string())
    } else {
        Some(version)
    }
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
    fn find_falls_back_to_npm_global_when_not_on_path() {
        // Simulate the hyenas2 scenario: copilot is installed at
        // ~/.npm-global/bin/copilot but the shell's $PATH was captured
        // before .bashrc was updated, so the shell doesn't include it.
        let temp = tempfile::tempdir().unwrap();
        let fake_home = temp.path();
        let bin_dir = fake_home.join(".npm-global/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let fake_tool = bin_dir.join("needle-tool-xyz");
        std::fs::write(&fake_tool, "#!/bin/sh\necho needle\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Deliberately strip any .npm-global from PATH and point HOME at
        // the temp dir so install_fallback_dirs() finds the binary.
        // SAFETY: Serialized by #[ignore]-free unit tests running with --test-threads=1
        // fallbacks plus the home_env_lock is overkill here; no other test
        // looks at `needle-tool-xyz`.
        let prev_home = env::var_os("HOME");
        let prev_path = env::var_os("PATH");
        unsafe {
            env::set_var("HOME", fake_home);
            env::set_var("PATH", "/nonexistent-just-for-this-test");
        }

        let result = BinaryFinder::find("needle-tool-xyz");

        unsafe {
            if let Some(v) = prev_home {
                env::set_var("HOME", v);
            } else {
                env::remove_var("HOME");
            }
            if let Some(v) = prev_path {
                env::set_var("PATH", v);
            } else {
                env::remove_var("PATH");
            }
        }

        let info = result.expect("fallback dir lookup should succeed");
        assert_eq!(info.path, fake_tool);
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
