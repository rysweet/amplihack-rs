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

use amplihack_types::ProjectDirs;
use serde_json::Value;
use std::fs;
use std::path::Path;

const AGENTS_FILE: &str = "AGENTS.md";
const CONTEXT_MARKER_START: &str = "<!-- AMPLIHACK_CONTEXT_START -->";
const CONTEXT_MARKER_END: &str = "<!-- AMPLIHACK_CONTEXT_END -->";
const MAX_CONTEXT_SIZE: usize = 10 * 1024 * 1024;

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

fn inject_copilot_context(dirs: &ProjectDirs, input_data: &Value) {
    if let Err(error) = write_copilot_context(dirs, input_data) {
        tracing::warn!("Copilot context injection failed (non-fatal): {}", error);
    }
}

fn write_copilot_context(dirs: &ProjectDirs, input_data: &Value) -> anyhow::Result<()> {
    let agents_path = dirs.root.join(AGENTS_FILE);
    validate_agents_path(&dirs.root, &agents_path)?;

    let context_markdown = format_context_markdown(input_data)?;
    let mut content = if agents_path.exists() {
        fs::read_to_string(&agents_path)?
    } else {
        "# Amplihack Agents\n\n".to_string()
    };
    content = remove_old_context(&content);

    let mut lines = content.lines().map(ToString::to_string).collect::<Vec<_>>();
    let title_line = lines
        .iter()
        .position(|line| line.starts_with("# "))
        .unwrap_or(0);
    lines.insert(title_line + 1, String::new());
    lines.insert(title_line + 2, context_markdown);
    lines.insert(title_line + 3, String::new());

    fs::write(agents_path, lines.join("\n"))?;
    Ok(())
}

fn validate_agents_path(project_root: &Path, agents_path: &Path) -> anyhow::Result<()> {
    let root = project_root.canonicalize()?;
    let parent = agents_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("AGENTS.md path has no parent"))?
        .canonicalize()?;
    anyhow::ensure!(
        parent == root,
        "AGENTS.md path escapes project root: {}",
        agents_path.display()
    );
    Ok(())
}

fn format_context_markdown(input_data: &Value) -> anyhow::Result<String> {
    let json = serde_json::to_string_pretty(input_data)?;
    anyhow::ensure!(
        json.len() <= MAX_CONTEXT_SIZE,
        "Context too large: {} bytes (max {})",
        json.len(),
        MAX_CONTEXT_SIZE
    );

    Ok(format!(
        "{CONTEXT_MARKER_START}\n\n## Current Session Context\n\n**Launcher**: Copilot CLI (via amplihack)\n\n**Context Data**:\n```json\n{json}\n```\n\n{CONTEXT_MARKER_END}"
    ))
}

fn remove_old_context(content: &str) -> String {
    let Some(start) = content.find(CONTEXT_MARKER_START) else {
        return content.to_string();
    };
    let Some(end) = content.find(CONTEXT_MARKER_END) else {
        return content.to_string();
    };
    let before = &content[..start];
    let after = &content[end + CONTEXT_MARKER_END.len()..];
    format!("{}\n\n{}", before.trim_end(), after.trim_start())
        .trim()
        .to_string()
        + "\n"
}

#[cfg(test)]
fn set_launcher_env(
    copilot: Option<&str>,
    amplifier: Option<&str>,
    claude_code: Option<&str>,
    claude_session: Option<&str>,
) {
    match copilot {
        Some(value) => unsafe { std::env::set_var("GITHUB_COPILOT_AGENT", value) },
        None => unsafe { std::env::remove_var("GITHUB_COPILOT_AGENT") },
    }
    unsafe { std::env::remove_var("COPILOT_AGENT") };
    match amplifier {
        Some(value) => unsafe { std::env::set_var("AMPLIFIER_SESSION", value) },
        None => unsafe { std::env::remove_var("AMPLIFIER_SESSION") },
    }
    match claude_code {
        Some(value) => unsafe { std::env::set_var("CLAUDE_CODE_SESSION", value) },
        None => unsafe { std::env::remove_var("CLAUDE_CODE_SESSION") },
    }
    match claude_session {
        Some(value) => unsafe { std::env::set_var("CLAUDE_SESSION_ID", value) },
        None => unsafe { std::env::remove_var("CLAUDE_SESSION_ID") },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;

    #[test]
    fn unknown_launcher_by_default() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_launcher_env(None, None, None, None);
        let result = detect_launcher();
        assert!(
            matches!(result, LauncherType::Unknown),
            "Expected Unknown when no launcher env vars set, got: {result:?}"
        );
    }

    #[test]
    fn inject_context_does_not_panic() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_launcher_env(None, None, None, None);
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

    #[test]
    fn inject_context_writes_agents_file_for_copilot() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_launcher_env(Some("1"), None, None, None);
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let input = serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "ls"}});

        inject_context(&dirs, &input);

        let content = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        // Restore env before lock drops so other tests see a clean environment.
        set_launcher_env(None, None, None, None);
        assert!(content.contains(CONTEXT_MARKER_START));
        assert!(content.contains("\"tool_name\": \"Bash\""));
        assert!(content.contains("Copilot CLI (via amplihack)"));
    }

    #[test]
    fn inject_context_replaces_existing_marker_block() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_launcher_env(Some("1"), None, None, None);
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let agents = dir.path().join("AGENTS.md");
        fs::write(
            &agents,
            format!(
                "# Amplihack Agents\n\n{CONTEXT_MARKER_START}\nold\n{CONTEXT_MARKER_END}\n\nkeep me\n"
            ),
        )
        .unwrap();

        inject_context(&dirs, &serde_json::json!({"tool_name": "Read"}));

        let content = fs::read_to_string(&agents).unwrap();
        // Restore env before lock drops so other tests see a clean environment.
        set_launcher_env(None, None, None, None);
        assert_eq!(content.matches(CONTEXT_MARKER_START).count(), 1);
        assert!(content.contains("\"tool_name\": \"Read\""));
        assert!(content.contains("keep me"));
        assert!(!content.contains("\nold\n"));
    }
}
