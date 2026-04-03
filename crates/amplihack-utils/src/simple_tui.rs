//! Simple TUI testing framework for amplihack.
//!
//! Ported from `amplihack/testing/simple_tui.py`.
//!
//! Provides a lightweight test harness that can run CLI commands either through
//! the gadugi-agentic-test framework (when available via `npx`) or via direct
//! subprocess execution as a fallback.  CI environments are detected
//! automatically so that interactive gadugi downloads are never attempted on
//! build servers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[path = "simple_tui_types.rs"]
mod types;
pub use types::*;

// ---------------------------------------------------------------------------
// SimpleTUITester
// ---------------------------------------------------------------------------

/// Lightweight test runner that exercises CLI commands and records results.
///
/// When gadugi-agentic-test is available it delegates to that framework;
/// otherwise it falls back to running each command as a subprocess.
pub struct SimpleTUITester {
    output_dir: PathBuf,
    test_cases: HashMap<String, TUITestCase>,
    results: HashMap<String, TestResult>,
    /// When `true`, always use the subprocess fallback path regardless of
    /// gadugi availability.  Useful in tests and headless environments.
    force_subprocess: bool,
}

impl SimpleTUITester {
    /// Create a tester that writes artefacts to `output_dir`.
    ///
    /// The directory is created if it does not exist.
    pub fn new(output_dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let output_dir = output_dir.into();
        std::fs::create_dir_all(&output_dir)?;
        Ok(Self {
            output_dir,
            test_cases: HashMap::new(),
            results: HashMap::new(),
            force_subprocess: false,
        })
    }

    /// Force the subprocess fallback path, bypassing gadugi detection.
    pub fn set_force_subprocess(&mut self, force: bool) {
        self.force_subprocess = force;
    }

    /// Register a [`TUITestCase`].
    pub fn add_test(&mut self, test_case: TUITestCase) {
        self.test_cases.insert(test_case.test_id.clone(), test_case);
    }

    /// Number of registered test cases.
    pub fn test_count(&self) -> usize {
        self.test_cases.len()
    }

    /// Collected results (populated after [`run_test`](Self::run_test) calls).
    pub fn results(&self) -> &HashMap<String, TestResult> {
        &self.results
    }

    /// Output directory path.
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// Run a single test by `test_id`.
    ///
    /// # Errors
    ///
    /// Returns an error when `test_id` is not found in the registered cases.
    pub fn run_test(&mut self, test_id: &str) -> Result<TestResult, String> {
        let test_case = self
            .test_cases
            .get(test_id)
            .ok_or_else(|| format!("Test '{test_id}' not found"))?
            .clone();

        let start = Instant::now();

        let result = if !self.force_subprocess && check_gadugi_available() {
            self.run_with_gadugi(&test_case, start)
        } else {
            self.run_with_subprocess(&test_case, start)
        };

        self.results.insert(test_id.to_string(), result.clone());
        Ok(result)
    }

    /// Run all registered tests sequentially and return the collected results.
    pub fn run_all(&mut self) -> HashMap<String, TestResult> {
        let ids: Vec<String> = self.test_cases.keys().cloned().collect();
        for id in &ids {
            // Errors here only happen for missing IDs which cannot occur.
            let _ = self.run_test(id);
        }
        self.results.clone()
    }

    // -- private helpers ----------------------------------------------------

    fn run_with_gadugi(&self, test_case: &TUITestCase, start: Instant) -> TestResult {
        let config = serde_json::json!({
            "testId": test_case.test_id,
            "name": test_case.name,
            "commands": test_case.commands,
            "timeout": test_case.timeout,
        });

        let config_path = self
            .output_dir
            .join(format!("{}_config.json", test_case.test_id));
        if let Err(e) = std::fs::write(&config_path, config.to_string()) {
            return TestResult::failed(
                &test_case.test_id,
                start.elapsed().as_secs_f64(),
                format!("Failed to write config: {e}"),
            );
        }

        let gadugi_timeout_secs = (test_case.timeout + 10).min(30);

        let outcome = run_command_with_timeout(
            &["npx", "gadugi-test", "run", &config_path.to_string_lossy()],
            Duration::from_secs(gadugi_timeout_secs),
            Some(&self.output_dir),
        );

        // Clean up config file regardless of outcome.
        let _ = std::fs::remove_file(&config_path);

        let duration = start.elapsed().as_secs_f64();

        match outcome {
            CmdOutcome::Success(stdout) => TestResult::passed(
                &test_case.test_id,
                duration,
                format!("gadugi-test completed successfully: {}", stdout.trim()),
            ),
            CmdOutcome::Failed(stderr) => TestResult::failed(
                &test_case.test_id,
                duration,
                format!("gadugi-test failed: {}", stderr.trim()),
            ),
            CmdOutcome::Timeout => TestResult::failed(
                &test_case.test_id,
                duration,
                format!("Test timed out after {gadugi_timeout_secs} seconds"),
            ),
            CmdOutcome::Error(e) => TestResult::failed(
                &test_case.test_id,
                duration,
                format!("gadugi-test error: {e}"),
            ),
        }
    }

    fn run_with_subprocess(&self, test_case: &TUITestCase, start: Instant) -> TestResult {
        let cmd_timeout = Duration::from_secs(test_case.timeout.min(5));

        for command in &test_case.commands {
            let parts: Vec<&str> = command.split_whitespace().collect();
            if parts.is_empty() {
                return TestResult::failed(
                    &test_case.test_id,
                    start.elapsed().as_secs_f64(),
                    format!("Empty command provided: '{command}'"),
                );
            }

            // Verify the command binary exists.
            if !command_exists_on_path(parts[0]) {
                return TestResult::failed(
                    &test_case.test_id,
                    start.elapsed().as_secs_f64(),
                    format!(
                        "Command '{}' not found in PATH. Check with 'which {}'",
                        parts[0], parts[0]
                    ),
                );
            }

            match run_command_with_timeout(&parts, cmd_timeout, None) {
                CmdOutcome::Success(_) => { /* continue to next command */ }
                CmdOutcome::Failed(stderr) => {
                    return TestResult::failed(
                        &test_case.test_id,
                        start.elapsed().as_secs_f64(),
                        format!("Command '{command}' failed: {}", stderr.trim()),
                    );
                }
                CmdOutcome::Timeout => {
                    return TestResult::failed(
                        &test_case.test_id,
                        start.elapsed().as_secs_f64(),
                        format!(
                            "Command '{command}' timed out after {} seconds",
                            cmd_timeout.as_secs()
                        ),
                    );
                }
                CmdOutcome::Error(e) => {
                    return TestResult::failed(
                        &test_case.test_id,
                        start.elapsed().as_secs_f64(),
                        format!("Command '{command}' failed with error: {e}"),
                    );
                }
            }
        }

        TestResult::passed(
            &test_case.test_id,
            start.elapsed().as_secs_f64(),
            format!(
                "Successfully executed {} commands via subprocess",
                test_case.commands.len()
            ),
        )
    }
}

// ---------------------------------------------------------------------------
// Internal command helpers

// Subprocess helpers extracted to simple_tui_runner.rs.
use crate::simple_tui_runner::{CmdOutcome, command_exists_on_path, run_command_with_timeout};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/simple_tui_tests.rs"]
mod tests;
