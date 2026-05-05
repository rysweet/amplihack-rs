//! Health check reporting for the amplihack runtime.
//!
//! Inspects critical dependencies and filesystem paths to produce a
//! structured health report indicating whether the installation is
//! healthy, degraded, or unhealthy.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Overall health status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// All checks passed.
    Healthy,
    /// Some non-critical checks failed.
    Degraded,
    /// Critical checks failed.
    Unhealthy,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded => write!(f, "degraded"),
            Self::Unhealthy => write!(f, "unhealthy"),
        }
    }
}

/// Outcome of a single health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckDetail {
    /// Name of the check (e.g. `"lbug"`, `"amplifier-bundle"`).
    pub name: String,
    /// `true` if the check passed.
    pub passed: bool,
    /// Human-readable detail or error message.
    pub message: String,
}

/// Aggregated health report (immutable once constructed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// Overall status derived from individual checks.
    pub status: HealthStatus,
    /// Number of checks that passed.
    pub checks_passed: usize,
    /// Number of checks that failed.
    pub checks_failed: usize,
    /// Individual check details.
    pub details: Vec<CheckDetail>,
}

/// Run all health checks and return a consolidated report.
pub fn check_health() -> HealthReport {
    let mut details = Vec::new();

    // Critical dependency checks
    details.push(_check_dependency("git", &["--version"]));
    details.push(_check_dependency("gh", &["--version"]));

    // Path checks
    let root = _project_root();
    if let Some(ref root) = root {
        details.push(_check_path(
            "amplifier-bundle",
            &root.join("amplifier-bundle"),
        ));
        details.push(_check_path(
            "recipes",
            &root.join("amplifier-bundle").join("recipes"),
        ));
        details.push(_check_path(
            "workflows",
            &root.join("amplifier-bundle").join("workflows"),
        ));
    } else {
        details.push(CheckDetail {
            name: "project-root".into(),
            passed: false,
            message: "Could not determine project root".into(),
        });
    }

    // Check copilot home
    let copilot_home = dirs_or_home().map(|h| h.join(".copilot"));
    if let Some(ref ch) = copilot_home {
        details.push(_check_path("copilot-home", ch));
    }

    let checks_passed = details.iter().filter(|d| d.passed).count();
    let checks_failed = details.iter().filter(|d| !d.passed).count();

    // Critical failures → unhealthy, any failure → degraded
    let has_critical_failure = details
        .iter()
        .any(|d| !d.passed && (d.name == "git" || d.name == "amplifier-bundle"));

    let status = if has_critical_failure {
        HealthStatus::Unhealthy
    } else if checks_failed > 0 {
        HealthStatus::Degraded
    } else {
        HealthStatus::Healthy
    };

    debug!(
        status = %status,
        passed = checks_passed,
        failed = checks_failed,
        "Health check complete"
    );

    HealthReport {
        status,
        checks_passed,
        checks_failed,
        details,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check whether a CLI dependency is available on PATH.
fn _check_dependency(name: &str, args: &[&str]) -> CheckDetail {
    match std::process::Command::new(name)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .status()
    {
        Ok(status) if status.success() => {
            debug!(dep = name, "Dependency available");
            CheckDetail {
                name: name.into(),
                passed: true,
                message: format!("{name} is available"),
            }
        }
        Ok(status) => {
            warn!(dep = name, code = ?status.code(), "Dependency check returned non-zero");
            CheckDetail {
                name: name.into(),
                passed: false,
                message: format!("{name} exited with {status}"),
            }
        }
        Err(e) => {
            warn!(dep = name, error = %e, "Dependency not found");
            CheckDetail {
                name: name.into(),
                passed: false,
                message: format!("{name} not found: {e}"),
            }
        }
    }
}

/// Check whether a filesystem path exists.
fn _check_path(name: &str, path: &Path) -> CheckDetail {
    if path.exists() {
        CheckDetail {
            name: name.into(),
            passed: true,
            message: format!("{} exists", path.display()),
        }
    } else {
        CheckDetail {
            name: name.into(),
            passed: false,
            message: format!("{} not found", path.display()),
        }
    }
}

/// Locate the project root by walking up from the current directory.
fn _project_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut dir = cwd.as_path();
    loop {
        if dir.join("amplifier-bundle").is_dir() {
            return Some(dir.to_path_buf());
        }
        if dir.join("Cargo.toml").exists() && dir.join("crates").is_dir() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Return the user home directory.
fn dirs_or_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_dependency_git_available() {
        let detail = _check_dependency("git", &["--version"]);
        // git should be available in any dev environment
        assert!(detail.passed, "git should be available: {}", detail.message);
        assert_eq!(detail.name, "git");
    }

    #[test]
    fn check_dependency_nonexistent_binary() {
        let detail = _check_dependency("totally-nonexistent-binary-xyz", &["--version"]);
        assert!(!detail.passed);
        assert!(detail.message.contains("not found"));
    }

    #[test]
    fn check_path_existing() {
        let detail = _check_path("root", Path::new("/"));
        assert!(detail.passed);
    }

    #[test]
    fn check_path_missing() {
        let detail = _check_path("phantom", Path::new("/nonexistent/phantom/path"));
        assert!(!detail.passed);
        assert!(detail.message.contains("not found"));
    }

    #[test]
    fn health_report_has_consistent_counts() {
        let report = check_health();
        assert_eq!(
            report.checks_passed + report.checks_failed,
            report.details.len()
        );
    }

    #[test]
    fn health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(HealthStatus::Degraded.to_string(), "degraded");
        assert_eq!(HealthStatus::Unhealthy.to_string(), "unhealthy");
    }

    #[test]
    fn health_report_serialization() {
        let report = HealthReport {
            status: HealthStatus::Degraded,
            checks_passed: 3,
            checks_failed: 1,
            details: vec![CheckDetail {
                name: "test".into(),
                passed: true,
                message: "ok".into(),
            }],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"degraded\""));
        let deser: HealthReport = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.status, HealthStatus::Degraded);
    }
}
