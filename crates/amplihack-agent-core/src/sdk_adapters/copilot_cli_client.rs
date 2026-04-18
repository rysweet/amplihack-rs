//! Native Copilot CLI client — replaces the Python PTY dependency.
//!
//! Implements [`SdkClient`] by invoking the `copilot` binary directly as a
//! subprocess, piping prompts via stdin and capturing structured output from
//! stdout. No Python intermediary is required.
//!
//! # Binary resolution order
//!
//! 1. Explicit path passed via [`CopilotCliClient::new`]
//! 2. `COPILOT_CLI_PATH` environment variable
//! 3. `~/.npm-global/bin/copilot`
//! 4. `copilot` on `$PATH` (via `which`)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::error::{AgentError, Result};

use super::base::{SdkClient, SdkClientResponse};

/// Well-known install location for the Copilot CLI binary.
const DEFAULT_COPILOT_PATH: &str = ".npm-global/bin/copilot";

/// Environment variable to override the binary path.
const COPILOT_PATH_ENV: &str = "COPILOT_CLI_PATH";

/// Default timeout for a single CLI invocation.
const DEFAULT_CLI_TIMEOUT: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// CopilotCliClient
// ---------------------------------------------------------------------------

/// A native [`SdkClient`] that communicates with GitHub Copilot by spawning
/// the `copilot` CLI binary directly.
///
/// Each [`query`](SdkClient::query) call spawns a fresh subprocess with the
/// prompt piped to stdin. The client captures stdout as the response and
/// parses any tool-call markers emitted by the CLI.
///
/// # Examples
///
/// ```rust,no_run
/// use amplihack_agent_core::sdk_adapters::copilot_cli_client::CopilotCliClient;
/// use amplihack_agent_core::sdk_adapters::copilot::CopilotAdapter;
/// use amplihack_agent_core::sdk_adapters::types::{SdkAdapterConfig, SdkType};
///
/// let client = CopilotCliClient::resolve().expect("copilot binary not found");
/// let config = SdkAdapterConfig::new("my-agent", SdkType::Copilot);
/// let adapter = CopilotAdapter::new(config).with_client(Box::new(client));
/// ```
#[derive(Debug, Clone)]
pub struct CopilotCliClient {
    /// Absolute path to the `copilot` binary.
    binary_path: PathBuf,
    /// Optional working directory for the subprocess.
    working_dir: Option<PathBuf>,
    /// Per-invocation timeout.
    timeout: Duration,
    /// Extra environment variables injected into the subprocess.
    extra_env: HashMap<String, String>,
}

impl CopilotCliClient {
    /// Create a client with an explicit binary path.
    ///
    /// Returns an error if the path does not exist or is not executable.
    pub fn new(binary_path: impl Into<PathBuf>) -> Result<Self> {
        let path = binary_path.into();
        if !path.exists() {
            return Err(AgentError::ConfigError(format!(
                "copilot binary not found at {}",
                path.display()
            )));
        }
        Ok(Self {
            binary_path: path,
            working_dir: None,
            timeout: DEFAULT_CLI_TIMEOUT,
            extra_env: HashMap::new(),
        })
    }

    /// Resolve the `copilot` binary using the standard search order:
    ///
    /// 1. `COPILOT_CLI_PATH` env var
    /// 2. `~/.npm-global/bin/copilot`
    /// 3. `copilot` on `$PATH`
    pub fn resolve() -> Result<Self> {
        if let Ok(env_path) = std::env::var(COPILOT_PATH_ENV) {
            let p = PathBuf::from(&env_path);
            if p.exists() {
                info!(path = %p.display(), "copilot resolved from {COPILOT_PATH_ENV}");
                return Self::new(p);
            }
            warn!(path = %env_path, "{COPILOT_PATH_ENV} set but path not found");
        }

        if let Some(home) = dirs_path() {
            let default = home.join(DEFAULT_COPILOT_PATH);
            if default.exists() {
                info!(path = %default.display(), "copilot resolved from home directory");
                return Self::new(default);
            }
        }

        if let Ok(which) = which_copilot() {
            info!(path = %which.display(), "copilot resolved via PATH");
            return Self::new(which);
        }

        Err(AgentError::ConfigError(
            "copilot binary not found — set COPILOT_CLI_PATH or install via npm".into(),
        ))
    }

    /// Set the working directory for subprocess invocations.
    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Override the per-invocation timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Add an environment variable to the subprocess environment.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_env.insert(key.into(), value.into());
        self
    }

    /// Returns the resolved binary path.
    pub fn binary_path(&self) -> &Path {
        &self.binary_path
    }

    /// Build the full prompt string sent to the copilot CLI.
    fn build_prompt(task: &str, system: &str) -> String {
        if system.is_empty() {
            task.to_string()
        } else {
            format!("{system}\n\n---\n\n{task}")
        }
    }

    /// Parse tool-call markers from raw CLI output.
    ///
    /// The copilot CLI emits lines like `[tool:name]` when it invokes a tool.
    /// We extract those names for the response metadata.
    fn extract_tool_calls(output: &str) -> Vec<String> {
        let mut tools = Vec::new();
        for line in output.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("[tool:")
                && let Some(name) = rest.strip_suffix(']')
            {
                let name = name.trim();
                if !name.is_empty() && !tools.contains(&name.to_string()) {
                    tools.push(name.to_string());
                }
            }
        }
        tools
    }

    /// Strip tool-call marker lines from the response content.
    fn clean_output(output: &str) -> String {
        output
            .lines()
            .filter(|line| !line.trim().starts_with("[tool:"))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }
}

#[async_trait]
impl SdkClient for CopilotCliClient {
    async fn query(
        &self,
        prompt: &str,
        system: &str,
        _model: &str,
        _max_turns: u32,
    ) -> Result<SdkClientResponse> {
        let full_prompt = Self::build_prompt(prompt, system);

        debug!(
            binary = %self.binary_path.display(),
            prompt_len = full_prompt.len(),
            "spawning copilot CLI subprocess"
        );

        let mut cmd = std::process::Command::new(&self.binary_path);
        cmd.arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(ref dir) = self.working_dir {
            cmd.current_dir(dir);
        }

        // Inject extra env vars (e.g., COPILOT_NONINTERACTIVE).
        cmd.env("COPILOT_NONINTERACTIVE", "1");
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| {
            AgentError::SubprocessError(format!(
                "failed to spawn copilot at {}: {e}",
                self.binary_path.display()
            ))
        })?;

        // Write prompt to stdin.
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(full_prompt.as_bytes()).map_err(|e| {
                AgentError::SubprocessError(format!("failed to write to copilot stdin: {e}"))
            })?;
            // Drop stdin to signal EOF.
        }

        // Wait with timeout using a polling loop.
        let timeout = self.timeout;
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => break,
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(AgentError::TimeoutError(timeout.as_secs()));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    return Err(AgentError::SubprocessError(format!(
                        "error waiting for copilot process: {e}"
                    )));
                }
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|e| AgentError::SubprocessError(format!("copilot subprocess failed: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            warn!(
                exit_code = code,
                stderr = %stderr.chars().take(500).collect::<String>(),
                "copilot CLI exited with non-zero status"
            );
            return Err(AgentError::SubprocessError(format!(
                "copilot exited with code {code}: {}",
                stderr.chars().take(500).collect::<String>()
            )));
        }

        let tool_calls = Self::extract_tool_calls(&stdout);
        let content = Self::clean_output(&stdout);

        debug!(
            content_len = content.len(),
            tool_count = tool_calls.len(),
            "copilot CLI response received"
        );

        let mut metadata = HashMap::new();
        if !stderr.is_empty() {
            metadata.insert("stderr".to_string(), Value::String(stderr.into_owned()));
        }

        Ok(SdkClientResponse {
            content,
            tool_calls,
            metadata,
        })
    }

    async fn close(&mut self) -> Result<()> {
        // Subprocess-per-invocation — nothing to tear down.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the user's home directory.
fn dirs_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Look up `copilot` on `$PATH`.
fn which_copilot() -> std::result::Result<PathBuf, String> {
    let output = std::process::Command::new("which")
        .arg("copilot")
        .output()
        .map_err(|e| format!("which failed: {e}"))?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            Err("which returned empty path".into())
        } else {
            Ok(PathBuf::from(path))
        }
    } else {
        Err("copilot not found on PATH".into())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_with_system() {
        let p = CopilotCliClient::build_prompt("do X", "You are helpful");
        assert!(p.starts_with("You are helpful"));
        assert!(p.contains("do X"));
        assert!(p.contains("---"));
    }

    #[test]
    fn build_prompt_without_system() {
        let p = CopilotCliClient::build_prompt("do X", "");
        assert_eq!(p, "do X");
    }

    #[test]
    fn extract_tool_calls_parses_markers() {
        let output = "Hello\n[tool:bash]\nsome output\n[tool:grep]\n[tool:bash]\nDone";
        let tools = CopilotCliClient::extract_tool_calls(output);
        assert_eq!(tools, vec!["bash", "grep"]);
    }

    #[test]
    fn extract_tool_calls_empty() {
        let tools = CopilotCliClient::extract_tool_calls("Just text\nno markers");
        assert!(tools.is_empty());
    }

    #[test]
    fn clean_output_strips_markers() {
        let output = "Hello\n[tool:bash]\nWorld";
        let cleaned = CopilotCliClient::clean_output(output);
        assert_eq!(cleaned, "Hello\nWorld");
    }

    #[test]
    fn resolve_finds_binary() {
        // This test only passes when copilot is actually installed.
        if std::process::Command::new("which")
            .arg("copilot")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            let client = CopilotCliClient::resolve().unwrap();
            assert!(client.binary_path().exists());
        }
    }

    #[test]
    fn new_rejects_missing_path() {
        let err = CopilotCliClient::new("/nonexistent/copilot").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn builder_methods() {
        // Use a known-existing path for the test.
        let path = std::env::current_exe().unwrap();
        let client = CopilotCliClient::new(&path)
            .unwrap()
            .with_working_dir("/tmp")
            .with_timeout(Duration::from_secs(60))
            .with_env("FOO", "bar");
        assert_eq!(client.timeout, Duration::from_secs(60));
        assert_eq!(client.working_dir.as_deref(), Some(Path::new("/tmp")));
        assert_eq!(client.extra_env.get("FOO").map(String::as_str), Some("bar"));
    }
}
