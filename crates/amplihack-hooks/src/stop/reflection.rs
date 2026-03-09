//! Reflection: post-session analysis via Python SDK bridge.
//!
//! After a session ends (and lock/power-steering don't block),
//! runs Claude reflection to generate feedback on the work done.
//! Results are saved to FEEDBACK_SUMMARY.md.

use amplihack_state::PythonBridge;
use amplihack_types::ProjectDirs;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// Check if reflection should run.
pub fn should_run(_dirs: &ProjectDirs) -> bool {
    // Check environment variable.
    match std::env::var("AMPLIHACK_ENABLE_REFLECTION") {
        Ok(val) => matches!(val.to_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => false,
    }
}

/// Embedded Python bridge script for session reflection.
const REFLECTION_BRIDGE: &str = r#"
import sys
import json

try:
    input_data = json.load(sys.stdin)
    session_id = input_data.get("session_id", "")
    project_path = input_data.get("project_path", "")
    transcript_path = input_data.get("transcript_path", "")

    from amplihack.hooks.stop import run_claude_reflection
    session_dir = input_data.get("session_dir", "")

    # Load conversation from transcript
    conversation = []
    if transcript_path:
        import os
        if os.path.exists(transcript_path):
            with open(transcript_path, "r") as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        entry = json.loads(line)
                        if entry.get("type") in ("user", "assistant") and "message" in entry:
                            msg = entry["message"]
                            if isinstance(msg, str):
                                conversation.append({"role": entry["type"], "content": msg})
                            elif isinstance(msg, list):
                                text = " ".join(
                                    b.get("text", "") for b in msg
                                    if isinstance(b, dict) and b.get("type") == "text"
                                )
                                if text:
                                    conversation.append({"role": entry["type"], "content": text})
                    except json.JSONDecodeError:
                        continue

    result = run_claude_reflection(session_dir, project_path, conversation)
    if result:
        json.dump({"success": True, "template": result}, sys.stdout)
    else:
        json.dump({"success": False, "reason": "empty result"}, sys.stdout)
        sys.exit(1)
except Exception as e:
    json.dump({"success": False, "error": str(e)}, sys.stdout)
    sys.exit(1)
"#;

/// Run session reflection and return findings if session should be blocked.
///
/// Returns `Some(block_json)` if reflection produced findings that should
/// be presented to the user, `None` otherwise.
pub fn run_reflection(
    dirs: &ProjectDirs,
    session_id: &str,
    transcript_path: Option<&Path>,
) -> anyhow::Result<Option<Value>> {
    let session_dir = dirs.session_logs(session_id);
    fs::create_dir_all(&session_dir)?;

    // Check semaphore to avoid re-presenting.
    let semaphore_path = session_dir.join(".reflection_presented");
    if semaphore_path.exists() {
        return Ok(None);
    }

    let input = serde_json::json!({
        "session_id": session_id,
        "project_path": dirs.root.display().to_string(),
        "session_dir": session_dir.display().to_string(),
        "transcript_path": transcript_path.map(|p| p.display().to_string()).unwrap_or_default(),
    });

    match PythonBridge::call(REFLECTION_BRIDGE, &input, Duration::from_secs(30)) {
        Ok(result) => {
            let success = result
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            if success && let Some(template) = result.get("template").and_then(Value::as_str) {
                // Save feedback to files.
                let _ = fs::write(session_dir.join("FEEDBACK_SUMMARY.md"), template);

                // Mirror for backward compatibility.
                let reflection_dir = dirs.runtime.join("reflection");
                let _ = fs::create_dir_all(&reflection_dir);
                let _ = fs::write(reflection_dir.join("current_findings.md"), template);

                // Set semaphore.
                let _ = fs::write(&semaphore_path, "");

                // Block with findings.
                return Ok(Some(serde_json::json!({
                    "decision": "block",
                    "reason": format!(
                        "📋 Session Reflection\n\n{}\n\nPlease review the findings above.",
                        template
                    )
                })));
            }

            Ok(None)
        }
        Err(e) => {
            tracing::warn!("Reflection bridge failed: {}", e);
            Ok(None)
        }
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
    fn semaphore_prevents_re_presentation() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let session_dir = dirs.session_logs("test-session");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join(".reflection_presented"), "").unwrap();

        // Should return None even without running bridge.
        let result = run_reflection(&dirs, "test-session", None).unwrap();
        assert!(result.is_none());
    }
}
