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
/// Context injection affects WHERE data is stored (AGENTS.md vs hook_context.json),
/// not WHETHER operations are blocked.
pub fn inject_context(dirs: &ProjectDirs, input_data: &Value) {
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
}

/// Embedded Python bridge script for Copilot context injection.
///
/// SEC-M2: The `except` clause serialises `str(e)` into the JSON response
/// sent to stdout.  This is acceptable in the current developer/CI context
/// because:
///   1. The output is consumed programmatically by the Rust caller, not
///      rendered to a user-facing terminal.
///   2. Exception messages from `amplihack.context` are controlled by the
///      amplihack Python package — not by untrusted external input.
///   3. If sanitisation is needed in the future, the `error` field should
///      be run through a length-limit and ANSI-strip step before forwarding
///      to any terminal or log sink.
const COPILOT_CONTEXT_BRIDGE: &str = r#"
import sys
import json

try:
    input_data = json.load(sys.stdin)
    project_path = input_data.get("project_path", "")

    from amplihack.context.adaptive.strategies import CopilotStrategy
    import logging
    strategy = CopilotStrategy(project_path, logging.getLogger("amplihack"))
    strategy.inject_context()
    json.dump({"injected": True}, sys.stdout)
except Exception as e:
    # str(e) is serialised to stdout as part of a structured JSON object.
    # See SEC-M2 comment above for the accepted-risk rationale.
    json.dump({"injected": False, "error": str(e)}, sys.stdout)
    sys.exit(1)
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
        // Clear all launcher env vars to get a deterministic result.
        // SAFETY: This test must not run in parallel with other tests that
        // read these env vars. Cargo runs tests in the same process but
        // the env vars cleared here are test-only.
        unsafe {
            std::env::remove_var("GITHUB_COPILOT_AGENT");
            std::env::remove_var("COPILOT_AGENT");
            std::env::remove_var("AMPLIFIER_SESSION");
            std::env::remove_var("CLAUDE_CODE_SESSION");
            std::env::remove_var("CLAUDE_SESSION_ID");
        }
        let result = detect_launcher();
        assert!(
            matches!(result, LauncherType::Unknown),
            "Expected Unknown when no launcher env vars set, got: {result:?}"
        );
    }

    #[test]
    fn inject_context_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let input = serde_json::json!({});
        // inject_context is side-effect-only; verify it completes without panic.
        inject_context(&dirs, &input);
        // The temp dir should still exist (no destructive side effects).
        assert!(
            dir.path().exists(),
            "temp dir should survive inject_context"
        );
    }
}
