//! Session stop hook: stores session memory and warns about uncommitted work.
//!
//! Two responsibilities:
//! 1. Delegate to Python bridge for MemoryCoordinator.store()
//! 2. Check for uncommitted git changes and warn the user
//!
//! Neither blocks session exit (fail-open).

use crate::protocol::{FailurePolicy, Hook};
use amplihack_state::PythonBridge;
use amplihack_types::HookInput;
use serde_json::Value;
use std::process::Command;
use std::time::Duration;

/// Embedded Python bridge script for memory storage.
const MEMORY_STORE_BRIDGE: &str = r#"
import sys
import json

try:
    input_data = json.load(sys.stdin)
    action = input_data.get("action", "store")
    session_id = input_data.get("session_id", "")
    transcript_path = input_data.get("transcript_path", "")

    from amplihack.memory.coordinator import MemoryCoordinator
    coordinator = MemoryCoordinator()
    coordinator.store(session_id=session_id, transcript_path=transcript_path)
    result = {"stored": True, "memories_count": 0}
    json.dump(result, sys.stdout)
except Exception as e:
    json.dump({"stored": False, "error": str(e)}, sys.stdout)
    sys.exit(1)
"#;

pub struct SessionStopHook;

impl Hook for SessionStopHook {
    fn name(&self) -> &'static str {
        "session_stop"
    }

    fn failure_policy(&self) -> FailurePolicy {
        // Don't block session exit on memory store failure.
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (session_id, transcript_path) = match input {
            HookInput::SessionStop {
                session_id,
                transcript_path,
                ..
            } => (session_id, transcript_path),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        // 1. Store session memory via Python bridge.
        let bridge_input = serde_json::json!({
            "action": "store",
            "session_id": session_id.as_deref().unwrap_or_default(),
            "transcript_path": transcript_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        });

        match PythonBridge::call(MEMORY_STORE_BRIDGE, &bridge_input, Duration::from_secs(15)) {
            Ok(result) => {
                let stored = result
                    .get("stored")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !stored {
                    let error = result
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    tracing::error!("Memory store failed: {}", error);
                }
            }
            Err(e) => {
                tracing::error!("Memory store bridge error: {}", e);
            }
        }

        // 2. Check for uncommitted work and warn.
        warn_uncommitted_work();

        Ok(Value::Object(serde_json::Map::new()))
    }
}

/// Check git status and print warnings about uncommitted changes.
///
/// Best-effort: never blocks session exit.
fn warn_uncommitted_work() {
    let staged = match Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect::<Vec<_>>(),
        _ => return,
    };

    let unstaged = match Command::new("git").args(["diff", "--name-only"]).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

    let untracked = match Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

    if staged.is_empty() && unstaged.is_empty() && untracked.is_empty() {
        return;
    }

    eprintln!("\n⚠️  Uncommitted work detected:");

    if !staged.is_empty() {
        eprintln!(
            "\n  Staged ({} file{}):",
            staged.len(),
            if staged.len() == 1 { "" } else { "s" }
        );
        for f in staged.iter().take(10) {
            eprintln!("    ✅ {f}");
        }
        if staged.len() > 10 {
            eprintln!("    ... and {} more", staged.len() - 10);
        }
    }

    if !unstaged.is_empty() {
        eprintln!(
            "\n  Modified ({} file{}):",
            unstaged.len(),
            if unstaged.len() == 1 { "" } else { "s" }
        );
        for f in unstaged.iter().take(10) {
            eprintln!("    📝 {f}");
        }
        if unstaged.len() > 10 {
            eprintln!("    ... and {} more", unstaged.len() - 10);
        }
    }

    if !untracked.is_empty() {
        eprintln!(
            "\n  Untracked ({} file{}):",
            untracked.len(),
            if untracked.len() == 1 { "" } else { "s" }
        );
        for f in untracked.iter().take(10) {
            eprintln!("    ❓ {f}");
        }
        if untracked.len() > 10 {
            eprintln!("    ... and {} more", untracked.len() - 10);
        }
    }

    let total = staged.len() + unstaged.len() + untracked.len();
    eprintln!("\n  💡 To commit: git add -A && git commit -m \"save work\"");
    eprintln!("  💡 To stash:  git stash push -m \"session work\"");
    eprintln!(
        "  📊 Total: {total} file{} with uncommitted changes\n",
        if total == 1 { "" } else { "s" }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_unknown_events() {
        let hook = SessionStopHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn warn_uncommitted_work_does_not_panic() {
        // Just verify it doesn't panic in test environment.
        warn_uncommitted_work();
    }
}
