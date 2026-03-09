//! Session start hook: initializes session state and injects context.
//!
//! On session start, this hook:
//! 1. Checks for version mismatches
//! 2. Migrates global hooks if needed
//! 3. Captures original request
//! 4. Injects project context, learnings, and preferences
//! 5. Returns additional context for the session

use crate::protocol::{FailurePolicy, Hook};
use amplihack_state::PythonBridge;
use amplihack_types::HookInput;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Embedded Python bridge script for memory/context retrieval.
const MEMORY_CONTEXT_BRIDGE: &str = r#"
import sys
import json

try:
    input_data = json.load(sys.stdin)
    session_id = input_data.get("session_id", "")
    project_path = input_data.get("project_path", "")

    try:
        from amplihack.memory.coordinator import MemoryCoordinator
        coordinator = MemoryCoordinator()
        context = coordinator.get_context(
            session_id=session_id,
            project_path=project_path
        )
        result = {"context": context or "", "memories": []}
    except ImportError:
        result = {"context": "", "memories": []}
    except Exception as e:
        result = {"context": "", "error": str(e)}

    json.dump(result, sys.stdout)
except Exception as e:
    json.dump({"context": "", "error": str(e)}, sys.stdout)
"#;

pub struct SessionStartHook;

impl Hook for SessionStartHook {
    fn name(&self) -> &'static str {
        "session_start"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let session_id = match &input {
            HookInput::SessionStart { session_id, .. } => {
                session_id.clone().unwrap_or_else(generate_session_id)
            }
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut context_parts: Vec<String> = Vec::new();

        // Load project context (PROJECT.md).
        if let Some(ctx) = load_project_context(&project_root) {
            context_parts.push(ctx);
        }

        // Load recent learnings/discoveries.
        if let Some(learnings) = load_discoveries(&project_root) {
            context_parts.push(learnings);
        }

        // Load user preferences.
        if let Some(prefs) = load_user_preferences(&project_root) {
            context_parts.push(prefs);
        }

        // Get memory context via bridge.
        if let Some(memory_ctx) = get_memory_context(&session_id, &project_root)
            && !memory_ctx.is_empty()
        {
            context_parts.push(memory_ctx);
        }

        // Check for version mismatch.
        if let Some(version_notice) = check_version(&project_root) {
            context_parts.push(version_notice);
        }

        // Migrate global hooks if needed.
        if let Some(migration_notice) = migrate_global_hooks(&project_root) {
            context_parts.push(migration_notice);
        }

        let additional_context = context_parts.join("\n\n");

        if additional_context.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        Ok(serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": additional_context
            }
        }))
    }
}

fn load_project_context(project_root: &Path) -> Option<String> {
    let candidates = [
        project_root.join("PROJECT.md"),
        project_root
            .join(".claude")
            .join("context")
            .join("PROJECT.md"),
    ];

    for path in &candidates {
        if let Ok(content) = fs::read_to_string(path)
            && !content.trim().is_empty()
        {
            return Some(format!("## Project Context\n\n{}", content.trim()));
        }
    }

    None
}

fn load_discoveries(project_root: &Path) -> Option<String> {
    let path = project_root.join("DISCOVERIES.md");
    if let Ok(content) = fs::read_to_string(path)
        && !content.trim().is_empty()
    {
        return Some(format!("## Recent Learnings\n\n{}", content.trim()));
    }
    None
}

fn load_user_preferences(project_root: &Path) -> Option<String> {
    let candidates = [
        project_root
            .join(".claude")
            .join("context")
            .join("USER_PREFERENCES.md"),
        project_root.join("USER_PREFERENCES.md"),
    ];

    for path in &candidates {
        if let Ok(content) = fs::read_to_string(path)
            && !content.trim().is_empty()
        {
            return Some(content.trim().to_string());
        }
    }

    None
}

fn get_memory_context(session_id: &str, project_root: &Path) -> Option<String> {
    let input = serde_json::json!({
        "action": "get_context",
        "session_id": session_id,
        "project_path": project_root.display().to_string(),
    });

    match PythonBridge::call(MEMORY_CONTEXT_BRIDGE, &input, Duration::from_secs(10)) {
        Ok(result) => {
            let context = result.get("context").and_then(Value::as_str).unwrap_or("");
            if context.is_empty() {
                None
            } else {
                Some(context.to_string())
            }
        }
        Err(e) => {
            tracing::warn!("Memory context bridge error: {}", e);
            None
        }
    }
}

fn check_version(project_root: &Path) -> Option<String> {
    let version_file = project_root.join(".claude").join(".version");
    if !version_file.exists() {
        return None;
    }

    // Version checking is best-effort.
    match fs::read_to_string(&version_file) {
        Ok(content) => {
            let project_commit = content.trim();
            if project_commit.is_empty() {
                return None;
            }
            // Full version comparison would be done by the Python layer.
            // Here we just note the version exists.
            None
        }
        Err(_) => None,
    }
}

fn migrate_global_hooks(_project_root: &Path) -> Option<String> {
    // Check if global hooks exist that should be migrated.
    let home = std::env::var("HOME").ok()?;
    let global_settings = PathBuf::from(&home).join(".claude").join("settings.json");

    if !global_settings.exists() {
        return None;
    }

    let content = fs::read_to_string(&global_settings).ok()?;
    let settings: Value = serde_json::from_str(&content).ok()?;

    // Check if there are amplihack hooks in global settings.
    let hooks = settings.get("hooks")?;
    let has_amplihack_hooks = hooks
        .as_object()
        .map(|obj| {
            obj.values().any(|v| {
                v.as_array()
                    .map(|arr| {
                        arr.iter().any(|h| {
                            h.get("command")
                                .and_then(Value::as_str)
                                .map(|c| c.contains("amplihack"))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    if has_amplihack_hooks {
        Some(
            "⚠️ Global amplihack hooks detected in ~/.claude/settings.json. \
             These should be migrated to project-local hooks."
                .to_string(),
        )
    } else {
        None
    }
}

fn generate_session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("session-{}", now.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_unknown_events() {
        let hook = SessionStartHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn load_project_context_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_project_context(dir.path()).is_none());
    }

    #[test]
    fn load_project_context_exists() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("PROJECT.md"), "# My Project\nDescription").unwrap();
        let ctx = load_project_context(dir.path());
        assert!(ctx.is_some());
        assert!(ctx.unwrap().contains("My Project"));
    }

    #[test]
    fn generate_session_id_format() {
        let id = generate_session_id();
        assert!(id.starts_with("session-"));
    }
}
