//! Centralized directory layout for the `.claude` runtime tree.
//!
//! All hooks reference paths through `ProjectDirs` instead of
//! ad-hoc `.join(".claude")` chains. Single source of truth.

use std::path::{Path, PathBuf};

/// Directory layout rooted at a project directory.
///
/// Every path that hooks touch is defined here. To change
/// the directory structure, edit this struct — not 20 call sites.
#[derive(Debug, Clone)]
pub struct ProjectDirs {
    /// Project root (CWD or explicit).
    pub root: PathBuf,
    /// `.claude/` configuration directory.
    pub claude: PathBuf,
    /// `.claude/runtime/` for ephemeral state.
    pub runtime: PathBuf,
    /// `.claude/runtime/locks/` for lock files.
    pub locks: PathBuf,
    /// `.claude/runtime/metrics/` for tool metrics.
    pub metrics: PathBuf,
    /// `.claude/runtime/logs/` for session logs.
    pub logs: PathBuf,
    /// `.claude/runtime/power-steering/` for power steering state.
    pub power_steering: PathBuf,
    /// `.claude/context/` for user preferences, project context.
    pub context: PathBuf,
    /// `.claude/tools/amplihack/` for hook config files.
    pub tools_amplihack: PathBuf,
}

impl ProjectDirs {
    /// Build all paths from a project root.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let claude = root.join(".claude");
        let runtime = claude.join("runtime");
        Self {
            locks: runtime.join("locks"),
            metrics: runtime.join("metrics"),
            logs: runtime.join("logs"),
            power_steering: runtime.join("power-steering"),
            context: claude.join("context"),
            tools_amplihack: claude.join("tools").join("amplihack"),
            runtime,
            claude,
            root,
        }
    }

    /// Lock file for a specific session.
    pub fn session_locks(&self, session_id: &str) -> PathBuf {
        self.locks.join(session_id)
    }

    /// Log directory for a specific session.
    pub fn session_logs(&self, session_id: &str) -> PathBuf {
        self.logs.join(session_id)
    }

    /// Power steering directory for a specific session.
    pub fn session_power_steering(&self, session_id: &str) -> PathBuf {
        self.power_steering.join(session_id)
    }

    /// The `.lock_active` sentinel file.
    pub fn lock_active_file(&self) -> PathBuf {
        self.locks.join(".lock_active")
    }

    /// The `.continuation_prompt` file.
    pub fn continuation_prompt_file(&self) -> PathBuf {
        self.locks.join(".continuation_prompt")
    }

    /// The `.version` file.
    pub fn version_file(&self) -> PathBuf {
        self.claude.join(".version")
    }

    /// Power steering config file.
    pub fn power_steering_config(&self) -> PathBuf {
        self.tools_amplihack.join(".power_steering_config")
    }

    /// USER_PREFERENCES.md (context dir).
    pub fn user_preferences(&self) -> PathBuf {
        self.context.join("USER_PREFERENCES.md")
    }

    /// PROJECT.md (context dir).
    pub fn project_context(&self) -> PathBuf {
        self.context.join("PROJECT.md")
    }

    /// AMPLIHACK.md in .claude/.
    pub fn amplihack_md(&self) -> PathBuf {
        self.claude.join("AMPLIHACK.md")
    }

    /// CLAUDE.md at project root.
    pub fn claude_md(&self) -> PathBuf {
        self.root.join("CLAUDE.md")
    }

    /// Build from current working directory (convenience).
    pub fn from_cwd() -> Self {
        let root = std::env::current_dir().unwrap_or_else(|e| {
            tracing::warn!("Failed to get CWD ({}), falling back to '.'", e);
            PathBuf::from(".")
        });
        Self::new(root)
    }

    /// Global settings.json at `~/.claude/settings.json`.
    pub fn global_settings() -> Option<PathBuf> {
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".claude").join("settings.json"))
    }

    /// Build from explicit root path (preferred for testability).
    pub fn from_root(root: &Path) -> Self {
        Self::new(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_consistent() {
        let dirs = ProjectDirs::new("/project");
        assert_eq!(dirs.claude, PathBuf::from("/project/.claude"));
        assert_eq!(dirs.runtime, PathBuf::from("/project/.claude/runtime"));
        assert_eq!(dirs.locks, PathBuf::from("/project/.claude/runtime/locks"));
        assert_eq!(
            dirs.metrics,
            PathBuf::from("/project/.claude/runtime/metrics")
        );
        assert_eq!(
            dirs.lock_active_file(),
            PathBuf::from("/project/.claude/runtime/locks/.lock_active")
        );
    }

    #[test]
    fn session_paths() {
        let dirs = ProjectDirs::new("/project");
        assert_eq!(
            dirs.session_locks("abc"),
            PathBuf::from("/project/.claude/runtime/locks/abc")
        );
        assert_eq!(
            dirs.session_logs("abc"),
            PathBuf::from("/project/.claude/runtime/logs/abc")
        );
    }
}
