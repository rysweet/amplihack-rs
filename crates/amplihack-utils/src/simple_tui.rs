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
use std::process::Command;
use std::time::{Duration, Instant};

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
        self.test_cases
            .insert(test_case.test_id.clone(), test_case);
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

        let config_path = self.output_dir.join(format!("{}_config.json", test_case.test_id));
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
            CmdOutcome::Success(stdout) => {
                TestResult::passed(&test_case.test_id, duration, format!("gadugi-test completed successfully: {}", stdout.trim()))
            }
            CmdOutcome::Failed(stderr) => {
                TestResult::failed(&test_case.test_id, duration, format!("gadugi-test failed: {}", stderr.trim()))
            }
            CmdOutcome::Timeout => {
                TestResult::failed(&test_case.test_id, duration, format!("Test timed out after {gadugi_timeout_secs} seconds"))
            }
            CmdOutcome::Error(e) => {
                TestResult::failed(&test_case.test_id, duration, format!("gadugi-test error: {e}"))
            }
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
                        format!("Command '{command}' timed out after {} seconds", cmd_timeout.as_secs()),
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
// ---------------------------------------------------------------------------

/// Outcome of a single subprocess invocation.
enum CmdOutcome {
    Success(String),
    Failed(String),
    Timeout,
    Error(String),
}

/// Run a command with a wall-clock timeout.
fn run_command_with_timeout(args: &[&str], timeout: Duration, cwd: Option<&Path>) -> CmdOutcome {
    if args.is_empty() {
        return CmdOutcome::Error("empty argument list".into());
    }

    let mut cmd = Command::new(args[0]);
    cmd.args(&args[1..])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("CI", "true")
        .env("DEBIAN_FRONTEND", "noninteractive");

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return CmdOutcome::Error(e.to_string()),
    };

    // Wait with timeout via a polling loop.
    wait_with_timeout(child, timeout)
}

/// Poll a child process until it exits or `timeout` elapses.
fn wait_with_timeout(mut child: std::process::Child, timeout: Duration) -> CmdOutcome {
    let start = Instant::now();
    let poll_interval = Duration::from_millis(50);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = child
                    .stdout
                    .take()
                    .map(|mut r| {
                        let mut s = String::new();
                        std::io::Read::read_to_string(&mut r, &mut s).ok();
                        s
                    })
                    .unwrap_or_default();
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut r| {
                        let mut s = String::new();
                        std::io::Read::read_to_string(&mut r, &mut s).ok();
                        s
                    })
                    .unwrap_or_default();

                return if status.success() {
                    CmdOutcome::Success(stdout)
                } else {
                    CmdOutcome::Failed(stderr)
                };
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return CmdOutcome::Timeout;
                }
                std::thread::sleep(poll_interval);
            }
            Err(e) => return CmdOutcome::Error(e.to_string()),
        }
    }
}

/// Return `true` if `name` resolves via `which`.
fn command_exists_on_path(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Factory / convenience helpers
// ---------------------------------------------------------------------------

/// Create a [`SimpleTUITester`] with the given (or default) output directory.
///
/// # Errors
///
/// Returns an I/O error if the output directory cannot be created.
pub fn create_tui_tester(output_dir: Option<PathBuf>) -> std::io::Result<SimpleTUITester> {
    SimpleTUITester::new(output_dir.unwrap_or_else(|| PathBuf::from("./tui_output")))
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_result_passed_constructor() {
        let r = TestResult::passed("t1", 1.5, "ok");
        assert!(r.is_passed());
        assert_eq!(r.status, "passed");
        assert_eq!(r.test_id, "t1");
    }

    #[test]
    fn test_result_failed_constructor() {
        let r = TestResult::failed("t2", 0.1, "boom");
        assert!(!r.is_passed());
        assert_eq!(r.status, "failed");
        assert_eq!(r.message, "boom");
    }

    #[test]
    fn test_result_serde_roundtrip() {
        let r = TestResult::passed("ser", 2.0, "ok");
        let json = serde_json::to_string(&r).expect("serialize");
        let r2: TestResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(r, r2);
    }

    #[test]
    fn tui_test_case_default_timeout() {
        let tc = TUITestCase::new("tc", "my test", vec!["echo hi".into()]);
        assert_eq!(tc.timeout, 10);
    }

    #[test]
    fn tui_test_case_custom_timeout() {
        let tc = TUITestCase::with_timeout("tc", "my test", vec!["echo hi".into()], 30);
        assert_eq!(tc.timeout, 30);
    }

    #[test]
    fn tui_test_case_serde_roundtrip() {
        let tc = TUITestCase::new("tc", "test", vec!["ls".into()]);
        let json = serde_json::to_string(&tc).expect("serialize");
        let tc2: TUITestCase = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(tc, tc2);
    }

    #[test]
    fn ci_detection_respects_env() {
        // In test environments CI is typically set, so just verify the function runs.
        let _ = is_ci_environment();
    }

    #[test]
    fn tester_add_and_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        assert_eq!(tester.test_count(), 0);

        tester.add_test(TUITestCase::new("a", "A", vec!["echo a".into()]));
        tester.add_test(TUITestCase::new("b", "B", vec!["echo b".into()]));
        assert_eq!(tester.test_count(), 2);
    }

    #[test]
    fn run_test_unknown_id_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        let res = tester.run_test("nope");
        assert!(res.is_err());
    }

    #[test]
    fn run_test_echo_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        tester.set_force_subprocess(true);
        tester.add_test(TUITestCase::new("echo", "echo test", vec!["echo hello".into()]));

        let res = tester.run_test("echo").expect("run");
        assert!(res.is_passed(), "message: {}", res.message);
    }

    #[test]
    fn run_test_bad_command_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        tester.set_force_subprocess(true);
        tester.add_test(TUITestCase::new(
            "bad",
            "bad cmd",
            vec!["this_command_does_not_exist_xyz".into()],
        ));

        let res = tester.run_test("bad").expect("run");
        assert!(!res.is_passed());
        assert!(res.message.contains("not found"), "msg: {}", res.message);
    }

    #[test]
    fn run_test_empty_command_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        tester.set_force_subprocess(true);
        tester.add_test(TUITestCase::new("empty", "empty", vec!["".into()]));

        let res = tester.run_test("empty").expect("run");
        assert!(!res.is_passed());
        assert!(res.message.contains("Empty command"), "msg: {}", res.message);
    }

    #[test]
    fn run_all_collects_results() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        tester.set_force_subprocess(true);
        tester.add_test(TUITestCase::new("a", "A", vec!["echo a".into()]));
        tester.add_test(TUITestCase::new("b", "B", vec!["echo b".into()]));

        let results = tester.run_all();
        assert_eq!(results.len(), 2);
        assert!(results["a"].is_passed());
        assert!(results["b"].is_passed());
    }

    #[test]
    fn create_amplihack_test_helper() {
        let tc = create_amplihack_test("help", "--help");
        assert_eq!(tc.test_id, "help");
        assert!(tc.commands[0].contains("amplihack --help"));
    }

    #[test]
    fn output_dir_is_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("nested").join("deep");
        let tester = SimpleTUITester::new(&nested).expect("new");
        assert!(tester.output_dir().exists());
    }

    #[test]
    fn command_exists_on_path_echo() {
        assert!(command_exists_on_path("echo"));
    }

    #[test]
    fn command_exists_on_path_missing() {
        assert!(!command_exists_on_path("no_such_binary_abc_xyz_123"));
    }
}
