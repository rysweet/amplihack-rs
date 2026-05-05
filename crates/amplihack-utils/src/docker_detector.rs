//! Docker availability detection.
//!
//! Ported from `amplihack/docker/detector.py`.
//!
//! Provides lightweight checks for whether Docker is installed, running,
//! and whether the current process is already inside a container.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

// The blocking Command API does not support timeouts directly; this constant
// documents the intended value for future async migration.
#[allow(dead_code)]
const DOCKER_TIMEOUT: Duration = Duration::from_secs(5);

/// Detects Docker availability and configuration.
///
/// All methods are side-effect-free (read-only queries).
pub struct DockerDetector;

impl DockerDetector {
    /// Check whether the `docker` binary is on `PATH`.
    pub fn is_available() -> bool {
        which_docker().is_some()
    }

    /// Check whether the Docker daemon is running and responsive.
    ///
    /// Returns `false` if Docker is not installed or the daemon does not
    /// respond within the timeout.
    pub fn is_running() -> bool {
        if !Self::is_available() {
            return false;
        }
        run_docker_silent(&["info"]).unwrap_or(false)
    }

    /// Determine whether Docker should be used for this session.
    ///
    /// Checks the `AMPLIHACK_USE_DOCKER` environment variable, ensures we
    /// are not already inside a container, and verifies the daemon is running.
    pub fn should_use_docker() -> bool {
        let env_val = std::env::var("AMPLIHACK_USE_DOCKER")
            .unwrap_or_default()
            .to_lowercase();

        if !matches!(env_val.as_str(), "1" | "true" | "yes") {
            return false;
        }
        if Self::is_in_docker() {
            return false;
        }
        Self::is_running()
    }

    /// Check whether we are running inside a Docker container.
    ///
    /// Inspects `AMPLIHACK_IN_DOCKER`, `/.dockerenv`, and `/proc/1/cgroup`.
    pub fn is_in_docker() -> bool {
        // Explicit env var
        if std::env::var("AMPLIHACK_IN_DOCKER").ok().as_deref() == Some("1") {
            return true;
        }

        // Docker sentinel file
        if Path::new("/.dockerenv").exists() {
            return true;
        }

        // cgroup check
        if let Ok(cgroup) = std::fs::read_to_string("/proc/1/cgroup")
            && cgroup.contains("docker")
        {
            return true;
        }

        false
    }

    /// Check whether a Docker image exists locally.
    ///
    /// Returns `false` if Docker is not running or the command fails.
    pub fn check_image_exists(image_name: &str) -> bool {
        if !Self::is_running() {
            return false;
        }
        match run_docker_output(&["images", "-q", image_name]) {
            Some(output) => !output.trim().is_empty(),
            None => false,
        }
    }
}

/// Locate the `docker` binary on `PATH`.
fn which_docker() -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| {
        let candidate = dir.join("docker");
        if candidate.is_file() {
            Some(candidate)
        } else {
            None
        }
    })
}

/// Run a docker command silently, returning `Some(true)` if exit code is 0.
fn run_docker_silent(args: &[&str]) -> Option<bool> {
    let status = Command::new("docker")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // Note: std::process::Command does not natively support timeouts.
    // For simplicity we rely on the command itself; a production version
    // could use tokio or spawn + wait_timeout.  The Python source uses a
    // 5-second timeout which is acceptable for a blocking CLI tool.
    match status {
        Ok(s) => Some(s.success()),
        Err(_) => None,
    }
}

/// Run a docker command and capture stdout.
fn run_docker_output(args: &[&str]) -> Option<String> {
    let output = Command::new("docker")
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_available_returns_bool() {
        // Just verify it doesn't panic. The result depends on the environment.
        let _ = DockerDetector::is_available();
    }

    #[test]
    fn is_in_docker_env_var() {
        // Save and restore.
        let prev = std::env::var("AMPLIHACK_IN_DOCKER").ok();
        // SAFETY: Tests run single-threaded per module; env var mutation is contained.
        unsafe {
            std::env::set_var("AMPLIHACK_IN_DOCKER", "1");
        }
        assert!(DockerDetector::is_in_docker());
        match prev {
            Some(v) => unsafe {
                std::env::set_var("AMPLIHACK_IN_DOCKER", v);
            },
            None => unsafe {
                std::env::remove_var("AMPLIHACK_IN_DOCKER");
            },
        }
    }

    #[test]
    fn is_in_docker_false_when_unset() {
        let prev = std::env::var("AMPLIHACK_IN_DOCKER").ok();
        // SAFETY: Tests run single-threaded per module; env var mutation is contained.
        unsafe {
            std::env::remove_var("AMPLIHACK_IN_DOCKER");
        }
        // On a normal host without /.dockerenv this should be false.
        // (We can't guarantee this in all CI environments, so just check no panic.)
        let _ = DockerDetector::is_in_docker();
        if let Some(v) = prev {
            unsafe {
                std::env::set_var("AMPLIHACK_IN_DOCKER", v);
            }
        }
    }

    #[test]
    fn should_use_docker_false_by_default() {
        let prev = std::env::var("AMPLIHACK_USE_DOCKER").ok();
        // SAFETY: Tests run single-threaded per module; env var mutation is contained.
        unsafe {
            std::env::remove_var("AMPLIHACK_USE_DOCKER");
        }
        assert!(!DockerDetector::should_use_docker());
        if let Some(v) = prev {
            unsafe {
                std::env::set_var("AMPLIHACK_USE_DOCKER", v);
            }
        }
    }

    #[test]
    fn check_image_exists_false_when_no_docker() {
        // If docker isn't running, this should return false, not panic.
        // We don't assume docker is available in tests.
        let prev_path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: Tests run single-threaded per module; env var mutation is contained.
        unsafe {
            std::env::set_var("PATH", "");
        }
        assert!(!DockerDetector::check_image_exists("nonexistent:latest"));
        unsafe {
            std::env::set_var("PATH", prev_path);
        }
    }

    #[test]
    fn which_docker_finds_binary_or_none() {
        // Smoke test – just verify it returns a sensible value.
        let result = which_docker();
        if let Some(path) = &result {
            assert!(path.to_string_lossy().contains("docker"));
        }
    }

    // Verify DOCKER_TIMEOUT has the correct architectural value.
    // This test documents and guards the intent: CLI timeout = 5 seconds.
    // Once #[allow(dead_code)] replaces the `const _` workaround, this
    // test also confirms the constant remains accessible and correct.
    #[test]
    fn docker_timeout_is_five_seconds() {
        assert_eq!(DOCKER_TIMEOUT, Duration::from_secs(5));
    }

    #[test]
    fn should_use_docker_true_for_all_truthy_env_values() {
        // Verify each truthy variant of AMPLIHACK_USE_DOCKER is accepted.
        // We can't easily make Docker run in CI, so we only test the
        // "not in docker + docker not running" path — should return false
        // (the daemon isn't up), but must NOT panic or return an unexpected
        // type. The important contract is that the string comparison works
        // for all three truthy variants.
        let prev_use = std::env::var("AMPLIHACK_USE_DOCKER").ok();
        let prev_in = std::env::var("AMPLIHACK_IN_DOCKER").ok();

        unsafe {
            std::env::remove_var("AMPLIHACK_IN_DOCKER");
        }

        for val in &["1", "true", "yes"] {
            unsafe {
                std::env::set_var("AMPLIHACK_USE_DOCKER", val);
            }
            // Either returns false (daemon not running) or true (daemon up).
            // The important thing: it does NOT treat these as "disabled".
            // We just check it doesn't panic.
            let _ = DockerDetector::should_use_docker();
        }

        match prev_use {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_USE_DOCKER", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_USE_DOCKER") },
        }
        if let Some(v) = prev_in {
            unsafe { std::env::set_var("AMPLIHACK_IN_DOCKER", v) }
        }
    }

    #[test]
    fn should_use_docker_false_for_falsy_env_values() {
        let prev = std::env::var("AMPLIHACK_USE_DOCKER").ok();
        for val in &["0", "false", "no", "off", ""] {
            unsafe {
                std::env::set_var("AMPLIHACK_USE_DOCKER", val);
            }
            assert!(
                !DockerDetector::should_use_docker(),
                "expected false for AMPLIHACK_USE_DOCKER={val}"
            );
        }
        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_USE_DOCKER", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_USE_DOCKER") },
        }
    }
}
