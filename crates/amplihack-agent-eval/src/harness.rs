//! Test harness for running eval suites.

use crate::error::EvalError;
use crate::models::HarnessConfig;

/// Result of a harness run.
#[derive(Debug, Clone)]
pub struct HarnessResult {
    pub success: bool,
    pub tests_run: usize,
    pub tests_passed: usize,
    pub tests_failed: usize,
    pub duration_seconds: f64,
}

/// Runs evaluation test harnesses.
pub struct HarnessRunner {
    config: HarnessConfig,
}

impl HarnessRunner {
    pub fn new(config: HarnessConfig) -> Self {
        Self { config }
    }

    /// Run the full test harness.
    pub fn run(&self) -> Result<HarnessResult, EvalError> {
        todo!("HarnessRunner::run not yet implemented")
    }

    /// Run with a timeout, returning TimeoutExceeded if exceeded.
    pub fn run_with_timeout(&self, _timeout_seconds: u64) -> Result<HarnessResult, EvalError> {
        todo!("HarnessRunner::run_with_timeout not yet implemented")
    }

    /// Access the config.
    pub fn config(&self) -> &HarnessConfig {
        &self.config
    }
}
