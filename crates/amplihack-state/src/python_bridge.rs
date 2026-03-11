//! Python subprocess bridge for SDK calls.
//!
//! Embeds Python scripts at compile time via `include_str!()` and
//! executes them via subprocess with JSON IPC and hard timeouts.

use serde_json::Value;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Errors from Python bridge operations.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("Python bridge IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Python bridge JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Python bridge timeout after {timeout:?}")]
    Timeout { timeout: Duration },
    #[error("Python bridge exited with status {code}: {stderr}")]
    ProcessFailed { code: i32, stderr: String },
    #[error("Python bridge: {0}")]
    Other(String),
}

/// Bridge to Python for SDK calls.
///
/// Embedded scripts are written to temp files with 0o600 permissions
/// and cleaned up after execution.
pub struct PythonBridge;

/// RAII guard that removes a temp file on drop (even on panic).
struct TempScript(PathBuf);

impl Drop for TempScript {
    fn drop(&mut self) {
        // Best-effort cleanup — failing to delete a temp file is not fatal.
        let _ = std::fs::remove_file(&self.0);
    }
}

impl PythonBridge {
    /// Call a Python bridge script with JSON input and return JSON output.
    ///
    /// The script is written to a temp file, executed, and cleaned up
    /// via RAII guard (even on panic). A hard timeout is enforced.
    pub fn call(
        script_content: &str,
        input: &Value,
        timeout: Duration,
    ) -> Result<Value, BridgeError> {
        let script_path = write_temp_script(script_content)?;
        let _guard = TempScript(script_path.clone());

        Self::execute_script(&script_path, input, timeout)
    }

    fn execute_script(
        script_path: &Path,
        input: &Value,
        timeout: Duration,
    ) -> Result<Value, BridgeError> {
        let mut child = Command::new("python3")
            .arg(script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Write input JSON to stdin.
        if let Some(mut stdin) = child.stdin.take() {
            let input_bytes = serde_json::to_vec(input)?;
            stdin.write_all(&input_bytes)?;
            // Drop stdin to signal EOF.
        }

        // Wait with timeout.
        let output = wait_with_timeout(&mut child, timeout)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(BridgeError::ProcessFailed {
                code: output.status.code().unwrap_or(-1),
                stderr,
            });
        }

        let stdout = &output.stdout;
        if stdout.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        let value: Value = serde_json::from_slice(stdout)?;
        Ok(value)
    }
}

/// Write a Python script to a temp file with restricted permissions.
fn write_temp_script(content: &str) -> Result<PathBuf, io::Error> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let dir = std::env::temp_dir();
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!(
        "amplihack-bridge-{}-{}.py",
        std::process::id(),
        unique
    ));

    std::fs::write(&path, content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(path)
}

/// Wait for a child process with timeout.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::Output, BridgeError> {
    let start = std::time::Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    io::Read::read_to_end(&mut out, &mut stdout)?;
                }
                if let Some(mut err) = child.stderr.take() {
                    io::Read::read_to_end(&mut err, &mut stderr)?;
                }
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    // Best-effort cleanup on timeout — process may already be dead.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(BridgeError::Timeout { timeout });
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(BridgeError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_simple_echo_script() {
        let script = r#"
import sys, json
data = json.load(sys.stdin)
json.dump({"echo": data}, sys.stdout)
"#;
        let input = serde_json::json!({"test": true});
        let result = PythonBridge::call(script, &input, Duration::from_secs(5)).unwrap();
        assert_eq!(result["echo"]["test"], true);
    }

    #[test]
    fn call_timeout() {
        let script = r#"
import time
time.sleep(60)
"#;
        let input = serde_json::json!({});
        let result = PythonBridge::call(script, &input, Duration::from_millis(100));
        assert!(matches!(result, Err(BridgeError::Timeout { .. })));
    }

    #[test]
    fn call_script_error() {
        let script = r#"
import sys
sys.exit(1)
"#;
        let input = serde_json::json!({});
        let result = PythonBridge::call(script, &input, Duration::from_secs(5));
        assert!(matches!(result, Err(BridgeError::ProcessFailed { .. })));
    }
}
