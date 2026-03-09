//! Launcher detection and context injection strategy.
//!
//! Detects whether running under Claude Code, Copilot, or Amplifier.
//! Each launcher has a different context injection strategy:
//! - Claude: writes to `.claude/runtime/hook_context.json` (pull model)
//! - Copilot: appends to `AGENTS.md` with HTML markers (push model)
//!
//! Note: Strategies only affect WHERE context is stored, not WHETHER
//! operations are blocked. Security checks (CWD, branch, --no-verify)
//! are independent of the detected launcher.

use amplihack_state::PythonBridge;
use amplihack_types::ProjectDirs;
use serde_json::Value;
use std::time::Duration;

/// Detected launcher type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherType {
    ClaudeCode,
    Copilot,
    Amplifier,
    Unknown,
}

/// Detect which launcher is running by checking environment variables.
pub fn detect_launcher() -> LauncherType {
    // Copilot sets specific env vars.
    if std::env::var("GITHUB_COPILOT_AGENT").is_ok() || std::env::var("COPILOT_AGENT").is_ok() {
        return LauncherType::Copilot;
    }

    // Amplifier sets its own marker.
    if std::env::var("AMPLIFIER_SESSION").is_ok() {
        return LauncherType::Amplifier;
    }

    // Claude Code is the default.
    if std::env::var("CLAUDE_CODE_SESSION").is_ok() || std::env::var("CLAUDE_SESSION_ID").is_ok() {
        return LauncherType::ClaudeCode;
    }

    LauncherType::Unknown
}

/// Inject context based on the detected launcher.
///
/// This is a side-effect-only operation — it never blocks.
/// Returns `None` always (strategies don't affect security decisions).
pub fn inject_context(dirs: &ProjectDirs, input_data: &Value) -> Option<Value> {
    let launcher = detect_launcher();

    match launcher {
        LauncherType::Copilot => {
            // Delegate to Python for AGENTS.md injection.
            inject_copilot_context(dirs, input_data);
        }
        LauncherType::ClaudeCode | LauncherType::Amplifier => {
            // Claude Code auto-discovers .claude/ files — no extra injection needed.
        }
        LauncherType::Unknown => {
            // Default: no context injection.
        }
    }

    // Strategies never block — all security checks are independent.
    None
}

/// Embedded Python bridge script for Copilot context injection.
const COPILOT_CONTEXT_BRIDGE: &str = r#"
import sys
import json

try:
    input_data = json.load(sys.stdin)
    project_path = input_data.get("project_path", "")

    try:
        from amplihack.context.adaptive.strategies import CopilotStrategy
        import logging
        strategy = CopilotStrategy(project_path, logging.getLogger("amplihack"))
        strategy.inject_context()
        json.dump({"injected": True}, sys.stdout)
    except ImportError:
        json.dump({"injected": False, "reason": "strategies not available"}, sys.stdout)
    except Exception as e:
        json.dump({"injected": False, "error": str(e)}, sys.stdout)
except Exception as e:
    json.dump({"injected": False, "error": str(e)}, sys.stdout)
"#;

fn inject_copilot_context(dirs: &ProjectDirs, _input_data: &Value) {
    let input = serde_json::json!({
        "project_path": dirs.root.display().to_string(),
    });

    // Best-effort — don't block on failure.
    match PythonBridge::call(COPILOT_CONTEXT_BRIDGE, &input, Duration::from_secs(5)) {
        Ok(_) => {}
        Err(e) => {
            tracing::debug!("Copilot context injection failed (non-fatal): {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_launcher_by_default() {
        // In test environment, no launcher env vars should be set.
        // We can't guarantee this, so just check it doesn't panic.
        let _ = detect_launcher();
    }

    #[test]
    fn inject_context_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let input = serde_json::json!({});
        assert!(inject_context(&dirs, &input).is_none());
    }
}
