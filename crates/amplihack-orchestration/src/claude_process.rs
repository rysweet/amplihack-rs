//! Claude subprocess execution with output capture and timeout support.
//!
//! Native Rust port of `claude_process.py` (DELEGATE_COMMANDS, build_command,
//! ProcessResult, ProcessRunner trait, TokioProcessRunner, MockProcessRunner,
//! ClaudeProcess + builder).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
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
    cmd.push("-p".to_string());
    cmd.push(prompt.to_string());
    if let Some(m) = model {
        cmd.push("--model".to_string());
        cmd.push(m.to_string());
    }
    cmd
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
}

#[async_trait]
impl ProcessRunner for TokioProcessRunner {
    async fn run(&self, opts: RunOptions) -> ProcessResult {
        let start = Instant::now();
        let delegate = std::env::var("AMPLIHACK_DELEGATE").ok();
        let cmd_parts = build_command(delegate.as_deref(), &opts.prompt, opts.model.as_deref());
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

        let mut command = Command::new(program);
        command.args(args);
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        if let Some(dir) = &opts.working_dir {
            command.current_dir(dir);
        }

        let spawn_result = command.spawn();
        let mut child = match spawn_result {
            Ok(c) => c,
            Err(e) => {
                return ProcessResult::err(
                    format!("Failed to spawn '{program}': {e}"),
                    opts.process_id,
                    start.elapsed(),
                );
            }
        };

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

        let (status, out, err) = match opts.timeout {
            Some(t) => match tokio_timeout(t, read_streams).await {
                Ok(v) => v,
                Err(_) => {
                    return ProcessResult::err(
                        format!("Timed out after {:?}", t),
                        opts.process_id,
                        start.elapsed(),
                    );
                }
            },
            None => read_streams.await,
        };

        let duration = start.elapsed();
        match status {
            Ok(s) => ProcessResult {
                exit_code: s.code().unwrap_or(-1),
                output: out,
                stderr: err,
                duration,
                process_id: opts.process_id,
            },
            Err(e) => ProcessResult::err(format!("wait failed: {e}"), opts.process_id, duration),
        }
    }
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
