//! Lock mode: blocks session exit when `.lock_active` exists.
//!
//! - Reads continuation prompt from `.continuation_prompt`
//! - Increments lock invocation counter per session
//! - Safety valve: auto-approves after N iterations

use amplihack_state::AtomicCounter;
use amplihack_state::env_config::env_u64;
use amplihack_types::ProjectDirs;
use serde_json::Value;
use std::fs;

/// Maximum lock iterations before safety valve triggers.
const DEFAULT_MAX_LOCK_ITERATIONS: u64 = 50;

/// Check if lock mode is active.
pub fn is_lock_active(dirs: &ProjectDirs) -> bool {
    dirs.lock_active_file().exists()
}

/// Handle lock mode: increment counter, check safety valve, block with prompt.
pub fn handle_lock_mode(dirs: &ProjectDirs, session_id: &str) -> anyhow::Result<Value> {
    let locks_dir = dirs.session_locks(session_id);
    fs::create_dir_all(&locks_dir)?;

    // Use .txt for Python parity (Python lock counter is plain text).
    let counter = AtomicCounter::new(locks_dir.join("lock_invocations.txt"));
    let count = counter.increment()?;

    let max_iterations = get_max_iterations();

    // Safety valve: auto-approve after too many iterations.
    if count >= max_iterations {
        tracing::warn!(
            "SAFETY VALVE: Lock mode auto-approving after {} iterations",
            count
        );

        // Remove lock file. If it's already gone, that's fine.
        match fs::remove_file(dirs.lock_active_file()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::error!("Failed to clear lock file: {}", e),
        }

        return Ok(serde_json::json!({"decision": "approve"}));
    }

    // Read continuation prompt.
    let prompt = read_continuation_prompt(dirs);

    Ok(serde_json::json!({
        "decision": "block",
        "reason": prompt
    }))
}

/// Read the continuation prompt file.
fn read_continuation_prompt(dirs: &ProjectDirs) -> String {
    let prompt_path = dirs.continuation_prompt_file();

    match fs::read_to_string(&prompt_path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() || trimmed.len() > 1000 {
                super::DEFAULT_CONTINUATION_PROMPT.to_string()
            } else {
                if trimmed.len() > 500 {
                    tracing::warn!(
                        "Continuation prompt is long ({} chars), consider shortening",
                        trimmed.len()
                    );
                }
                trimmed.to_string()
            }
        }
        Err(_) => super::DEFAULT_CONTINUATION_PROMPT.to_string(),
    }
}

/// Get the max lock iterations from env or default.
fn get_max_iterations() -> u64 {
    env_u64("AMPLIHACK_MAX_LOCK_ITERATIONS", DEFAULT_MAX_LOCK_ITERATIONS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_not_active_in_temp() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        assert!(!is_lock_active(&dirs));
    }

    #[test]
    fn lock_active_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(&dirs.locks).unwrap();
        fs::write(dirs.lock_active_file(), "").unwrap();
        assert!(is_lock_active(&dirs));
    }

    #[test]
    fn handle_lock_increments_counter() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(&dirs.locks).unwrap();
        fs::write(dirs.lock_active_file(), "").unwrap();

        let result = handle_lock_mode(&dirs, "test-session").unwrap();
        assert_eq!(result["decision"], "block");
    }

    #[test]
    fn safety_valve_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(&dirs.locks).unwrap();
        fs::write(dirs.lock_active_file(), "").unwrap();

        // Pre-seed counter just below threshold so next increment triggers safety valve.
        let session_locks = dirs.session_locks("valve-session");
        fs::create_dir_all(&session_locks).unwrap();
        fs::write(
            session_locks.join("lock_invocations.txt"),
            format!(r#"{{"value":{}}}"#, DEFAULT_MAX_LOCK_ITERATIONS - 1),
        )
        .unwrap();

        let result = handle_lock_mode(&dirs, "valve-session").unwrap();
        assert_eq!(result["decision"], "approve");
        assert!(!dirs.lock_active_file().exists());
    }

    #[test]
    fn default_continuation_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let prompt = read_continuation_prompt(&dirs);
        assert_eq!(prompt, super::super::DEFAULT_CONTINUATION_PROMPT);
    }

    #[test]
    fn custom_continuation_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(&dirs.locks).unwrap();
        fs::write(dirs.continuation_prompt_file(), "Keep going!").unwrap();

        let prompt = read_continuation_prompt(&dirs);
        assert_eq!(prompt, "Keep going!");
    }
}
