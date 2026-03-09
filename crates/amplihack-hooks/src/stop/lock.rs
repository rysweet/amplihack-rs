//! Lock mode: blocks session exit when `.lock_active` exists.
//!
//! - Reads continuation prompt from `.continuation_prompt`
//! - Increments lock invocation counter per session
//! - Safety valve: auto-approves after N iterations

use amplihack_state::AtomicCounter;
use amplihack_state::env_config::env_u64;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Maximum lock iterations before safety valve triggers.
const DEFAULT_MAX_LOCK_ITERATIONS: u64 = 50;

/// Check if lock mode is active.
pub fn is_lock_active(project_root: &Path) -> bool {
    let lock_path = project_root
        .join(".claude")
        .join("runtime")
        .join("locks")
        .join(".lock_active");
    lock_path.exists()
}

/// Handle lock mode: increment counter, check safety valve, block with prompt.
pub fn handle_lock_mode(project_root: &Path, session_id: &str) -> anyhow::Result<Value> {
    let locks_dir = project_root
        .join(".claude")
        .join("runtime")
        .join("locks")
        .join(session_id);
    fs::create_dir_all(&locks_dir)?;

    let counter = AtomicCounter::new(locks_dir.join("lock_invocations.json"));
    let count = counter.increment()?;

    let max_iterations = get_max_iterations();

    // Safety valve: auto-approve after too many iterations.
    if count >= max_iterations {
        tracing::warn!(
            "SAFETY VALVE: Lock mode auto-approving after {} iterations",
            count
        );

        // Remove lock file.
        let lock_path = project_root
            .join(".claude")
            .join("runtime")
            .join("locks")
            .join(".lock_active");
        let _ = fs::remove_file(lock_path);

        return Ok(serde_json::json!({"decision": "approve"}));
    }

    // Read continuation prompt.
    let prompt = read_continuation_prompt(project_root);

    Ok(serde_json::json!({
        "decision": "block",
        "reason": prompt
    }))
}

/// Read the continuation prompt file.
fn read_continuation_prompt(project_root: &Path) -> String {
    let prompt_path = project_root
        .join(".claude")
        .join("runtime")
        .join("locks")
        .join(".continuation_prompt");

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
        assert!(!is_lock_active(dir.path()));
    }

    #[test]
    fn lock_active_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join(".claude").join("runtime").join("locks");
        fs::create_dir_all(&locks_dir).unwrap();
        fs::write(locks_dir.join(".lock_active"), "").unwrap();
        assert!(is_lock_active(dir.path()));
    }

    #[test]
    fn handle_lock_increments_counter() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join(".claude").join("runtime").join("locks");
        fs::create_dir_all(&locks_dir).unwrap();
        fs::write(locks_dir.join(".lock_active"), "").unwrap();

        let result = handle_lock_mode(dir.path(), "test-session").unwrap();
        assert_eq!(result["decision"], "block");
    }

    #[test]
    fn safety_valve_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join(".claude").join("runtime").join("locks");
        fs::create_dir_all(&locks_dir).unwrap();
        fs::write(locks_dir.join(".lock_active"), "").unwrap();

        // Set max iterations very low.
        unsafe {
            std::env::set_var("AMPLIHACK_MAX_LOCK_ITERATIONS", "2");
        }

        let _ = handle_lock_mode(dir.path(), "valve-session").unwrap();
        let _ = handle_lock_mode(dir.path(), "valve-session").unwrap();

        // Third should trigger safety valve.
        // Counter is at 2 now, which equals max.
        // Actually counter starts at 0, first increment = 1, second = 2.
        // Since 2 >= 2, it should approve.
        // But we already called twice, let's check the lock file was removed.
        assert!(!locks_dir.join(".lock_active").exists());

        unsafe {
            std::env::remove_var("AMPLIHACK_MAX_LOCK_ITERATIONS");
        }
    }

    #[test]
    fn default_continuation_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = read_continuation_prompt(dir.path());
        assert_eq!(prompt, super::super::DEFAULT_CONTINUATION_PROMPT);
    }

    #[test]
    fn custom_continuation_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join(".claude").join("runtime").join("locks");
        fs::create_dir_all(&locks_dir).unwrap();
        fs::write(locks_dir.join(".continuation_prompt"), "Keep going!").unwrap();

        let prompt = read_continuation_prompt(dir.path());
        assert_eq!(prompt, "Keep going!");
    }
}
