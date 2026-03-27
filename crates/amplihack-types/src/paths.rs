//! Centralized directory layout for the `.claude` runtime tree.
//!
//! All hooks reference paths through `ProjectDirs` instead of
//! ad-hoc `.join(".claude")` chains. Single source of truth.

use std::env;
use std::path::{Path, PathBuf};

/// Sanitize a session ID to prevent path traversal attacks.
///
/// Strips path separators (`/`, `\`) and `..` components, returning a safe
/// string for use in filesystem path construction.
///
/// # Panics
/// Panics if the sanitized result is empty.
pub fn sanitize_session_id(session_id: &str) -> String {
    let sanitized: String = session_id.replace(['/', '\\'], "").replace("..", "");
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
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "amplihack-types-{name}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

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

    #[test]
    fn sanitize_normal_session_id() {
        assert_eq!(
            sanitize_session_id("normal-session-id-123"),
            "normal-session-id-123"
        );
    }

    #[test]
    fn sanitize_strips_path_traversal() {
        assert_eq!(sanitize_session_id("../../../etc/passwd"), "etcpasswd");
    }

    #[test]
    fn sanitize_strips_forward_slashes() {
        assert_eq!(sanitize_session_id("foo/bar"), "foobar");
    }

    #[test]
    fn sanitize_strips_backslashes() {
        assert_eq!(sanitize_session_id("foo\\bar"), "foobar");
    }

    #[test]
    fn sanitize_strips_mixed_traversal() {
        assert_eq!(
            sanitize_session_id("..\\..\\windows\\system32"),
            "windowssystem32"
        );
    }

    #[test]
    #[should_panic(expected = "session_id is empty after sanitization")]
    fn sanitize_rejects_empty_result() {
        sanitize_session_id("../../../");
    }

    #[test]
    fn session_locks_sanitizes_traversal() {
        let dirs = ProjectDirs::new("/project");
        let path = dirs.session_locks("../../../etc/passwd");
        assert_eq!(
            path,
            PathBuf::from("/project/.claude/runtime/locks/etcpasswd")
        );
    }

    #[test]
    fn session_logs_sanitizes_traversal() {
        let dirs = ProjectDirs::new("/project");
        let path = dirs.session_logs("../../../etc/passwd");
        assert_eq!(
            path,
            PathBuf::from("/project/.claude/runtime/logs/etcpasswd")
        );
    }

    #[test]
    fn session_power_steering_sanitizes_traversal() {
        let dirs = ProjectDirs::new("/project");
        let path = dirs.session_power_steering("../../../etc/passwd");
        assert_eq!(
            path,
            PathBuf::from("/project/.claude/runtime/power-steering/etcpasswd")
        );
    }

    #[test]
    fn resolve_framework_file_prefers_src_amplihack_checkout() {
        let dir = temp_test_dir("src-amplihack");
        let project = dir.join("worktree").join("nested");
        let framework = dir.join("worktree").join("src").join("amplihack");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(framework.join(".claude/context")).unwrap();
        fs::write(
            framework.join(".claude/context/USER_PREFERENCES.md"),
            "verbosity = balanced",
        )
        .unwrap();

        let resolved = resolve_framework_file_from(&project, ".claude/context/USER_PREFERENCES.md");

        assert_eq!(
            resolved.as_deref(),
            Some(
                framework
                    .join(".claude/context/USER_PREFERENCES.md")
                    .as_path()
            )
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn resolve_framework_file_uses_amplihack_root_override() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_test_dir("amplihack-root");
        let project = dir.join("project");
        let framework = dir.join("framework-root");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(framework.join(".claude/context")).unwrap();
        fs::write(
            framework.join(".claude/context/USER_PREFERENCES.md"),
            "verbosity = concise",
        )
        .unwrap();
        let previous = env::var_os("AMPLIHACK_ROOT");
        unsafe { env::set_var("AMPLIHACK_ROOT", &framework) };

        let resolved = resolve_framework_file_from(&project, ".claude/context/USER_PREFERENCES.md");

        match previous {
            Some(value) => unsafe { env::set_var("AMPLIHACK_ROOT", value) },
            None => unsafe { env::remove_var("AMPLIHACK_ROOT") },
        }

        assert_eq!(
            resolved.as_deref(),
            Some(
                framework
                    .join(".claude/context/USER_PREFERENCES.md")
                    .as_path()
            )
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn resolve_framework_file_rejects_path_traversal() {
        let dir = temp_test_dir("path-traversal");
        fs::create_dir_all(dir.join(".claude")).unwrap();

        assert!(resolve_framework_file_from(&dir, "../secret").is_none());
        assert!(resolve_framework_file_from(&dir, "/absolute/path").is_none());

        let _ = fs::remove_dir_all(dir);
    }
}
