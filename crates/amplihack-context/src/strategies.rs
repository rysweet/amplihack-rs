use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

// ── Trait ──────────────────────────────────────────────────────────────

/// Strategy for injecting context into a specific agent launcher.
pub trait HookStrategy {
    /// Inject context and return the serialized payload that was written.
    fn inject_context(&self, context: &str) -> Result<String>;
    /// Remove any injected context artefacts.
    fn cleanup(&self) -> Result<()>;
    /// Called when the session stops.
    fn handle_stop(&self) -> Result<()> {
        self.cleanup()
    }
    fn pre_tool_use(&self, _tool_name: &str) -> Result<()> {
        Ok(())
    }
    fn post_tool_use(&self, _tool_name: &str) -> Result<()> {
        Ok(())
    }
    fn user_prompt_submit(&self, _prompt: &str) -> Result<()> {
        Ok(())
    }
}

// ── Claude ─────────────────────────────────────────────────────────────

/// Writes context as JSON to `.claude/runtime/hook_context.json`.
pub struct ClaudeStrategy {
    context_path: PathBuf,
}

impl ClaudeStrategy {
    pub fn new(project_dir: &Path) -> Self {
        Self {
            context_path: project_dir.join(".claude/runtime/hook_context.json"),
        }
    }

    pub fn context_path(&self) -> &Path {
        &self.context_path
    }
}

impl HookStrategy for ClaudeStrategy {
    fn inject_context(&self, context: &str) -> Result<String> {
        if let Some(parent) = self.context_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let payload = serde_json::json!({ "context": context });
        let json_str = serde_json::to_string_pretty(&payload)?;
        std::fs::write(&self.context_path, &json_str)?;
        Ok(json_str)
    }

    fn cleanup(&self) -> Result<()> {
        if self.context_path.exists() {
            std::fs::remove_file(&self.context_path)?;
        }
        Ok(())
    }
}

// ── Copilot ────────────────────────────────────────────────────────────

const MARKER_START: &str = "<!-- AMPLIHACK_CONTEXT_START -->";
const MARKER_END: &str = "<!-- AMPLIHACK_CONTEXT_END -->";
/// 10 MB hard limit.
const MAX_CONTEXT_SIZE: usize = 10 * 1024 * 1024;

/// Injects context into `AGENTS.md` at the repository root, delimited by
/// marker comments so repeated injections replace rather than append.
pub struct CopilotStrategy {
    agents_path: PathBuf,
}

impl CopilotStrategy {
    pub fn new(project_dir: &Path) -> Self {
        Self {
            agents_path: project_dir.join("AGENTS.md"),
        }
    }

    pub fn agents_path(&self) -> &Path {
        &self.agents_path
    }
}

impl HookStrategy for CopilotStrategy {
    fn inject_context(&self, context: &str) -> Result<String> {
        if context.len() > MAX_CONTEXT_SIZE {
            bail!(
                "context exceeds maximum size of {} bytes ({} given)",
                MAX_CONTEXT_SIZE,
                context.len()
            );
        }

        let marked = format!("{MARKER_START}\n{context}\n{MARKER_END}");

        let existing = if self.agents_path.exists() {
            std::fs::read_to_string(&self.agents_path)?
        } else {
            String::new()
        };

        let new_content =
            if let (Some(s), Some(e)) = (existing.find(MARKER_START), existing.find(MARKER_END)) {
                let end = e + MARKER_END.len();
                format!("{}{marked}{}", &existing[..s], &existing[end..])
            } else if existing.is_empty() {
                marked.clone()
            } else {
                format!("{}\n\n{marked}", existing.trim_end())
            };

        std::fs::write(&self.agents_path, &new_content)?;
        Ok(marked)
    }

    fn cleanup(&self) -> Result<()> {
        if !self.agents_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.agents_path)?;
        if let (Some(s), Some(e)) = (content.find(MARKER_START), content.find(MARKER_END)) {
            let end = e + MARKER_END.len();
            let mut cleaned = format!("{}{}", &content[..s], &content[end..]);
            while cleaned.contains("\n\n\n") {
                cleaned = cleaned.replace("\n\n\n", "\n\n");
            }
            let cleaned = cleaned.trim().to_string();
            if cleaned.is_empty() {
                std::fs::remove_file(&self.agents_path)?;
            } else {
                std::fs::write(&self.agents_path, &cleaned)?;
            }
        }
        Ok(())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -- Claude ----------------------------------------------------------

    #[test]
    fn claude_inject_writes_json() {
        let dir = TempDir::new().unwrap();
        let strat = ClaudeStrategy::new(dir.path());
        let result = strat.inject_context("hello world").unwrap();
        assert!(result.contains("hello world"));
        let on_disk = std::fs::read_to_string(strat.context_path()).unwrap();
        let val: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(val["context"], "hello world");
    }

    #[test]
    fn claude_inject_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let strat = ClaudeStrategy::new(dir.path());
        strat.inject_context("x").unwrap();
        assert!(strat.context_path().exists());
    }

    #[test]
    fn claude_cleanup_removes_file() {
        let dir = TempDir::new().unwrap();
        let strat = ClaudeStrategy::new(dir.path());
        strat.inject_context("data").unwrap();
        strat.cleanup().unwrap();
        assert!(!strat.context_path().exists());
    }

    #[test]
    fn claude_cleanup_noop_when_missing() {
        let dir = TempDir::new().unwrap();
        let strat = ClaudeStrategy::new(dir.path());
        strat.cleanup().unwrap();
    }

    #[test]
    fn claude_handle_stop_cleans_up() {
        let dir = TempDir::new().unwrap();
        let strat = ClaudeStrategy::new(dir.path());
        strat.inject_context("ctx").unwrap();
        strat.handle_stop().unwrap();
        assert!(!strat.context_path().exists());
    }

    // -- Copilot ---------------------------------------------------------

    #[test]
    fn copilot_inject_creates_agents_md() {
        let dir = TempDir::new().unwrap();
        let strat = CopilotStrategy::new(dir.path());
        let result = strat.inject_context("my context").unwrap();
        assert!(result.contains(MARKER_START));
        assert!(result.contains(MARKER_END));
        assert!(result.contains("my context"));
        let on_disk = std::fs::read_to_string(strat.agents_path()).unwrap();
        assert!(on_disk.contains("my context"));
    }

    #[test]
    fn copilot_inject_replaces_existing_markers() {
        let dir = TempDir::new().unwrap();
        let strat = CopilotStrategy::new(dir.path());
        strat.inject_context("first").unwrap();
        strat.inject_context("second").unwrap();
        let on_disk = std::fs::read_to_string(strat.agents_path()).unwrap();
        assert!(!on_disk.contains("first"));
        assert!(on_disk.contains("second"));
        // Only one pair of markers
        assert_eq!(on_disk.matches(MARKER_START).count(), 1);
    }

    #[test]
    fn copilot_inject_appends_to_existing_content() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agents\n\nExisting.").unwrap();
        let strat = CopilotStrategy::new(dir.path());
        strat.inject_context("new ctx").unwrap();
        let on_disk = std::fs::read_to_string(strat.agents_path()).unwrap();
        assert!(on_disk.starts_with("# Agents"));
        assert!(on_disk.contains("new ctx"));
    }

    #[test]
    fn copilot_cleanup_removes_markers() {
        let dir = TempDir::new().unwrap();
        let strat = CopilotStrategy::new(dir.path());
        std::fs::write(
            strat.agents_path(),
            format!("# Title\n\n{MARKER_START}\nctx\n{MARKER_END}\n\nFooter"),
        )
        .unwrap();
        strat.cleanup().unwrap();
        let on_disk = std::fs::read_to_string(strat.agents_path()).unwrap();
        assert!(!on_disk.contains(MARKER_START));
        assert!(on_disk.contains("# Title"));
        assert!(on_disk.contains("Footer"));
    }

    #[test]
    fn copilot_cleanup_removes_file_when_only_markers() {
        let dir = TempDir::new().unwrap();
        let strat = CopilotStrategy::new(dir.path());
        strat.inject_context("solo").unwrap();
        strat.cleanup().unwrap();
        assert!(!strat.agents_path().exists());
    }

    #[test]
    fn copilot_cleanup_noop_when_missing() {
        let dir = TempDir::new().unwrap();
        let strat = CopilotStrategy::new(dir.path());
        strat.cleanup().unwrap();
    }

    #[test]
    fn copilot_rejects_oversized_context() {
        let dir = TempDir::new().unwrap();
        let strat = CopilotStrategy::new(dir.path());
        let huge = "x".repeat(MAX_CONTEXT_SIZE + 1);
        assert!(strat.inject_context(&huge).is_err());
    }

    #[test]
    fn copilot_markers_are_present_in_output() {
        let dir = TempDir::new().unwrap();
        let strat = CopilotStrategy::new(dir.path());
        let result = strat.inject_context("payload").unwrap();
        assert!(result.starts_with(MARKER_START));
        assert!(result.ends_with(MARKER_END));
    }
}
