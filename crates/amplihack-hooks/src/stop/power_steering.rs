//! Power steering: checks if the agent has completed enough work.
//!
//! Reads power steering configuration and counter files to determine
//! if the session should be blocked or approved.

use amplihack_state::AtomicCounter;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Check if power steering should run for this project.
pub fn should_run(project_root: &Path) -> bool {
    // Check if power steering is configured.
    let config_path = project_root
        .join(".claude")
        .join("tools")
        .join("amplihack")
        .join(".power_steering_config");

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
    project_root: &Path,
    session_id: &str,
    _transcript_path: Option<&Path>,
) -> anyhow::Result<Option<Value>> {
    let ps_dir = project_root
        .join(".claude")
        .join("runtime")
        .join("power-steering")
        .join(session_id);
    fs::create_dir_all(&ps_dir)?;

    let counter = AtomicCounter::new(ps_dir.join("session_count.json"));
    let count = counter.increment()?;

    // First stop: always approve (let power steering checker handle on subsequent stops).
    if count <= 1 {
        return Ok(None);
    }

    // For subsequent stops, approve (power steering logic is in Python checker).
    // The full power steering analysis requires the Python SDK bridge
    // for reading transcripts and evaluating completion evidence.
    // This will be enhanced when the SDK bridge pattern is fully established.
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_enabled_by_default() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!should_run(dir.path()));
    }

    #[test]
    fn enabled_when_config_exists() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".claude").join("tools").join("amplihack");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join(".power_steering_config"),
            r#"{"enabled": true}"#,
        )
        .unwrap();
        assert!(should_run(dir.path()));
    }
}
