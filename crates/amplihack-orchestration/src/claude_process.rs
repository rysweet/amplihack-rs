//! Claude subprocess execution with output capture and timeout support.
//!
//! Native Rust port of `claude_process.py` (DELEGATE_COMMANDS, build_command,
//! ProcessResult, ProcessRunner trait, TokioProcessRunner, MockProcessRunner,
//! ClaudeProcess + builder).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command as StdCommand;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use amplihack_utils::prompt_delivery::{
    DeliveryCaps, DeliveryHandle, DeliveryMode, PromptDelivery, deliver, from_env, select_mode,
};
use async_trait::async_trait;
use once_cell::sync::Lazy;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout as tokio_timeout;

/// Pre-split command lookup: maps `AMPLIHACK_DELEGATE` value to its binary
/// command prefix (vectors, never shell strings — prevents injection).
pub static DELEGATE_COMMANDS: Lazy<HashMap<String, Vec<String>>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("amplihack claude".to_string(), vec!["claude".to_string()]);
    m.insert(
        "amplihack copilot".to_string(),
        vec!["amplihack".to_string(), "copilot".to_string()],
    );
    m.insert(
        "amplihack amplifier".to_string(),
        vec!["amplihack".to_string(), "amplifier".to_string()],
    );
    m
});

/// Build the agent CLI command (mirrors Python `_build_command`).
/// `delegate=None` or unrecognised → `["claude"]` with `tracing::warn!`.
/// Appends `--dangerously-skip-permissions -p <prompt>` and optional
/// `--model <model>`.
pub fn build_command(delegate: Option<&str>, prompt: &str, model: Option<&str>) -> Vec<String> {
    let mut cmd = build_command_prefix(delegate, model);
    cmd.push(prompt.to_string());
    cmd
}

fn build_command_prefix(delegate: Option<&str>, model: Option<&str>) -> Vec<String> {
    let prefix: Vec<String> = match delegate {
        None => {
            tracing::warn!(
                "AMPLIHACK_DELEGATE not set — defaulting to 'claude'. \
                 Set AMPLIHACK_DELEGATE (e.g. 'amplihack claude') to select the agent."
            );
            vec!["claude".to_string()]
        }
        Some(d) => match DELEGATE_COMMANDS.get(d) {
            Some(v) => v.clone(),
            None => {
                tracing::warn!(
                    delegate = d,
                    "Unrecognised AMPLIHACK_DELEGATE; falling back to 'claude'."
                );
                vec!["claude".to_string()]
            }
        },
    };

    let mut cmd = prefix;
    cmd.push("--dangerously-skip-permissions".to_string());
    if let Some(m) = model {
        cmd.push("--model".to_string());
        cmd.push(m.to_string());
    }
    cmd.push("-p".to_string());
    cmd
}

#[derive(Debug)]
pub struct DeliveredProcessCommand {
    pub command: StdCommand,
    pub delivery_handle: DeliveryHandle,
    pub selected_mode: DeliveryMode,
    pub stdin_payload: Option<Vec<u8>>,
}

pub fn build_command_with_prompt_delivery<I, S>(
    program: &str,
    args: I,
    prompt: &str,
    requested: PromptDelivery,
    caps: DeliveryCaps,
) -> std::io::Result<DeliveredProcessCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = StdCommand::new(program);
    command.args(args);
    let selected_mode = select_mode(requested, prompt.len(), &caps);
    let delivery_handle = deliver(&mut command, prompt, requested, &caps)?;
    let stdin_payload = (selected_mode == DeliveryMode::Stdin).then(|| prompt.as_bytes().to_vec());
    Ok(DeliveredProcessCommand {
        command,
        delivery_handle,
        selected_mode,
        stdin_payload,
    })
}

/// Result of a single process execution.
#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub exit_code: i32,
    pub output: String,
    pub stderr: String,
    pub duration: Duration,
    pub process_id: String,
}

impl ProcessResult {
    /// Construct a successful result (`exit_code = 0`).
    pub fn ok(output: String, process_id: String, duration: Duration) -> Self {
        Self {
            exit_code: 0,
            output,
            stderr: String::new(),
            duration,
            process_id,
        }
    }

    /// Construct a failure sentinel (`exit_code = -1`) — used for timeouts
    /// and fatal errors before the subprocess even runs to completion.
    pub fn err(stderr: String, process_id: String, duration: Duration) -> Self {
        Self {
            exit_code: -1,
            output: String::new(),
            stderr,
            duration,
            process_id,
        }
    }

    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Options for a single `ProcessRunner::run` invocation. `prompt` and
/// `process_id` are required; everything else is optional.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub prompt: String,
    pub process_id: String,
    pub timeout: Option<Duration>,
    pub model: Option<String>,
    pub working_dir: Option<PathBuf>,
}

impl RunOptions {
    pub fn new(prompt: String, process_id: String) -> Self {
        Self {
            prompt,
            process_id,
            timeout: None,
            model: None,
            working_dir: None,
        }
    }
}

/// Async abstraction over command execution. Object-safe via
/// `#[async_trait]`. Implementors must be `Send + Sync + 'static` to support
/// long-lived `tokio::task::JoinSet` scopes.
#[async_trait]
pub trait ProcessRunner: Send + Sync + 'static {
    async fn run(&self, opts: RunOptions) -> ProcessResult;
}

/// Production runner that spawns the agent CLI via `tokio::process`.
#[derive(Default, Debug, Clone)]
pub struct TokioProcessRunner;

impl TokioProcessRunner {
    pub fn new() -> Self {
        Self
    }

    pub async fn run_with_prompt_delivery_for_test<I, S>(
        &self,
        opts: RunOptions,
        requested: PromptDelivery,
        caps: DeliveryCaps,
        program: &str,
        args: I,
    ) -> ProcessResult
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let start = Instant::now();
        let mut delivered = match build_command_with_prompt_delivery(
            program,
            args,
            &opts.prompt,
            requested,
            caps,
        ) {
            Ok(command) => command,
            Err(e) => {
                return ProcessResult::err(
                    format!("prompt delivery failed: {e}"),
                    opts.process_id,
                    start.elapsed(),
                );
            }
        };
        if let Some(dir) = &opts.working_dir {
            delivered.command.current_dir(dir);
        }
        run_delivered_command(delivered, opts.timeout, opts.process_id, start).await
    }
}

#[async_trait]
impl ProcessRunner for TokioProcessRunner {
    async fn run(&self, opts: RunOptions) -> ProcessResult {
        let start = Instant::now();
        let delegate = std::env::var("AMPLIHACK_DELEGATE").ok();
        let cmd_parts = build_command_prefix(delegate.as_deref(), opts.model.as_deref());
        let (program, args) = match cmd_parts.split_first() {
            Some(parts) => parts,
            None => {
                return ProcessResult::err(
                    "build_command returned an empty argv".to_string(),
                    opts.process_id,
                    start.elapsed(),
                );
            }
        };

        let mut delivered = match build_command_with_prompt_delivery(
            program,
            args,
            &opts.prompt,
            from_env(),
            delivery_caps_for_delegate(delegate.as_deref()),
        ) {
            Ok(command) => command,
            Err(e) => {
                return ProcessResult::err(
                    format!("prompt delivery failed: {e}"),
                    opts.process_id,
                    start.elapsed(),
                );
            }
        };
        if let Some(dir) = &opts.working_dir {
            delivered.command.current_dir(dir);
        }

        run_delivered_command(delivered, opts.timeout, opts.process_id, start).await
    }
}

async fn run_delivered_command(
    delivered: DeliveredProcessCommand,
    timeout: Option<Duration>,
    process_id: String,
    start: Instant,
) -> ProcessResult {
    let DeliveredProcessCommand {
        mut command,
        delivery_handle,
        stdin_payload,
        ..
    } = delivered;
    if stdin_payload.is_none() {
        command.stdin(std::process::Stdio::null());
    }
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let program = command.get_program().to_string_lossy().into_owned();
    let mut command = TokioCommand::from(command);
    command.kill_on_drop(true);
    let spawn_result = command.spawn();
    let mut child = match spawn_result {
        Ok(c) => c,
        Err(e) => {
            return ProcessResult::err(
                format!("Failed to spawn '{program}': {e}"),
                process_id,
                start.elapsed(),
            );
        }
    };

    if let Some(payload) = stdin_payload
        && let Some(mut stdin) = child.stdin.take()
    {
        if let Err(e) = stdin.write_all(&payload).await {
            return ProcessResult::err(
                format!("failed to write prompt to child stdin: {e}"),
                process_id,
                start.elapsed(),
            );
        }
        if let Err(e) = stdin.flush().await {
            return ProcessResult::err(
                format!("failed to flush child stdin: {e}"),
                process_id,
                start.elapsed(),
            );
        }
    }

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let read_streams = async {
        let mut out = String::new();
        let mut err = String::new();
        if let Some(p) = stdout_pipe.as_mut() {
            let _ = p.read_to_string(&mut out).await;
        }
        if let Some(p) = stderr_pipe.as_mut() {
            let _ = p.read_to_string(&mut err).await;
        }
        let status = child.wait().await;
        (status, out, err)
    };

    let (status, out, err) = match timeout {
        Some(t) => match tokio_timeout(t, read_streams).await {
            Ok(v) => v,
            Err(_) => {
                return ProcessResult::err(
                    format!("Timed out after {t:?}"),
                    process_id,
                    start.elapsed(),
                );
            }
        },
        None => read_streams.await,
    };

    drop(delivery_handle);
    let duration = start.elapsed();
    match status {
        Ok(s) => ProcessResult {
            exit_code: s.code().unwrap_or(-1),
            output: out,
            stderr: err,
            duration,
            process_id,
        },
        Err(e) => ProcessResult::err(format!("wait failed: {e}"), process_id, duration),
    }
}

fn delivery_caps_for_delegate(_delegate: Option<&str>) -> DeliveryCaps {
    DeliveryCaps::argv_only()
}

pub async fn run_delivered_command_for_test(
    delivered: DeliveredProcessCommand,
    timeout: Duration,
) -> std::io::Result<ProcessResult> {
    Ok(run_delivered_command(
        delivered,
        Some(timeout),
        "prompt-delivery-test".to_string(),
        Instant::now(),
    )
    .await)
}

/// Internal mock expectation entry.
#[derive(Debug, Clone)]
enum MockExpect {
    Exact {
        prompt: String,
        result: ProcessResult,
    },
    Substring {
        needle: String,
        result: ProcessResult,
    },
    Any {
        result: ProcessResult,
    },
}

/// Test double for `ProcessRunner`. Records every invocation and returns
/// canned responses; matching iterates in **reverse insertion order** so
/// later/more-specific expectations override earlier ones (incl. `expect_any`).
#[derive(Default, Debug)]
pub struct MockProcessRunner {
    state: Mutex<MockState>,
}

#[derive(Default, Debug)]
struct MockState {
    expectations: Vec<MockExpect>,
    calls: Vec<RunOptions>,
}

impl MockProcessRunner {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, e: MockExpect) {
        self.state.lock().unwrap().expectations.push(e);
    }

    /// Match `prompt` exactly.
    pub fn expect(&self, prompt: &str, result: ProcessResult) {
        self.push(MockExpect::Exact {
            prompt: prompt.to_string(),
            result,
        });
    }

    /// Match any prompt that contains `needle`.
    pub fn expect_substring(&self, needle: &str, result: ProcessResult) {
        self.push(MockExpect::Substring {
            needle: needle.to_string(),
            result,
        });
    }

    /// Match any prompt (catch-all).
    pub fn expect_any(&self, result: ProcessResult) {
        self.push(MockExpect::Any { result });
    }

    /// Snapshot of recorded invocations in call order.
    pub fn calls(&self) -> Vec<RunOptions> {
        self.state.lock().unwrap().calls.clone()
    }
}

#[async_trait]
impl ProcessRunner for MockProcessRunner {
    async fn run(&self, opts: RunOptions) -> ProcessResult {
        let mut state = self.state.lock().unwrap();
        state.calls.push(opts.clone());

        // Reverse iteration: last-added expectation wins, so callers can
        // layer specific matchers on top of `expect_any` defaults.
        for exp in state.expectations.iter().rev() {
            match exp {
                MockExpect::Exact { prompt, result } if *prompt == opts.prompt => {
                    return result.clone();
                }
                MockExpect::Substring { needle, result } if opts.prompt.contains(needle) => {
                    return result.clone();
                }
                MockExpect::Any { result } => {
                    return result.clone();
                }
                _ => continue,
            }
        }

        ProcessResult::err(
            format!(
                "MockProcessRunner: no expectation matched prompt {:?}",
                opts.prompt
            ),
            opts.process_id,
            Duration::ZERO,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_command ──────────────────────────────────────────────

    #[test]
    fn build_command_no_delegate_defaults_to_claude() {
        let cmd = build_command(None, "do stuff", None);
        assert_eq!(cmd[0], "claude");
        assert!(cmd.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(cmd.contains(&"-p".to_string()));
        assert!(cmd.contains(&"do stuff".to_string()));
    }

    #[test]
    fn build_command_with_known_delegate() {
        let cmd = build_command(Some("amplihack copilot"), "hello", None);
        assert_eq!(cmd[0], "amplihack");
        assert_eq!(cmd[1], "copilot");
        assert!(cmd.contains(&"hello".to_string()));
    }

    #[test]
    fn build_command_with_unknown_delegate_falls_back() {
        let cmd = build_command(Some("unknown-agent"), "test", None);
        assert_eq!(cmd[0], "claude");
    }

    #[test]
    fn build_command_with_model() {
        let cmd = build_command(None, "prompt", Some("opus-4"));
        assert!(cmd.contains(&"--model".to_string()));
        assert!(cmd.contains(&"opus-4".to_string()));
    }

    #[test]
    fn build_command_without_model() {
        let cmd = build_command(None, "prompt", None);
        assert!(!cmd.contains(&"--model".to_string()));
    }

    #[test]
    fn build_command_amplihack_claude_delegate() {
        let cmd = build_command(Some("amplihack claude"), "x", None);
        assert_eq!(cmd[0], "claude");
    }

    #[test]
    fn build_command_amplihack_amplifier_delegate() {
        let cmd = build_command(Some("amplihack amplifier"), "x", None);
        assert_eq!(cmd[0], "amplihack");
        assert_eq!(cmd[1], "amplifier");
    }

    // ── DELEGATE_COMMANDS ──────────────────────────────────────────

    #[test]
    fn delegate_commands_has_three_entries() {
        assert_eq!(DELEGATE_COMMANDS.len(), 3);
        assert!(DELEGATE_COMMANDS.contains_key("amplihack claude"));
        assert!(DELEGATE_COMMANDS.contains_key("amplihack copilot"));
        assert!(DELEGATE_COMMANDS.contains_key("amplihack amplifier"));
    }

    // ── ProcessResult ──────────────────────────────────────────────

    #[test]
    fn process_result_ok_is_success() {
        let r = ProcessResult::ok("output".into(), "pid-1".into(), Duration::from_secs(1));
        assert!(r.is_success());
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.output, "output");
        assert!(r.stderr.is_empty());
    }

    #[test]
    fn process_result_err_is_not_success() {
        let r = ProcessResult::err("boom".into(), "pid-2".into(), Duration::from_secs(2));
        assert!(!r.is_success());
        assert_eq!(r.exit_code, -1);
        assert_eq!(r.stderr, "boom");
        assert!(r.output.is_empty());
    }

    #[test]
    fn process_result_custom_exit_code() {
        let r = ProcessResult {
            exit_code: 42,
            output: String::new(),
            stderr: String::new(),
            duration: Duration::ZERO,
            process_id: "x".into(),
        };
        assert!(!r.is_success());
    }

    // ── RunOptions ─────────────────────────────────────────────────

    #[test]
    fn run_options_new_defaults() {
        let opts = RunOptions::new("prompt".into(), "id".into());
        assert_eq!(opts.prompt, "prompt");
        assert_eq!(opts.process_id, "id");
        assert!(opts.timeout.is_none());
        assert!(opts.model.is_none());
        assert!(opts.working_dir.is_none());
    }

    // ── MockProcessRunner ──────────────────────────────────────────

    #[tokio::test]
    async fn mock_runner_exact_match() {
        let mock = MockProcessRunner::new();
        mock.expect(
            "hello",
            ProcessResult::ok("world".into(), "1".into(), Duration::ZERO),
        );
        let r = mock.run(RunOptions::new("hello".into(), "1".into())).await;
        assert!(r.is_success());
        assert_eq!(r.output, "world");
    }

    #[tokio::test]
    async fn mock_runner_substring_match() {
        let mock = MockProcessRunner::new();
        mock.expect_substring(
            "needle",
            ProcessResult::ok("found".into(), "2".into(), Duration::ZERO),
        );
        let r = mock
            .run(RunOptions::new("hayneedlestack".into(), "2".into()))
            .await;
        assert!(r.is_success());
        assert_eq!(r.output, "found");
    }

    #[tokio::test]
    async fn mock_runner_any_match() {
        let mock = MockProcessRunner::new();
        mock.expect_any(ProcessResult::ok("any".into(), "3".into(), Duration::ZERO));
        let r = mock
            .run(RunOptions::new("anything".into(), "3".into()))
            .await;
        assert!(r.is_success());
        assert_eq!(r.output, "any");
    }

    #[tokio::test]
    async fn mock_runner_no_match_returns_error() {
        let mock = MockProcessRunner::new();
        let r = mock
            .run(RunOptions::new("nomatch".into(), "4".into()))
            .await;
        assert!(!r.is_success());
        assert!(r.stderr.contains("no expectation matched"));
    }

    #[tokio::test]
    async fn mock_runner_later_expectation_wins() {
        let mock = MockProcessRunner::new();
        mock.expect_any(ProcessResult::ok(
            "early".into(),
            "5".into(),
            Duration::ZERO,
        ));
        mock.expect(
            "specific",
            ProcessResult::ok("late".into(), "5".into(), Duration::ZERO),
        );
        let r = mock
            .run(RunOptions::new("specific".into(), "5".into()))
            .await;
        assert_eq!(r.output, "late");
    }

    #[tokio::test]
    async fn mock_runner_records_calls() {
        let mock = MockProcessRunner::new();
        mock.expect_any(ProcessResult::ok(
            String::new(),
            String::new(),
            Duration::ZERO,
        ));
        mock.run(RunOptions::new("a".into(), "1".into())).await;
        mock.run(RunOptions::new("b".into(), "2".into())).await;
        let calls = mock.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].prompt, "a");
        assert_eq!(calls[1].prompt, "b");
    }

    // ── BuildError ─────────────────────────────────────────────────

    #[test]
    fn build_error_display() {
        let e = BuildError::MissingField("prompt");
        assert!(e.to_string().contains("prompt"));
    }

    // ── ClaudeProcess ──────────────────────────────────────────────

    #[tokio::test]
    async fn claude_process_run_delegates_to_runner() {
        let mock = Arc::new(MockProcessRunner::new());
        mock.expect_any(ProcessResult::ok(
            "done".into(),
            "cp-1".into(),
            Duration::from_millis(50),
        ));
        let tmp = tempfile::tempdir().unwrap();
        let cp = ClaudeProcess::__from_builder_parts(
            "test prompt".into(),
            "cp-1".into(),
            tmp.path().to_path_buf(),
            tmp.path().join("logs"),
            None,
            None,
            mock.clone(),
        );
        assert_eq!(cp.process_id(), "cp-1");
        assert_eq!(cp.prompt(), "test prompt");
        let result = cp.run().await;
        assert!(result.is_success());
        assert_eq!(mock.calls().len(), 1);
    }

    #[tokio::test]
    async fn claude_process_log_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockProcessRunner::new());
        let cp = ClaudeProcess::__from_builder_parts(
            "p".into(),
            "log-test".into(),
            tmp.path().to_path_buf(),
            tmp.path().join("logs"),
            None,
            None,
            mock,
        );
        cp.log("test message", "DEBUG");
        let log_path = tmp.path().join("logs").join("log-test.log");
        assert!(log_path.exists());
        let content = std::fs::read_to_string(log_path).unwrap();
        assert!(content.contains("test message"));
        assert!(content.contains("DEBUG"));
    }

    #[test]
    fn claude_process_set_prompt() {
        let mock = Arc::new(MockProcessRunner::new());
        let tmp = tempfile::tempdir().unwrap();
        let mut cp = ClaudeProcess::__from_builder_parts(
            "old".into(),
            "x".into(),
            tmp.path().to_path_buf(),
            tmp.path().join("logs"),
            None,
            None,
            mock,
        );
        cp.set_prompt("new".into());
        assert_eq!(cp.prompt(), "new");
    }

    #[test]
    fn claude_process_set_timeout() {
        let mock = Arc::new(MockProcessRunner::new());
        let tmp = tempfile::tempdir().unwrap();
        let mut cp = ClaudeProcess::__from_builder_parts(
            "p".into(),
            "x".into(),
            tmp.path().to_path_buf(),
            tmp.path().join("logs"),
            None,
            None,
            mock,
        );
        cp.set_timeout(Some(Duration::from_secs(60)));
        // No public getter for timeout, but set shouldn't panic
        cp.set_timeout(None);
    }
}

/// Builder error for `ClaudeProcess`.
#[derive(Debug, Error)]
pub enum BuildError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
}

/// High-level wrapper around a single agent CLI execution. Mirrors the
/// Python `ClaudeProcess` class but delegates subprocess work to a
/// `ProcessRunner` for testability.
pub struct ClaudeProcess {
    prompt: String,
    process_id: String,
    working_dir: PathBuf,
    log_dir: PathBuf,
    model: Option<String>,
    timeout: Option<Duration>,
    runner: Arc<dyn ProcessRunner>,
}

impl ClaudeProcess {
    pub fn builder() -> crate::claude_process_builder::ClaudeProcessBuilder {
        crate::claude_process_builder::ClaudeProcessBuilder::default()
    }

    /// Internal constructor used by [`ClaudeProcessBuilder::build`].
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn __from_builder_parts(
        prompt: String,
        process_id: String,
        working_dir: PathBuf,
        log_dir: PathBuf,
        model: Option<String>,
        timeout: Option<Duration>,
        runner: Arc<dyn ProcessRunner>,
    ) -> Self {
        Self {
            prompt,
            process_id,
            working_dir,
            log_dir,
            model,
            timeout,
            runner,
        }
    }

    pub fn process_id(&self) -> &str {
        &self.process_id
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn set_prompt(&mut self, prompt: String) {
        self.prompt = prompt;
    }

    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    pub fn log_dir(&self) -> &std::path::Path {
        &self.log_dir
    }

    /// Append a per-process log line to `<log_dir>/<process_id>.log`.
    pub fn log(&self, msg: &str, level: &str) {
        let _ = std::fs::create_dir_all(&self.log_dir);
        let line = format!(
            "[{}] [{}] [{}] {}\n",
            crate::time_utils::current_hms(),
            level,
            self.process_id,
            msg
        );
        let path = self.log_dir.join(format!("{}.log", self.process_id));
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(line.as_bytes());
        }
        tracing::info!(target: "amplihack_orchestration", process_id = %self.process_id, level, "{msg}");
    }

    /// Execute the underlying process via the configured runner.
    pub async fn run(&self) -> ProcessResult {
        self.log(
            &format!("Starting process with timeout={:?}", self.timeout),
            "INFO",
        );
        let opts = RunOptions {
            prompt: self.prompt.clone(),
            process_id: self.process_id.clone(),
            timeout: self.timeout,
            model: self.model.clone(),
            working_dir: Some(self.working_dir.clone()),
        };
        let result = self.runner.run(opts).await;
        self.log(
            &format!(
                "Completed with exit_code={} in {:.1}s",
                result.exit_code,
                result.duration.as_secs_f32()
            ),
            "INFO",
        );
        result
    }
}
