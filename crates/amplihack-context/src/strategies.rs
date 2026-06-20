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
const AGENTS_TITLE: &str = "# Amplihack Agents";

/// Injects context into `AGENTS.md` at the repository root, delimited by
/// marker comments so repeated injections replace rather than append.
pub struct CopilotStrategy {
    agents_path: PathBuf,
    project_dir: PathBuf,
}

impl CopilotStrategy {
    pub fn new(project_dir: &Path) -> Self {
        Self {
            agents_path: project_dir.join("AGENTS.md"),
            project_dir: project_dir.to_path_buf(),
        }
    }

    pub fn agents_path(&self) -> &Path {
        &self.agents_path
    }

    /// Validate that AGENTS.md path stays inside the project root.
    fn validate_path(&self) -> Result<()> {
        let canonical_project = self
            .project_dir
            .canonicalize()
            .unwrap_or_else(|_| self.project_dir.clone());
        let canonical_agents = if self.agents_path.exists() {
            self.agents_path.canonicalize()?
        } else if let Some(parent) = self.agents_path.parent() {
            let parent_canonical = parent
                .canonicalize()
                .unwrap_or_else(|_| parent.to_path_buf());
            parent_canonical.join(self.agents_path.file_name().unwrap_or_default())
        } else {
            self.agents_path.clone()
        };

        if !canonical_agents.starts_with(&canonical_project) {
            bail!(
                "AGENTS.md path escapes project root: {}",
                canonical_agents.display()
            );
        }
        Ok(())
    }

    /// Format context string as a markdown section with session header.
    fn format_context_markdown(context: &str) -> String {
        format!("## Current Session Context\n\n{context}")
    }

    fn marker_block_bounds(content: &str) -> Result<Option<(usize, usize)>> {
        let start_count = content.matches(MARKER_START).count();
        let end_count = content.matches(MARKER_END).count();
        if start_count == 0 && end_count == 0 {
            return Ok(None);
        }
        if start_count != end_count {
            bail!("AGENTS.md contains a partial Amplihack marker block");
        }
        if start_count > 1 {
            bail!("AGENTS.md contains multiple Amplihack marker blocks; ownership is ambiguous");
        }

        let s = content.find(MARKER_START).expect("counted start marker");
        let e = content.find(MARKER_END).expect("counted end marker");
        if s >= e {
            bail!("AGENTS.md contains misordered Amplihack context markers");
        }
        Ok(Some((s, e)))
    }

    fn try_remove_old_context(content: &str) -> Result<String> {
        if let Some((s, e)) = Self::marker_block_bounds(content)? {
            let end = e + MARKER_END.len();
            let mut cleaned = format!("{}{}", &content[..s], &content[end..]);
            while cleaned.contains("\n\n\n") {
                cleaned = cleaned.replace("\n\n\n", "\n\n");
            }
            Ok(cleaned.trim().to_string())
        } else {
            Ok(content.to_string())
        }
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

        self.validate_path()?;

        let formatted = Self::format_context_markdown(context);
        let marked = format!("{MARKER_START}\n{formatted}\n{MARKER_END}");

        let existing = if self.agents_path.exists() {
            std::fs::read_to_string(&self.agents_path)?
        } else {
            String::new()
        };

        let base = Self::try_remove_old_context(&existing)?;

        let new_content = if base.is_empty() {
            format!("{AGENTS_TITLE}\n\n{marked}")
        } else {
            // Insert after the title line if present
            if let Some(title_end) = base.find('\n') {
                let first_line = &base[..title_end];
                if first_line.starts_with('#') {
                    format!("{first_line}\n\n{marked}{}", &base[title_end..])
                } else {
                    format!("{}\n\n{marked}", base.trim_end())
                }
            } else {
                format!("{}\n\n{marked}", base.trim_end())
            }
        };

        std::fs::write(&self.agents_path, &new_content)?;
        Ok(marked)
    }

    fn cleanup(&self) -> Result<()> {
        if !self.agents_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.agents_path)?;
        let cleaned = Self::try_remove_old_context(&content)?;
        let trimmed = cleaned.trim();
        if trimmed.is_empty() || trimmed == AGENTS_TITLE {
            std::fs::remove_file(&self.agents_path)?;
        } else {
            std::fs::write(&self.agents_path, &cleaned)?;
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
    fn copilot_inject_creates_agents_md_with_title() {
        let dir = TempDir::new().unwrap();
        let strat = CopilotStrategy::new(dir.path());
        let result = strat.inject_context("my context").unwrap();
        assert!(result.contains(MARKER_START));
        assert!(result.contains(MARKER_END));
        assert!(result.contains("my context"));
        let on_disk = std::fs::read_to_string(strat.agents_path()).unwrap();
        assert!(on_disk.starts_with(AGENTS_TITLE));
        assert!(on_disk.contains("## Current Session Context"));
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
        assert_eq!(on_disk.matches(MARKER_START).count(), 1);
    }

    #[test]
    fn copilot_inject_inserts_after_title() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agents\n\nExisting.").unwrap();
        let strat = CopilotStrategy::new(dir.path());
        strat.inject_context("new ctx").unwrap();
        let on_disk = std::fs::read_to_string(strat.agents_path()).unwrap();
        assert!(on_disk.starts_with("# Agents"));
        assert!(on_disk.contains("new ctx"));
        // Context should appear before "Existing."
        let marker_pos = on_disk.find(MARKER_START).unwrap();
        let existing_pos = on_disk.find("Existing.").unwrap();
        assert!(marker_pos < existing_pos);
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

    #[test]
    fn copilot_remove_old_context_helper() {
        let content = format!("before\n{MARKER_START}\nold\n{MARKER_END}\nafter");
        let cleaned = CopilotStrategy::try_remove_old_context(&content).unwrap();
        assert!(!cleaned.contains("old"));
        assert!(cleaned.contains("before"));
        assert!(cleaned.contains("after"));
    }

    #[test]
    fn copilot_remove_old_context_misordered_markers() {
        let content = format!("before\n{MARKER_END}\nstuff\n{MARKER_START}\nafter");
        let err = CopilotStrategy::try_remove_old_context(&content).unwrap_err();
        assert!(
            err.to_string().contains("misordered"),
            "misordered marker error should be explicit: {err:#}"
        );
    }

    #[test]
    fn copilot_inject_replaces_stale_per_tool_context_inside_owned_block() {
        let dir = TempDir::new().unwrap();
        let strat = CopilotStrategy::new(dir.path());
        let stale = format!(
            "# Team Notes\n\nKeep this user-authored line.\n\n{MARKER_START}\n## Copilot Context\nstale per-tool Copilot context\n{MARKER_END}\n\nFooter stays."
        );
        std::fs::write(strat.agents_path(), stale).unwrap();

        strat.inject_context("fresh generated context").unwrap();
        let on_disk = std::fs::read_to_string(strat.agents_path()).unwrap();

        assert!(on_disk.contains("Keep this user-authored line."));
        assert!(on_disk.contains("Footer stays."));
        assert!(on_disk.contains("fresh generated context"));
        assert!(
            !on_disk.contains("stale per-tool Copilot context"),
            "#800 regression: owned marker block must be fully replaced on regeneration"
        );
        assert_eq!(on_disk.matches(MARKER_START).count(), 1);
        assert_eq!(on_disk.matches(MARKER_END).count(), 1);
    }

    #[test]
    fn copilot_inject_rejects_partial_marker_blocks_instead_of_appending() {
        let dir = TempDir::new().unwrap();
        let strat = CopilotStrategy::new(dir.path());
        std::fs::write(
            strat.agents_path(),
            format!("# Agents\n\n{MARKER_START}\nstale generated context without an end marker"),
        )
        .unwrap();

        let err = strat
            .inject_context("fresh context")
            .expect_err("#800 regression: partial marker blocks must fail visibly");
        assert!(
            err.to_string().contains("marker") || err.to_string().contains("AGENTS.md"),
            "partial marker error should explain the AGENTS.md marker problem: {err:#}"
        );
        let on_disk = std::fs::read_to_string(strat.agents_path()).unwrap();
        assert!(
            !on_disk.contains("fresh context"),
            "failed partial-marker injection must not append a second generated block"
        );
    }

    #[test]
    fn copilot_inject_rejects_multiple_owned_blocks_as_ambiguous() {
        let dir = TempDir::new().unwrap();
        let strat = CopilotStrategy::new(dir.path());
        std::fs::write(
            strat.agents_path(),
            format!(
                "# Agents\n\n{MARKER_START}\none\n{MARKER_END}\n\nUser note\n\n{MARKER_START}\ntwo\n{MARKER_END}\n"
            ),
        )
        .unwrap();

        let err = strat
            .inject_context("fresh context")
            .expect_err("#800 regression: multiple owned marker blocks must fail visibly");
        assert!(
            err.to_string().contains("multiple") || err.to_string().contains("ambiguous"),
            "multiple-marker error should explain ambiguity: {err:#}"
        );
    }

    #[test]
    fn copilot_format_context_markdown() {
        let formatted = CopilotStrategy::format_context_markdown("test data");
        assert!(formatted.starts_with("## Current Session Context"));
        assert!(formatted.contains("test data"));
    }
}
