//! Types and helper functions for simple TUI testing.

use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of a single TUI test execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestResult {
    /// Identifier matching the originating [`TUITestCase`].
    pub test_id: String,
    /// `"passed"` or `"failed"`.
    pub status: String,
    /// Wall-clock duration in seconds.
    pub duration: f64,
    /// Human-readable diagnostic message.
    pub message: String,
}

impl TestResult {
    /// Create a *passed* result.
    pub fn passed(test_id: impl Into<String>, duration: f64, message: impl Into<String>) -> Self {
        Self {
            test_id: test_id.into(),
            status: "passed".into(),
            duration,
            message: message.into(),
        }
    }

    /// Create a *failed* result.
    pub fn failed(test_id: impl Into<String>, duration: f64, message: impl Into<String>) -> Self {
        Self {
            test_id: test_id.into(),
            status: "failed".into(),
            duration,
            message: message.into(),
        }
    }

    /// Returns `true` when the test passed.
    pub fn is_passed(&self) -> bool {
        self.status == "passed"
    }
}

/// A single test case describing commands to execute.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TUITestCase {
    /// Unique test identifier.
    pub test_id: String,
    /// Human-readable test name.
    pub name: String,
    /// Shell commands to execute sequentially.
    pub commands: Vec<String>,
    /// Per-command timeout in seconds (default: 10).
    pub timeout: u64,
}

impl TUITestCase {
    /// Create a new test case with the default timeout of 10 s.
    pub fn new(test_id: impl Into<String>, name: impl Into<String>, commands: Vec<String>) -> Self {
        Self {
            test_id: test_id.into(),
            name: name.into(),
            commands,
            timeout: 10,
        }
    }

    /// Create a new test case with a custom timeout.
    pub fn with_timeout(
        test_id: impl Into<String>,
        name: impl Into<String>,
        commands: Vec<String>,
        timeout: u64,
    ) -> Self {
        Self {
            test_id: test_id.into(),
            name: name.into(),
            commands,
            timeout,
        }
    }
}

// ---------------------------------------------------------------------------
// CI detection
// ---------------------------------------------------------------------------

/// Well-known CI environment variables.
const CI_ENV_VARS: &[&str] = &[
    "CI",
    "GITHUB_ACTIONS",
    "TRAVIS",
    "CIRCLECI",
    "JENKINS_URL",
    "GITLAB_CI",
    "TF_BUILD",
    "BUILDKITE",
];

/// Returns `true` if the current process is running inside a CI system.
pub fn is_ci_environment() -> bool {
    CI_ENV_VARS
        .iter()
        .any(|var| std::env::var_os(var).is_some())
}

// ---------------------------------------------------------------------------
// Gadugi availability
// ---------------------------------------------------------------------------

/// Check whether the `gadugi-test` binary is reachable through `npx`.
///
/// Returns `false` in CI environments to avoid hanging on auto-install prompts.
pub fn check_gadugi_available() -> bool {
    if is_ci_environment() {
        return false;
    }

    // Verify npx itself is available.
    let npx_ok = Command::new("npx")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !npx_ok {
        return false;
    }

    // Probe gadugi-test with NPX_NO_INSTALL to prevent silent downloads.
    Command::new("npx")
        .args(["gadugi-test", "--help"])
        .env("NPX_NO_INSTALL", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Factory / convenience helpers
// ---------------------------------------------------------------------------

/// Create a [`SimpleTUITester`](super::SimpleTUITester) with the given (or default) output directory.
///
/// # Errors
///
/// Returns an I/O error if the output directory cannot be created.
pub fn create_tui_tester(output_dir: Option<PathBuf>) -> std::io::Result<super::SimpleTUITester> {
    super::SimpleTUITester::new(output_dir.unwrap_or_else(|| PathBuf::from("./tui_output")))
}

/// Convenience: build a [`TUITestCase`] that invokes `amplihack <args>`.
pub fn create_amplihack_test(test_id: impl Into<String>, args: &str) -> TUITestCase {
    let id = test_id.into();
    TUITestCase::new(
        id.clone(),
        format!("AmplIHack {args}"),
        vec![format!("amplihack {args}")],
    )
}
