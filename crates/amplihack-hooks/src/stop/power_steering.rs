//! Power steering: checks if the agent has completed enough work.
//!
//! Reads power steering configuration and counter files to determine
//! if the session should be blocked or approved.

use amplihack_state::AtomicCounter;
use amplihack_types::ProjectDirs;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Check if power steering should run for this project.
pub fn should_run(dirs: &ProjectDirs) -> bool {
    let config_path = dirs.power_steering_config();

    if !config_path.exists() {
        return false;
    }

    match fs::read_to_string(&config_path) {
        Ok(content) => {
            if let Ok(config) = serde_json::from_str::<Value>(&content) {
                config
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

/// Check power steering state and decide whether to block.
///
/// Returns `Some(block_json)` if the session should be blocked,
/// `None` if it should be approved.
pub fn check(
    dirs: &ProjectDirs,
    session_id: &str,
    _transcript_path: Option<&Path>,
) -> anyhow::Result<Option<Value>> {
    let ps_dir = dirs.session_power_steering(session_id);
    fs::create_dir_all(&ps_dir)?;

    let counter = AtomicCounter::new(ps_dir.join("session_count"));
    let count = counter.increment()?;

    // First stop: always approve (let power steering checker handle on subsequent stops).
    if count <= 1 {
        return Ok(None);
    }

    // For subsequent stops, delegate to Python SDK bridge for work-completion analysis.
    // The full power steering analysis requires the Python SDK bridge
    // for reading transcripts and evaluating completion evidence.
    match run_power_steering_check(dirs, session_id, count) {
        Ok(Some(block)) => Ok(Some(block)),
        Ok(None) => Ok(None),
        Err(e) => {
            tracing::warn!("Power steering bridge failed, approving: {}", e);
            Ok(None)
        }
    }
}

/// Embedded Python bridge script for power steering analysis.
const POWER_STEERING_BRIDGE: &str = r#"
import sys
import json

try:
    input_data = json.load(sys.stdin)
    session_id = input_data.get("session_id", "")
    project_path = input_data.get("project_path", "")
    stop_count = input_data.get("stop_count", 0)

    from amplihack.hooks.stop import check_power_steering
    result = check_power_steering(
        session_id=session_id,
        project_path=project_path,
        stop_count=stop_count
    )
    json.dump(result or {"should_block": False}, sys.stdout)
except Exception as e:
    json.dump({"should_block": False, "error": str(e)}, sys.stdout)
    sys.exit(1)
"#;

fn run_power_steering_check(
    dirs: &ProjectDirs,
    session_id: &str,
    stop_count: u64,
) -> anyhow::Result<Option<Value>> {
    use amplihack_state::PythonBridge;
    use std::time::Duration;

    let input = serde_json::json!({
        "session_id": session_id,
        "project_path": dirs.root.display().to_string(),
        "stop_count": stop_count,
    });

    let result = PythonBridge::call(POWER_STEERING_BRIDGE, &input, Duration::from_secs(10))?;

    let should_block = result
        .get("should_block")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if should_block {
        let reason = result
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("Work appears incomplete. Continue working.");
        Ok(Some(serde_json::json!({
            "decision": "block",
            "reason": reason
        })))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_enabled_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        assert!(!should_run(&dirs));
    }

    #[test]
    fn enabled_when_config_exists() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(&dirs.tools_amplihack).unwrap();
        fs::write(dirs.power_steering_config(), r#"{"enabled": true}"#).unwrap();
        assert!(should_run(&dirs));
    }
}
