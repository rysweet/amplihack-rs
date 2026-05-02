//! Advanced scenarios for `amplihack-session`:
//! * Custom [`CommandExecutor`] impl (replaces Python `_simulate_command_execution`).
//! * Structured logging with a child [`amplihack_session::ToolkitLogger`].
//! * Export & import round trip across two runtime directories.

use amplihack_session::{
    ClaudeSession, CommandExecutor, LogLevel, SessionConfig, SessionError, SessionToolkit,
    ToolkitLogger, safe_write_json,
};
use serde_json::{Value, json};
use std::process::Command;

/// Real shell executor: runs the command via `/bin/sh -c` and captures output.
///
/// Demonstrates the design-spec extension point that replaces the random-sleep
/// simulator from the Python original.
struct StdShellExecutor;

impl CommandExecutor for StdShellExecutor {
    fn execute(&self, command: &str, _kwargs: &Value) -> Result<Value, SessionError> {
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .output()
            .map_err(|e| SessionError::Corruption(format!("shell exec failed: {e}")))?;
        Ok(json!({
            "command": command,
            "status": if output.status.success() { "completed" } else { "failed" },
            "exit_code": output.status.code(),
            "stdout_bytes": output.stdout.len(),
            "stderr_bytes": output.stderr.len(),
        }))
    }
}

fn main() -> Result<(), SessionError> {
    let tmp = tempfile::tempdir().expect("tempdir");

    // Scenario 1: real shell executor.
    let cfg = SessionConfig {
        session_id: Some("shell-demo".into()),
        ..SessionConfig::default()
    };
    let mut session = ClaudeSession::with_executor(cfg, Box::new(StdShellExecutor));
    session.start();
    let r = session.execute_command("echo hi", None, json!({}))?;
    println!("shell run: {r}");
    session.stop();

    // Scenario 2: structured logging with a child component.
    let log_dir = tmp.path().join("logs");
    let parent = ToolkitLogger::builder()
        .session_id("logging-demo")
        .component("toolkit")
        .log_dir(&log_dir)
        .level(LogLevel::Debug)
        .enable_console(false)
        .enable_file(true)
        .build()?;
    parent.info("parent log line", Some(json!({"phase": "init"})))?;
    let child = parent.create_child_logger("worker")?;
    {
        let _op = child.operation("scan-files");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let entries = parent.get_session_logs(None)?;
    println!("captured {} log entries", entries.len());

    // Scenario 3: export/import round trip.
    let rt_a = tmp.path().join("rt-a");
    let rt_b = tmp.path().join("rt-b");
    let mut tk_a = SessionToolkit::new(&rt_a, true, "INFO")?;
    let id = tk_a.create_session("portable", None, None)?;
    tk_a.manager_mut().save_session(&id, true)?;
    let exported = tmp.path().join("portable.json");
    tk_a.export_session(&id, &exported)?;

    let mut tk_b = SessionToolkit::new(&rt_b, true, "INFO")?;
    let imported_id = tk_b.import_session(&exported)?;
    println!("imported session: {imported_id}");

    // Demonstrate safe_write_json for ad-hoc artifacts.
    safe_write_json(tmp.path().join("summary.json"), &json!({"sessions": 1}))?;
    Ok(())
}
