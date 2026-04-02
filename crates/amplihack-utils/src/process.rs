//! Cross-platform process management with timeout support.
//!
//! Ported from `amplihack/utils/process.py`. Provides a [`ProcessManager`] for
//! running external commands with optional timeouts, working directory
//! overrides, and environment variable injection, as well as a path-safety
//! helper [`ensure_path_within_root`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

/// Errors produced by process management operations.
#[derive(Debug, Error)]
pub enum ProcessError {
    /// An I/O error occurred when spawning or interacting with the child process.
    #[error("process I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The supplied path escapes the allowed root directory.
    #[error("path {path} escapes root {root}")]
    PathEscape {
        /// The offending path.
        path: String,
        /// The root directory it should stay within.
        root: String,
    },

    /// Path canonicalization failed.
    #[error("failed to canonicalize path {path}: {source}")]
    Canonicalize {
        /// The path that could not be canonicalized.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
}

/// The result of running an external command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandResult {
    /// Process exit code (`None` if the process was killed/signalled).
    pub exit_code: Option<i32>,
    /// Captured standard output (lossy UTF-8).
    pub stdout: String,
    /// Captured standard error (lossy UTF-8).
    pub stderr: String,
    /// Whether the command was terminated because it exceeded the timeout.
    pub timed_out: bool,
}

impl CommandResult {
    /// Returns `true` when the command exited successfully (code 0) without
    /// timing out.
    pub fn success(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

/// Cross-platform process manager with timeout support.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::ProcessManager;
/// use std::time::Duration;
///
/// let mgr = ProcessManager::new();
/// let result = mgr.run_command(&["echo", "hello"], None, None, None)
///     .expect("echo should succeed");
/// assert!(result.success());
/// ```
#[derive(Debug, Default)]
pub struct ProcessManager {
    _private: (),
}

impl ProcessManager {
    /// Create a new `ProcessManager`.
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Run a command synchronously with optional timeout.
    ///
    /// # Arguments
    ///
    /// * `args`    — Command and arguments (first element is the program).
    /// * `timeout` — Maximum wall-clock duration. `None` means no limit.
    /// * `cwd`     — Working directory for the child process.
    /// * `env`     — Extra environment variables merged into the child's env.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::Io`] if the process cannot be spawned.
    pub fn run_command(
        &self,
        args: &[&str],
        timeout: Option<Duration>,
        cwd: Option<&Path>,
        env: Option<&HashMap<String, String>>,
    ) -> Result<CommandResult, ProcessError> {
        if args.is_empty() {
            return Ok(CommandResult {
                exit_code: None,
                stdout: String::new(),
                stderr: "no command provided".into(),
                timed_out: false,
            });
        }

        let mut cmd = std::process::Command::new(args[0]);
        cmd.args(&args[1..]);

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        if let Some(vars) = env {
            for (k, v) in vars {
                cmd.env(k, v);
            }
        }

        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn()?;

        if let Some(dur) = timeout {
            // Poll-based timeout: check in small intervals.
            let deadline = std::time::Instant::now() + dur;
            loop {
                match child.try_wait()? {
                    Some(status) => {
                        let output = collect_output(child)?;
                        return Ok(CommandResult {
                            exit_code: status.code(),
                            stdout: output.0,
                            stderr: output.1,
                            timed_out: false,
                        });
                    }
                    None => {
                        if std::time::Instant::now() >= deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            let output = collect_output(child)?;
                            return Ok(CommandResult {
                                exit_code: None,
                                stdout: output.0,
                                stderr: output.1,
                                timed_out: true,
                            });
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        } else {
            let output = child.wait_with_output()?;
            Ok(CommandResult {
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                timed_out: false,
            })
        }
    }

    /// Convenience wrapper: run a command with an explicit timeout.
    ///
    /// Equivalent to calling [`run_command`](Self::run_command) with
    /// `Some(timeout)` and no extra environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::Io`] if the process cannot be spawned.
    pub fn run_command_with_timeout(
        &self,
        args: &[&str],
        timeout: Duration,
        cwd: Option<&Path>,
    ) -> Result<CommandResult, ProcessError> {
        self.run_command(args, Some(timeout), cwd, None)
    }
}

/// Read remaining stdout/stderr from a child whose status has already been
/// collected via `try_wait`.
fn collect_output(child: std::process::Child) -> Result<(String, String), std::io::Error> {
    let mut stdout_str = String::new();
    let mut stderr_str = String::new();

    if let Some(mut out) = child.stdout {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        stdout_str = String::from_utf8_lossy(&buf).into_owned();
    }
    if let Some(mut err) = child.stderr {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = err.read_to_end(&mut buf);
        stderr_str = String::from_utf8_lossy(&buf).into_owned();
    }

    Ok((stdout_str, stderr_str))
}

/// Validate that `path` does not escape `root` after canonicalization.
///
/// Both `path` and `root` must exist on disk so they can be canonicalized.
/// Returns the canonicalized path on success.
///
/// # Errors
///
/// Returns [`ProcessError::PathEscape`] if the resolved path is not a
/// descendant of `root`, or [`ProcessError::Canonicalize`] if either path
/// cannot be resolved.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::process::ensure_path_within_root;
/// use std::path::Path;
///
/// let safe = ensure_path_within_root(
///     Path::new("/home/user/project/src/lib.rs"),
///     Path::new("/home/user/project"),
/// );
/// assert!(safe.is_ok());
/// ```
pub fn ensure_path_within_root(path: &Path, root: &Path) -> Result<PathBuf, ProcessError> {
    let canonical_root = root
        .canonicalize()
        .map_err(|e| ProcessError::Canonicalize {
            path: root.display().to_string(),
            source: e,
        })?;

    let canonical_path = path
        .canonicalize()
        .map_err(|e| ProcessError::Canonicalize {
            path: path.display().to_string(),
            source: e,
        })?;

    if canonical_path.starts_with(&canonical_root) {
        Ok(canonical_path)
    } else {
        Err(ProcessError::PathEscape {
            path: canonical_path.display().to_string(),
            root: canonical_root.display().to_string(),
        })
    }
}

#[cfg(test)]
#[path = "tests/process_tests.rs"]
mod tests;
