//! Centralized directory layout for the `.claude` runtime tree.
//!
//! All hooks reference paths through `ProjectDirs` instead of
//! ad-hoc `.join(".claude")` chains. Single source of truth.

use std::env;
use std::path::{Path, PathBuf};

/// Sanitize a session ID to prevent path traversal and metadata injection.
///
/// Replaces any character that is not alphanumeric, hyphen, or underscore
/// with an underscore.  Mirrors the Python `_sanitize_session_id()`.
///
/// # Panics
/// Panics if the sanitized result is empty.
pub fn sanitize_session_id(session_id: &str) -> String {
    let sanitized: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    assert!(
        !sanitized.is_empty(),
        "session_id is empty after sanitization (original: {session_id:?})"
    );
    sanitized
}

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
    ///
    /// The session ID is sanitized to prevent path traversal.
    pub fn session_locks(&self, session_id: &str) -> PathBuf {
        self.locks.join(sanitize_session_id(session_id))
    }

    /// Log directory for a specific session.
    ///
    /// The session ID is sanitized to prevent path traversal.
    pub fn session_logs(&self, session_id: &str) -> PathBuf {
        self.logs.join(sanitize_session_id(session_id))
    }

    /// Power steering directory for a specific session.
    ///
    /// The session ID is sanitized to prevent path traversal.
    pub fn session_power_steering(&self, session_id: &str) -> PathBuf {
        self.power_steering.join(sanitize_session_id(session_id))
    }

    /// The `.lock_active` sentinel file.
    pub fn lock_active_file(&self) -> PathBuf {
        self.locks.join(".lock_active")
    }

    /// The `.continuation_prompt` file.
    pub fn continuation_prompt_file(&self) -> PathBuf {
        self.locks.join(".continuation_prompt")
    }

    /// The `.lock_goal` file.
    pub fn lock_goal_file(&self) -> PathBuf {
        self.locks.join(".lock_goal")
    }

    /// The `.lock_message` file.
    pub fn lock_message_file(&self) -> PathBuf {
        self.locks.join(".lock_message")
    }

    /// The persisted launcher context file.
    pub fn launcher_context_file(&self) -> PathBuf {
        self.runtime.join("launcher_context.json")
    }

    /// The launcher session lifecycle log.
    pub fn sessions_log_file(&self) -> PathBuf {
        self.runtime.join("sessions.jsonl")
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

    /// Resolve a framework-owned file from deterministic search roots.
    pub fn resolve_framework_file(&self, relative_path: &str) -> Option<PathBuf> {
        resolve_framework_file_from(&self.root, relative_path)
    }

    /// Resolve the framework-owned USER_PREFERENCES.md file, if present.
    pub fn resolve_preferences_file(&self) -> Option<PathBuf> {
        self.resolve_framework_file(".claude/context/USER_PREFERENCES.md")
    }

    /// Resolve the framework-owned default workflow file, if present.
    pub fn resolve_workflow_file(&self) -> Option<PathBuf> {
        self.resolve_framework_file(".claude/workflow/DEFAULT_WORKFLOW.md")
    }
}

fn is_framework_root(path: &Path) -> bool {
    path.join(".claude").is_dir()
}

fn push_framework_root(roots: &mut Vec<PathBuf>, candidate: PathBuf) {
    if is_framework_root(&candidate) && !roots.iter().any(|existing| existing == &candidate) {
        roots.push(candidate);
    }
}

pub fn framework_roots_from(start: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut current = Some(start);

    while let Some(path) = current {
        push_framework_root(&mut roots, path.to_path_buf());
        push_framework_root(&mut roots, path.join("src").join("amplihack"));
        current = path.parent();
    }

    if let Some(root) = env::var_os("AMPLIHACK_ROOT").map(PathBuf::from) {
        push_framework_root(&mut roots, root);
    }

    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        push_framework_root(&mut roots, home.join(".amplihack"));
    }

    roots
}

pub fn resolve_framework_file_from(start: &Path, relative_path: &str) -> Option<PathBuf> {
    if relative_path.contains("..")
        || relative_path.contains('\0')
        || Path::new(relative_path).is_absolute()
    {
        return None;
    }

    for root in framework_roots_from(start) {
        let candidate = root.join(relative_path);
        if !candidate.exists() {
            continue;
        }

        let Ok(resolved_root) = root.canonicalize() else {
            continue;
        };
        let Ok(resolved_file) = candidate.canonicalize() else {
            continue;
        };

        if resolved_file.starts_with(&resolved_root) {
            return Some(resolved_file);
        }
    }

    None
}

#[cfg(test)]
#[path = "tests_paths.rs"]
mod tests;
