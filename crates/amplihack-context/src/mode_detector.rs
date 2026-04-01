use std::path::{Path, PathBuf};

/// Installation mode for the amplihack Claude integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeMode {
    /// `.claude/` exists in the project with all essential directories.
    Local,
    /// `~/.amplihack/.claude/` contains a plugin manifest.
    Plugin,
    /// No installation detected.
    None,
}

/// Essential sub-directories that must all exist under `.claude/` for a
/// local installation to be recognised.
const ESSENTIAL_DIRS: &[&str] = &["agents", "commands", "skills", "tools"];

/// Detects the amplihack installation mode for a project.
pub struct ModeDetector;

impl ModeDetector {
    /// Detect mode with precedence: env-override → Local → Plugin → None.
    pub fn detect(project_dir: &Path) -> ClaudeMode {
        if let Ok(mode) = std::env::var("AMPLIHACK_MODE") {
            match mode.to_lowercase().as_str() {
                "local" => return ClaudeMode::Local,
                "plugin" => return ClaudeMode::Plugin,
                "none" => return ClaudeMode::None,
                other => tracing::warn!("unknown AMPLIHACK_MODE value: {other}"),
            }
        }

        if Self::has_local_installation(project_dir) {
            ClaudeMode::Local
        } else if Self::has_plugin_installation() {
            ClaudeMode::Plugin
        } else {
            ClaudeMode::None
        }
    }

    /// Check whether `project_dir/.claude/` exists with all essential
    /// sub-directories.
    pub fn has_local_installation(project_dir: &Path) -> bool {
        let claude_dir = project_dir.join(".claude");
        claude_dir.is_dir()
            && ESSENTIAL_DIRS
                .iter()
                .all(|d| claude_dir.join(d).is_dir())
    }

    /// Check whether `~/.amplihack/.claude/` contains a plugin manifest.
    pub fn has_plugin_installation() -> bool {
        home_dir()
            .map(|h| {
                let plugin = h.join(".amplihack/.claude");
                plugin.is_dir() && plugin.join("plugin_manifest.json").exists()
            })
            .unwrap_or(false)
    }

    /// Return the `.claude` directory for a given mode, if any.
    pub fn get_claude_dir(mode: &ClaudeMode, project_dir: &Path) -> Option<PathBuf> {
        match mode {
            ClaudeMode::Local => Some(project_dir.join(".claude")),
            ClaudeMode::Plugin => home_dir().map(|h| h.join(".amplihack/.claude")),
            ClaudeMode::None => None,
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_local_installation(base: &Path) {
        let claude = base.join(".claude");
        for d in ESSENTIAL_DIRS {
            std::fs::create_dir_all(claude.join(d)).unwrap();
        }
    }

    #[test]
    fn detect_none_when_empty() {
        let dir = TempDir::new().unwrap();
        // Ensure env override is not set for this test
        let mode = ModeDetector::detect(dir.path());
        // May be None or Plugin depending on host; at minimum it's not Local
        assert_ne!(mode, ClaudeMode::Local);
    }

    #[test]
    fn detect_local_when_essential_dirs_present() {
        let dir = TempDir::new().unwrap();
        create_local_installation(dir.path());
        // Temporarily override to avoid plugin detection interfering
        unsafe { std::env::remove_var("AMPLIHACK_MODE") };
        let mode = ModeDetector::detect(dir.path());
        assert_eq!(mode, ClaudeMode::Local);
    }

    #[test]
    fn has_local_false_without_claude_dir() {
        let dir = TempDir::new().unwrap();
        assert!(!ModeDetector::has_local_installation(dir.path()));
    }

    #[test]
    fn has_local_false_with_partial_dirs() {
        let dir = TempDir::new().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(claude.join("agents")).unwrap();
        std::fs::create_dir_all(claude.join("commands")).unwrap();
        // Missing skills and tools
        assert!(!ModeDetector::has_local_installation(dir.path()));
    }

    #[test]
    fn has_local_true_with_all_dirs() {
        let dir = TempDir::new().unwrap();
        create_local_installation(dir.path());
        assert!(ModeDetector::has_local_installation(dir.path()));
    }

    #[test]
    fn env_override_local() {
        let dir = TempDir::new().unwrap();
        unsafe { std::env::set_var("AMPLIHACK_MODE", "local") };
        let mode = ModeDetector::detect(dir.path());
        unsafe { std::env::remove_var("AMPLIHACK_MODE") };
        assert_eq!(mode, ClaudeMode::Local);
    }

    #[test]
    fn env_override_plugin() {
        let dir = TempDir::new().unwrap();
        unsafe { std::env::set_var("AMPLIHACK_MODE", "plugin") };
        let mode = ModeDetector::detect(dir.path());
        unsafe { std::env::remove_var("AMPLIHACK_MODE") };
        assert_eq!(mode, ClaudeMode::Plugin);
    }

    #[test]
    fn env_override_none() {
        let dir = TempDir::new().unwrap();
        create_local_installation(dir.path());
        unsafe { std::env::set_var("AMPLIHACK_MODE", "none") };
        let mode = ModeDetector::detect(dir.path());
        unsafe { std::env::remove_var("AMPLIHACK_MODE") };
        assert_eq!(mode, ClaudeMode::None);
    }

    #[test]
    fn env_override_unknown_falls_through() {
        let dir = TempDir::new().unwrap();
        unsafe { std::env::set_var("AMPLIHACK_MODE", "bogus") };
        // Should fall through to normal detection (not Local)
        let mode = ModeDetector::detect(dir.path());
        unsafe { std::env::remove_var("AMPLIHACK_MODE") };
        assert_ne!(mode, ClaudeMode::Local);
    }

    #[test]
    fn get_claude_dir_local() {
        let dir = TempDir::new().unwrap();
        let result = ModeDetector::get_claude_dir(&ClaudeMode::Local, dir.path());
        assert_eq!(result, Some(dir.path().join(".claude")));
    }

    #[test]
    fn get_claude_dir_none() {
        let dir = TempDir::new().unwrap();
        assert_eq!(ModeDetector::get_claude_dir(&ClaudeMode::None, dir.path()), None);
    }

    #[test]
    fn get_claude_dir_plugin() {
        let result = ModeDetector::get_claude_dir(&ClaudeMode::Plugin, Path::new("/unused"));
        // Should be Some(HOME/.amplihack/.claude) when HOME is set
        if std::env::var("HOME").is_ok() {
            assert!(result.is_some());
        }
    }
}
