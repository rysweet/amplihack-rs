//! Copilot hook strategy.
//!
//! Injects context by writing a dynamic context file to disk and updating
//! `AGENTS.md` with an `@include` directive. Power-steering is dispatched
//! via the `gh copilot` CLI subprocess.

use super::base::HookStrategy;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::{fs, io::Write};
use tracing::{debug, info, warn};

/// Hook strategy for the GitHub Copilot launcher.
pub struct CopilotStrategy {
    /// Project root (used to resolve runtime and .github paths).
    project_root: PathBuf,
}

/// Include directive injected into AGENTS.md.
const INCLUDE_DIRECTIVE: &str = "@include runtime/copilot/dynamic_context.md";

impl CopilotStrategy {
    /// Create a new Copilot strategy rooted at `project_root`.
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
        }
    }

    /// Path to the dynamic context markdown file.
    fn dynamic_context_path(&self) -> PathBuf {
        self.project_root
            .join("runtime")
            .join("copilot")
            .join("dynamic_context.md")
    }

    /// Path to `.github/agents/AGENTS.md`.
    fn agents_md_path(&self) -> PathBuf {
        self.project_root
            .join(".github")
            .join("agents")
            .join("AGENTS.md")
    }

    /// Write the dynamic context file.
    fn write_dynamic_context(&self, context: &str) -> Result<()> {
        let path = self.dynamic_context_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, context).with_context(|| format!("writing {}", path.display()))?;
        debug!("wrote dynamic context to {}", path.display());
        Ok(())
    }

    /// Ensure `AGENTS.md` contains the `@include` directive.
    fn ensure_agents_include(&self) -> Result<()> {
        let path = self.agents_md_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }

        let existing = fs::read_to_string(&path).unwrap_or_default();
        if existing.contains(INCLUDE_DIRECTIVE) {
            debug!("AGENTS.md already contains include directive");
            return Ok(());
        }

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;

        writeln!(file)?;
        writeln!(file, "{INCLUDE_DIRECTIVE}")?;
        info!("added include directive to {}", path.display());
        Ok(())
    }
}

impl HookStrategy for CopilotStrategy {
    fn inject_context(&self, context: &str) -> Result<HashMap<String, Value>> {
        self.write_dynamic_context(context)?;
        self.ensure_agents_include()?;

        let mut map = HashMap::new();
        map.insert(
            "dynamicContextPath".to_string(),
            Value::String(self.dynamic_context_path().display().to_string()),
        );
        Ok(map)
    }

    /// Dispatch power-steering via `gh copilot`.
    fn power_steer(&self, prompt: &str, session_id: &str) -> Result<bool> {
        info!(session_id, "power-steering via gh copilot");

        let output = std::process::Command::new("gh")
            .arg("copilot")
            .arg("--session")
            .arg(session_id)
            .arg("--prompt")
            .arg(prompt)
            .current_dir(&self.project_root)
            .output()
            .context("failed to launch gh copilot")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("gh copilot exited {}: {}", output.status, stderr);
            return Ok(false);
        }

        debug!("gh copilot succeeded");
        Ok(true)
    }

    fn get_launcher_name(&self) -> &'static str {
        "copilot"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, CopilotStrategy) {
        let dir = tempfile::tempdir().unwrap();
        let strategy = CopilotStrategy::new(dir.path());
        (dir, strategy)
    }

    #[test]
    fn inject_context_writes_file() {
        let (_dir, strategy) = setup();
        let map = strategy.inject_context("hello copilot").unwrap();

        let content = fs::read_to_string(strategy.dynamic_context_path()).unwrap();
        assert_eq!(content, "hello copilot");
        assert!(map.contains_key("dynamicContextPath"));
    }

    #[test]
    fn inject_context_creates_agents_include() {
        let (_dir, strategy) = setup();
        strategy.inject_context("ctx").unwrap();

        let agents = fs::read_to_string(strategy.agents_md_path()).unwrap();
        assert!(agents.contains(INCLUDE_DIRECTIVE));
    }

    #[test]
    fn idempotent_include_directive() {
        let (_dir, strategy) = setup();
        strategy.inject_context("first").unwrap();
        strategy.inject_context("second").unwrap();

        let agents = fs::read_to_string(strategy.agents_md_path()).unwrap();
        let count = agents.matches(INCLUDE_DIRECTIVE).count();
        assert_eq!(count, 1, "include directive should appear exactly once");
    }

    #[test]
    fn launcher_name() {
        let (_dir, strategy) = setup();
        assert_eq!(strategy.get_launcher_name(), "copilot");
    }

    #[test]
    fn dynamic_context_path_structure() {
        let (dir, strategy) = setup();
        let expected = dir
            .path()
            .join("runtime")
            .join("copilot")
            .join("dynamic_context.md");
        assert_eq!(strategy.dynamic_context_path(), expected);
    }
}
