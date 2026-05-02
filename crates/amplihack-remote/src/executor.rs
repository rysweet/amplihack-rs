//! Remote command execution via azlin.
//!
//! Handles SCP file transfer, SSH remote command execution, and
//! result retrieval from Azure VMs.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::error::{ErrorContext, RemoteError};
use crate::orchestrator::VM;

/// Result of a remote command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_seconds: f64,
    pub timed_out: bool,
}

/// Executes amplihack commands on remote VMs.
pub struct Executor {
    vm: VM,
    timeout_seconds: u64,
    remote_workspace: String,
    tunnel_port: Option<u16>,
}

impl Executor {
    pub fn new(vm: VM, timeout_minutes: u64, tunnel_port: Option<u16>) -> Self {
        Self {
            vm,
            timeout_seconds: timeout_minutes * 60,
            remote_workspace: "~/workspace".to_string(),
            tunnel_port,
        }
    }

    /// Transfer context archive to remote VM via `azlin cp`.
    pub async fn transfer_context(&self, archive_path: &Path) -> Result<(), RemoteError> {
        if !archive_path.exists() {
            return Err(RemoteError::transfer_ctx(
                format!("Archive file not found: {}", archive_path.display()),
                ErrorContext::new().insert("archive_path", archive_path.display().to_string()),
            ));
        }

        let size_mb = std::fs::metadata(archive_path)
            .map(|m| m.len() as f64 / 1024.0 / 1024.0)
            .unwrap_or(0.0);
        info!(size_mb = format!("{size_mb:.1}"), "transferring context");

        let archive_dir = archive_path.parent().unwrap_or_else(|| Path::new("."));
        let archive_name = archive_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("context.tar.gz");
        let remote_path = format!("{}:~/context.tar.gz", self.vm.name);

        let max_retries = 2u32;
        for attempt in 0..max_retries {
            let mut cmd = Command::new("azlin");
            cmd.arg("cp");
            self.append_port_args(&mut cmd);
            cmd.args([archive_name, &remote_path]);
            cmd.current_dir(archive_dir);
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());

            let start = Instant::now();
            match tokio::time::timeout(std::time::Duration::from_secs(600), cmd.output()).await {
                Ok(Ok(output)) if output.status.success() => {
                    let dur = start.elapsed().as_secs_f64();
                    info!(duration_secs = format!("{dur:.1}"), "transfer complete");
                    return Ok(());
                }
                Ok(Ok(output)) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if attempt < max_retries - 1 {
                        warn!(attempt = attempt + 1, "transfer failed, retrying");
                        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                        continue;
                    }
                    return Err(RemoteError::transfer_ctx(
                        format!("Failed to transfer file: {stderr}"),
                        ErrorContext::new().insert("vm_name", &self.vm.name),
                    ));
                }
                Ok(Err(e)) => {
                    if attempt < max_retries - 1 {
                        warn!(
                            error = %e,
                            "transfer error, retrying"
                        );
                        continue;
                    }
                    return Err(RemoteError::transfer(format!(
                        "Transfer command failed: {e}"
                    )));
                }
                Err(_) => {
                    if attempt < max_retries - 1 {
                        warn!("transfer timeout, retrying");
                        continue;
                    }
                    return Err(RemoteError::transfer_ctx(
                        format!(
                            "Transfer timed out after \
                             {max_retries} attempts"
                        ),
                        ErrorContext::new().insert("vm_name", &self.vm.name),
                    ));
                }
            }
        }

        Err(RemoteError::transfer("Transfer failed after all retries"))
    }

    /// Execute amplihack command on the remote VM.
    pub async fn execute_remote(
        &self,
        command: &str,
        prompt: &str,
        max_turns: u32,
    ) -> Result<ExecutionResult, RemoteError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| RemoteError::execution("ANTHROPIC_API_KEY not found in environment"))?;
        self.execute_remote_with_api_key(command, prompt, max_turns, &api_key)
            .await
    }

    /// Execute amplihack command on the remote VM with an explicit API key.
    pub async fn execute_remote_with_api_key(
        &self,
        command: &str,
        prompt: &str,
        max_turns: u32,
        api_key: &str,
    ) -> Result<ExecutionResult, RemoteError> {
        if api_key.trim().is_empty() {
            return Err(RemoteError::execution(
                "ANTHROPIC_API_KEY not found in environment",
            ));
        }
        let encoded_prompt = b64_encode(prompt.as_bytes());
        let encoded_key = b64_encode(api_key.as_bytes());

        info!(command, "executing remote command");

        let setup_script = format!(
            r#"
set -e
cd ~
tar xzf context.tar.gz
rm -rf {workspace}
mkdir -p {workspace}
cd {workspace}
git clone ~/repo.bundle .
rm -rf .claude && cp -r ~/.claude .
export ANTHROPIC_API_KEY=$(echo '{key}' | base64 -d)
PROMPT=$(echo '{prompt}' | base64 -d)
amplihack claude --{command} --max-turns {turns} -- -p "$PROMPT"
"#,
            workspace = self.remote_workspace,
            key = encoded_key,
            prompt = encoded_prompt,
            command = command,
            turns = max_turns,
        );

        let mut cmd = Command::new("azlin");
        cmd.arg("connect");
        self.append_port_args(&mut cmd);
        cmd.args([&self.vm.name, &setup_script]);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let start = Instant::now();

        match tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_seconds),
            cmd.output(),
        )
        .await
        {
            Ok(Ok(output)) => {
                let duration = start.elapsed().as_secs_f64();
                Ok(ExecutionResult {
                    exit_code: output.status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    duration_seconds: duration,
                    timed_out: false,
                })
            }
            Ok(Err(e)) => Err(RemoteError::execution(format!(
                "Remote command failed: {e}"
            ))),
            Err(_) => {
                let duration = start.elapsed().as_secs_f64();
                warn!(
                    duration_secs = format!("{duration:.1}"),
                    "execution timed out"
                );
                Ok(ExecutionResult {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!(
                        "Execution timed out after {:.1} minutes",
                        self.timeout_seconds as f64 / 60.0
                    ),
                    duration_seconds: duration,
                    timed_out: true,
                })
            }
        }
    }

    /// Launch an amplihack command inside a detached tmux session on the VM.
    pub async fn execute_remote_tmux(
        &self,
        session_id: &str,
        command: &str,
        prompt: &str,
        max_turns: u32,
        api_key: &str,
    ) -> Result<(), RemoteError> {
        if api_key.trim().is_empty() {
            return Err(RemoteError::execution(
                "ANTHROPIC_API_KEY not found in environment",
            ));
        }

        let encoded_prompt = b64_encode(prompt.as_bytes());
        let encoded_key = b64_encode(api_key.as_bytes());
        let script = format!(
            r#"
set -e
export ANTHROPIC_API_KEY=$(echo '{key}' | base64 -d)
PROMPT=$(echo '{prompt}' | base64 -d)
tmux new-session -d -s {session} "cd ~/workspace && amplihack claude --{command} --max-turns {turns} -- -p \"$PROMPT\""
"#,
            key = encoded_key,
            prompt = encoded_prompt,
            session = shell_escape(session_id),
            command = command,
            turns = max_turns,
        );

        let mut cmd = Command::new("azlin");
        cmd.arg("connect");
        self.append_port_args(&mut cmd);
        cmd.args([&self.vm.name, &script]);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output = tokio::time::timeout(std::time::Duration::from_secs(60), cmd.output())
            .await
            .map_err(|_| RemoteError::execution("tmux launch timed out"))?
            .map_err(|e| RemoteError::execution(format!("tmux launch failed: {e}")))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(RemoteError::execution(format!(
                "tmux launch failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )))
        }
    }

    /// Retrieve execution logs from remote VM.
    pub async fn retrieve_logs(&self, local_dest: &Path) -> Result<(), RemoteError> {
        tokio::fs::create_dir_all(local_dest)
            .await
            .map_err(|e| RemoteError::transfer(format!("Failed to create dest dir: {e}")))?;

        let archive_script = format!(
            r#"
if [ -d {ws}/.claude/runtime/logs ]; then
    cd {ws} && tar czf ~/logs.tar.gz .claude/runtime/logs/
else
    echo "No logs directory found" && exit 1
fi
"#,
            ws = self.remote_workspace,
        );

        self.run_remote_command(&archive_script).await?;

        let local_archive = local_dest.join("logs.tar.gz");
        self.download_file("~/logs.tar.gz", &local_archive).await?;

        // Extract locally
        let status = Command::new("tar")
            .args(["xzf", local_archive.to_str().unwrap_or("logs.tar.gz")])
            .current_dir(local_dest)
            .status()
            .await
            .map_err(|e| RemoteError::transfer(format!("Failed to extract logs: {e}")))?;

        if !status.success() {
            return Err(RemoteError::transfer("Log extraction failed"));
        }

        let _ = tokio::fs::remove_file(&local_archive).await;
        debug!(dest = %local_dest.display(), "logs retrieved");
        Ok(())
    }

    /// Retrieve git state as a bundle from remote VM.
    pub async fn retrieve_git_state(&self, local_dest: &Path) -> Result<PathBuf, RemoteError> {
        tokio::fs::create_dir_all(local_dest)
            .await
            .map_err(|e| RemoteError::transfer(format!("Failed to create dest dir: {e}")))?;

        let bundle_script = format!(
            "cd {ws} && git bundle create ~/results.bundle --all",
            ws = self.remote_workspace,
        );

        self.run_remote_command(&bundle_script).await?;

        let local_bundle = local_dest.join("results.bundle");
        self.download_file("~/results.bundle", &local_bundle)
            .await?;

        info!(
            path = %local_bundle.display(),
            "git state retrieved"
        );
        Ok(local_bundle)
    }

    // ---- helpers ----

    async fn run_remote_command(&self, script: &str) -> Result<(), RemoteError> {
        let mut cmd = Command::new("azlin");
        cmd.arg("connect");
        self.append_port_args(&mut cmd);
        cmd.args([&self.vm.name, script]);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output = tokio::time::timeout(std::time::Duration::from_secs(300), cmd.output())
            .await
            .map_err(|_| RemoteError::transfer("Remote command timed out"))?
            .map_err(|e| RemoteError::transfer(format!("Remote command failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RemoteError::transfer_ctx(
                format!("Remote command failed: {stderr}"),
                ErrorContext::new().insert("vm_name", &self.vm.name),
            ));
        }
        Ok(())
    }

    async fn download_file(&self, remote_path: &str, local_path: &Path) -> Result<(), RemoteError> {
        let local_dir = local_path.parent().unwrap_or_else(|| Path::new("."));
        let local_name = local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");
        let remote = format!("{}:{remote_path}", self.vm.name);

        let mut cmd = Command::new("azlin");
        cmd.arg("cp");
        self.append_port_args(&mut cmd);
        cmd.args([&remote, local_name]);
        cmd.current_dir(local_dir);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output = tokio::time::timeout(std::time::Duration::from_secs(300), cmd.output())
            .await
            .map_err(|_| RemoteError::transfer("Download timed out"))?
            .map_err(|e| RemoteError::transfer(format!("Download failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RemoteError::transfer(format!("Download failed: {stderr}")));
        }
        Ok(())
    }

    fn append_port_args(&self, cmd: &mut Command) {
        if let Some(port) = self.tunnel_port {
            cmd.args(["--port", &port.to_string()]);
        }
    }
}

/// Simple base64 encoder (standard alphabet, with padding).
fn b64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        result.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(ALPHABET[(n & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn shell_escape(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_result_serialization() {
        let r = ExecutionResult {
            exit_code: 0,
            stdout: "ok".into(),
            stderr: String::new(),
            duration_seconds: 1.5,
            timed_out: false,
        };
        let json = serde_json::to_string(&r).unwrap();
        let r2: ExecutionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(r2.exit_code, 0);
        assert!(!r2.timed_out);
    }

    #[test]
    fn executor_construction() {
        let vm = VM {
            name: "test-vm".into(),
            size: "Standard_D2s_v3".into(),
            region: "eastus".into(),
            created_at: None,
            tags: None,
        };
        let exec = Executor::new(vm, 60, Some(2222));
        assert_eq!(exec.timeout_seconds, 3600);
        assert_eq!(exec.tunnel_port, Some(2222));
    }

    #[test]
    fn executor_no_tunnel() {
        let vm = VM {
            name: "vm".into(),
            size: "s".into(),
            region: "r".into(),
            created_at: None,
            tags: None,
        };
        let exec = Executor::new(vm, 10, None);
        assert!(exec.tunnel_port.is_none());
    }
}
