use std::path::{Path, PathBuf};

use anyhow::Result;

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

// CopilotStrategy lived here and has been removed (issues #1333, #1350).
//
// It constructed `project_dir.join("AGENTS.md")` and injected content between
// `AMPLIHACK_CONTEXT_START` / `_END` markers. Issue #862 removed its call sites
// and left the machinery compiling, with a public constructor and a full test
// suite, in a crate with no dependents.
//
// That artifact caused a real outage. A tracked repo-root AGENTS.md carrying the
// routing prompt reached every git worktree the workflow creates, and Copilot CLI
// loads such a file unconditionally as a custom instruction -- so every leaf agent
// step, including ones already inside an orchestration, was told to start another
// one. A default-workflow run spent 2h47m and produced zero commits.
//
// #1346 removed the file and added a source-tree guard. A guard cannot see a file
// written at runtime inside a worktree, so the writer had to go too: re-wire it,
// deliberately or by accident, and the outage returns with the suite still green.
//
// If context injection is wanted again, it needs a destination that is not an
// auto-loaded instruction channel, and provenance gating so it cannot reach an
// agent-authored step. Do not restore this from history.

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
}
