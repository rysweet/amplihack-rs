//! Docker container manager for amplihack execution.
//!
//! Ported from `amplihack/docker/manager.py`.
//!
//! Provides [`DockerManager`] which builds Docker images and runs amplihack
//! commands inside containers with security restrictions and resource limits.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use regex::Regex;
use tracing::{debug, error, info, warn};

use crate::docker_detector::DockerDetector;

/// Default Docker image name.
const IMAGE_NAME: &str = "amplihack:latest";

/// Manages Docker containers for amplihack execution.
pub struct DockerManager;

impl DockerManager {
    /// Build the Docker image if it does not already exist.
    ///
    /// Looks for a `Dockerfile` relative to `project_root`. Returns `true` on
    /// success (including when the image already exists).
    pub fn build_image(project_root: &Path) -> bool {
        if !DockerDetector::is_running() {
            error!("Docker is not running");
            return false;
        }

        if DockerDetector::check_image_exists(IMAGE_NAME) {
            debug!("Docker image already exists: {IMAGE_NAME}");
            return true;
        }

        info!("Building Docker image: {IMAGE_NAME}");

        let dockerfile = project_root.join("Dockerfile");
        if !dockerfile.exists() {
            error!("Dockerfile not found at {}", dockerfile.display());
            return false;
        }

        match Command::new("docker")
            .args([
                "build",
                "-t",
                IMAGE_NAME,
                "-f",
                &dockerfile.to_string_lossy(),
                &project_root.to_string_lossy(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .status()
        {
            Ok(status) if status.success() => {
                info!("Successfully built Docker image: {IMAGE_NAME}");
                true
            }
            Ok(_) => {
                error!("Docker build failed");
                false
            }
            Err(e) => {
                error!("Error running docker build: {e}");
                false
            }
        }
    }

    /// Run an amplihack command inside a Docker container.
    ///
    /// Mounts `cwd` (or the current directory) as `/workspace`, forwards
    /// relevant environment variables, and applies security constraints.
    ///
    /// Returns the container's exit code, or `1` on launch failure.
    pub fn run_command(args: &[&str], cwd: Option<&Path>) -> i32 {
        if !DockerDetector::is_running() {
            error!("Docker is not running");
            return 1;
        }

        let work_dir = match cwd {
            Some(p) => p.to_path_buf(),
            None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };
        let work_dir = work_dir.canonicalize().unwrap_or_else(|_| work_dir.clone());

        let mut docker_cmd = vec![
            "run".to_string(),
            "--rm".into(),
            "--interactive".into(),
            // Security options
            "--security-opt".into(),
            "no-new-privileges".into(),
            // Resource limits
            "--memory".into(),
            "4g".into(),
            "--cpus".into(),
            "2".into(),
        ];

        // Run as current user (Unix only).
        #[cfg(unix)]
        {
            let uid = unsafe { libc::getuid() };
            let gid = unsafe { libc::getgid() };
            docker_cmd.push("--user".into());
            docker_cmd.push(format!("{uid}:{gid}"));
        }

        // Mount workspace.
        docker_cmd.push("-v".into());
        docker_cmd.push(format!("{}:/workspace", work_dir.display()));
        docker_cmd.push("-w".into());
        docker_cmd.push("/workspace".into());

        // Forward validated environment variables.
        for (key, value) in get_env_vars() {
            docker_cmd.push("-e".into());
            docker_cmd.push(format!("{key}={value}"));
        }

        // Image and arguments.
        docker_cmd.push(IMAGE_NAME.into());
        docker_cmd.extend(args.iter().map(|s| (*s).to_string()));

        debug!("Running Docker command: docker {}", docker_cmd.join(" "));

        match Command::new("docker")
            .args(&docker_cmd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
        {
            Ok(status) => status.code().unwrap_or(1),
            Err(e) => {
                error!("Error running Docker container: {e}");
                1
            }
        }
    }

    /// Check whether Docker should be used for this session.
    ///
    /// Delegates to [`DockerDetector::should_use_docker`].
    pub fn should_use_docker() -> bool {
        DockerDetector::should_use_docker()
    }
}

// ---------------------------------------------------------------------------
// Environment variable handling
// ---------------------------------------------------------------------------

/// Sanitise an environment variable value by removing control characters.
fn sanitize_env_value(value: &str) -> String {
    value
        .chars()
        .filter(|&c| {
            // Keep printable chars plus newline and tab.
            c >= ' ' || c == '\n' || c == '\t'
        })
        .collect()
}

/// Validate an API key against known provider patterns.
fn validate_api_key(key_name: &str, value: &str) -> bool {
    if value.len() < 10 {
        return false;
    }

    match key_name {
        "ANTHROPIC_API_KEY" | "OPENAI_API_KEY" => Regex::new(r"^sk-[a-zA-Z0-9\-_]+$")
            .map(|re| re.is_match(value))
            .unwrap_or(false),
        "GITHUB_TOKEN" | "GH_TOKEN" => {
            let prefixed = Regex::new(r"^(ghp_|ghs_|github_pat_|gho_|ghu_)[a-zA-Z0-9_]+$")
                .map(|re| re.is_match(value))
                .unwrap_or(false);
            let classic = Regex::new(r"^[a-f0-9]{40}$")
                .map(|re| re.is_match(value))
                .unwrap_or(false);
            prefixed || classic
        }
        _ => Regex::new(r"^[a-zA-Z0-9\-_./+=]+$")
            .map(|re| re.is_match(value))
            .unwrap_or(false),
    }
}

/// Collect environment variables to forward into the container.
fn get_env_vars() -> Vec<(String, String)> {
    let mut vars = Vec::new();

    // API keys with validation.
    for key in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GITHUB_TOKEN",
        "GH_TOKEN",
    ] {
        if let Ok(value) = std::env::var(key) {
            let sanitized = sanitize_env_value(&value);
            if validate_api_key(key, &sanitized) {
                vars.push((key.into(), sanitized));
            } else {
                warn!("Invalid format for {key}, skipping");
            }
        }
    }

    // AMPLIHACK_* variables (except the Docker trigger).
    for (key, value) in std::env::vars() {
        if key.starts_with("AMPLIHACK_") && key != "AMPLIHACK_USE_DOCKER" {
            vars.push((key, sanitize_env_value(&value)));
        }
    }

    // Terminal settings.
    if let Ok(term) = std::env::var("TERM") {
        vars.push(("TERM".into(), sanitize_env_value(&term)));
    }

    vars
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_removes_control_chars() {
        let input = "hello\x00world\x01\ttab\nnewline";
        let result = sanitize_env_value(input);
        assert_eq!(result, "helloworld\ttab\nnewline");
    }

    #[test]
    fn validate_anthropic_key() {
        assert!(validate_api_key(
            "ANTHROPIC_API_KEY",
            "sk-ant-api03-abcdef1234567890"
        ));
        assert!(!validate_api_key("ANTHROPIC_API_KEY", "invalid"));
        assert!(!validate_api_key("ANTHROPIC_API_KEY", "short"));
    }

    #[test]
    fn validate_github_token_prefixed() {
        assert!(validate_api_key(
            "GITHUB_TOKEN",
            "ghp_1234567890abcdef1234567890abcdef12345678"
        ));
        assert!(validate_api_key("GITHUB_TOKEN", "ghs_1234567890abcdef"));
        assert!(validate_api_key(
            "GITHUB_TOKEN",
            "github_pat_1234567890abcdef"
        ));
    }

    #[test]
    fn validate_github_token_classic() {
        assert!(validate_api_key(
            "GITHUB_TOKEN",
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
        ));
    }

    #[test]
    fn validate_unknown_key() {
        assert!(validate_api_key("CUSTOM_KEY", "some-valid_key.value/+=="));
        assert!(!validate_api_key("CUSTOM_KEY", "has spaces!"));
    }

    #[test]
    fn should_use_docker_false_by_default() {
        // In test env without AMPLIHACK_USE_DOCKER, should return false.
        assert!(!DockerManager::should_use_docker());
    }
}
