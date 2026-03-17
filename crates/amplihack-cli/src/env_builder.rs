//! Type-safe environment builder for launching child processes.
//!
//! Constructs the environment variables needed by launched tools,
//! including AMPLIHACK_* vars and PATH augmentation. Uses set-based
//! PATH deduplication instead of error-prone substring matching.

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// Builder for constructing the environment passed to child processes.
#[derive(Debug)]
pub struct EnvBuilder {
    vars: HashMap<String, String>,
    removed_vars: HashSet<String>,
    path_prepend: Vec<PathBuf>,
}

impl EnvBuilder {
    pub fn new() -> Self {
        Self {
            vars: HashMap::new(),
            removed_vars: HashSet::new(),
            path_prepend: Vec::new(),
        }
    }

    /// Set a specific environment variable.
    pub fn set(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.vars.insert(key.into(), value.into());
        self
    }

    /// Remove a specific environment variable from child processes.
    pub fn unset(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        self.vars.remove(&key);
        self.removed_vars.insert(key);
        self
    }

    /// Prepend a directory to PATH (deduplicated).
    pub fn prepend_path(mut self, dir: impl Into<PathBuf>) -> Self {
        self.path_prepend.push(dir.into());
        self
    }

    /// Add AMPLIHACK_SESSION_ID (generate a new one if not already set).
    pub fn with_amplihack_session_id(self) -> Self {
        let session_id = env::var("AMPLIHACK_SESSION_ID").unwrap_or_else(|_| generate_session_id());
        // Pass depth through unchanged (matching Python behavior)
        let depth = env::var("AMPLIHACK_DEPTH").unwrap_or_else(|_| "1".to_string());

        self.set("AMPLIHACK_SESSION_ID", session_id)
            .set("AMPLIHACK_DEPTH", depth)
    }

    /// Conditionally set an environment variable.
    ///
    /// If `condition` is `false` this is a no-op and `self` is returned unchanged.
    /// Used by callers to propagate flags (e.g. `AMPLIHACK_NONINTERACTIVE`) only
    /// when the corresponding condition holds at the call site.
    pub fn set_if(self, condition: bool, key: impl Into<String>, value: impl Into<String>) -> Self {
        if condition {
            self.set(key, value)
        } else {
            self
        }
    }

    /// Set `AMPLIHACK_AGENT_BINARY` to the name of the CLI binary being launched.
    ///
    /// Downstream consumers (recipe runner, hooks) use this to determine which
    /// agent binary to invoke. The value must be one of the four known tool names:
    /// `claude`, `copilot`, `codex`, or `amplifier`.
    ///
    /// # Security (SEC-WS1-01)
    ///
    /// A `debug_assert!` validates the value in debug and test builds. The check
    /// is compiled out in release builds — callers are responsible for passing a
    /// valid tool name (controlled by `Commands` dispatch in `launch.rs`).
    pub fn with_agent_binary(self, tool: impl Into<String>) -> Self {
        let tool = tool.into();
        debug_assert!(
            matches!(tool.as_str(), "claude" | "copilot" | "codex" | "amplifier"),
            "AMPLIHACK_AGENT_BINARY must be one of: claude, copilot, codex, amplifier; got: {tool}"
        );
        self.set("AMPLIHACK_AGENT_BINARY", tool)
    }

    /// Set the backend-neutral code-graph DB path for child processes.
    ///
    /// Prefer an existing backend-neutral `AMPLIHACK_GRAPH_DB_PATH`, then accept
    /// a legacy `AMPLIHACK_KUZU_DB_PATH` as input-only compatibility. Child
    /// processes receive `AMPLIHACK_GRAPH_DB_PATH` and have the legacy alias
    /// explicitly removed so the neutral contract is what propagates forward.
    pub fn with_project_graph_db(self, project_root: &Path) -> Self {
        debug_assert!(
            project_root.is_absolute(),
            "project_root must be an absolute path; got: {}",
            project_root.display()
        );
        fn validate_graph_db_path(candidate: &str, env_var: &str) -> Option<String> {
            let path = Path::new(candidate);
            if !path.is_absolute() {
                tracing::warn!(
                    env_var,
                    path = %path.display(),
                    "ignoring non-absolute graph DB override"
                );
                return None;
            }
            if path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
            {
                tracing::warn!(
                    env_var,
                    path = %path.display(),
                    "ignoring graph DB override with parent traversal"
                );
                return None;
            }
            for blocked in [Path::new("/proc"), Path::new("/sys"), Path::new("/dev")] {
                if path.starts_with(blocked) {
                    tracing::warn!(
                        env_var,
                        path = %path.display(),
                        blocked = %blocked.display(),
                        "ignoring graph DB override with unsafe path prefix"
                    );
                    return None;
                }
            }
            Some(candidate.to_string())
        }

        if let Ok(existing) = env::var("AMPLIHACK_GRAPH_DB_PATH")
            && !existing.is_empty()
            && let Some(existing) = validate_graph_db_path(&existing, "AMPLIHACK_GRAPH_DB_PATH")
        {
            return self
                .unset("AMPLIHACK_KUZU_DB_PATH")
                .set("AMPLIHACK_GRAPH_DB_PATH", existing);
        }
        if let Ok(existing) = env::var("AMPLIHACK_KUZU_DB_PATH")
            && !existing.is_empty()
            && let Some(existing) = validate_graph_db_path(&existing, "AMPLIHACK_KUZU_DB_PATH")
        {
            return self
                .unset("AMPLIHACK_KUZU_DB_PATH")
                .set("AMPLIHACK_GRAPH_DB_PATH", existing);
        }

        let path = project_root.join(".amplihack").join("graph_db");
        let path = path.to_string_lossy().into_owned();
        self.unset("AMPLIHACK_KUZU_DB_PATH")
            .set("AMPLIHACK_GRAPH_DB_PATH", path)
    }

    /// Set the project-local Kuzu database path.
    ///
    /// # Deprecation
    ///
    /// This method is a compatibility shim. Use [`with_project_graph_db`] instead.
    /// The backend-neutral `AMPLIHACK_GRAPH_DB_PATH` variable is now the canonical
    /// contract; this method delegates directly to `with_project_graph_db`.
    #[deprecated(since = "0.7.0", note = "use with_project_graph_db instead")]
    pub fn with_project_kuzu_db(self, project_root: &Path) -> Self {
        self.with_project_graph_db(project_root)
    }

    /// Resolve and set `AMPLIHACK_HOME` in the child environment.
    ///
    /// Resolution order:
    /// 1. If `AMPLIHACK_HOME` is already set in the current environment → no-op.
    /// 2. If `HOME` is set → use `$HOME/.amplihack`.
    /// 3. If `std::env::current_exe()` succeeds → use the parent directory of
    ///    the running executable.
    /// 4. All attempts fail → return `self` unchanged (silent degradation).
    ///
    /// # Security (SEC-WS3-01/02/03)
    ///
    /// Paths containing `..` (parent directory) components are rejected with a
    /// `tracing::warn!` and the variable is NOT set. This prevents an attacker
    /// who controls `$HOME` from injecting traversal paths such as
    /// `/tmp/../../etc`. Non-absolute paths are also rejected.
    ///
    /// Note: existence of the path is NOT checked — the consumer (recipe runner)
    /// creates it on demand. Existence checks are avoided to keep this free of
    /// filesystem side-effects and to work correctly in test environments.
    pub fn with_amplihack_home(self) -> Self {
        use std::path::{Component, Path, PathBuf};

        /// Validate a candidate path and return it as a String if safe,
        /// or `None` if it fails security checks (SEC-WS3-01/02).
        fn validate_path(candidate: &Path) -> Option<String> {
            // SEC-WS3-02: must be absolute.
            if !candidate.is_absolute() {
                tracing::warn!(
                    path = %candidate.display(),
                    "AMPLIHACK_HOME resolution produced a non-absolute path — skipping"
                );
                return None;
            }
            // SEC-WS3-01: reject any path containing '..' (ParentDir) components.
            if candidate.components().any(|c| c == Component::ParentDir) {
                tracing::warn!(
                    path = %candidate.display(),
                    "AMPLIHACK_HOME resolution produced a path with '..' components — skipping (SEC-WS3-01)"
                );
                return None;
            }
            Some(candidate.to_string_lossy().into_owned())
        }

        // Step 1: already set in environment — preserve it.
        if let Ok(existing) = env::var("AMPLIHACK_HOME")
            && !existing.is_empty()
        {
            return self.set("AMPLIHACK_HOME", existing);
        }

        // Step 2: derive from HOME env var → $HOME/.amplihack
        if let Ok(home) = env::var("HOME")
            && !home.is_empty()
        {
            let candidate = PathBuf::from(&home).join(".amplihack");
            if let Some(value) = validate_path(&candidate) {
                return self.set("AMPLIHACK_HOME", value);
            }
            // Path failed security check — do NOT fall through to exe-based
            // strategy, as a poisoned HOME should not silently resolve to
            // the binary directory (which may be controlled by the attacker).
            return self;
        }

        // Step 3: fall back to the parent directory of the running executable.
        if let Ok(exe) = env::current_exe()
            && let Some(parent) = exe.parent()
        {
            let candidate = parent.to_path_buf();
            if let Some(value) = validate_path(&candidate) {
                return self.set("AMPLIHACK_HOME", value);
            }
        }

        // Step 4: all strategies exhausted — return unchanged (SEC-WS3-03 silent).
        self
    }

    /// Resolve and set `AMPLIHACK_ASSET_RESOLVER` in the child environment.
    ///
    /// Resolution order:
    /// 1. Preserve a pre-existing `AMPLIHACK_ASSET_RESOLVER`
    /// 2. Sibling binary next to the running executable
    /// 3. `amplihack-asset-resolver` on PATH
    /// 4. `~/.local/bin/amplihack-asset-resolver`
    /// 5. `~/.cargo/bin/amplihack-asset-resolver`
    pub fn with_asset_resolver(self) -> Self {
        if let Ok(existing) = env::var("AMPLIHACK_ASSET_RESOLVER")
            && !existing.is_empty()
        {
            return self.set("AMPLIHACK_ASSET_RESOLVER", existing);
        }

        if let Some(path) = find_asset_resolver_binary() {
            return self.set(
                "AMPLIHACK_ASSET_RESOLVER",
                path.to_string_lossy().into_owned(),
            );
        }

        self
    }

    /// Add standard AMPLIHACK_* variables and NODE_OPTIONS.
    pub fn with_amplihack_vars(self) -> Self {
        // Merge NODE_OPTIONS: append if existing (and not already present), set fresh otherwise
        let max_old_space = "--max-old-space-size=32768";
        let node_opts_value = match env::var("NODE_OPTIONS") {
            Ok(existing) if !existing.is_empty() => {
                if existing.contains("--max-old-space-size=") {
                    // Already has a --max-old-space-size setting, don't duplicate
                    existing
                } else {
                    format!("{existing} {max_old_space}")
                }
            }
            _ => max_old_space.to_string(),
        };

        self.set("AMPLIHACK_RUST_RUNTIME", "1")
            .set("AMPLIHACK_VERSION", env!("CARGO_PKG_VERSION"))
            .set("NODE_OPTIONS", node_opts_value)
    }

    /// Build the final environment as key-value pairs.
    ///
    /// The returned map includes only the variables explicitly set via this builder,
    /// plus the augmented PATH. The child process inherits the rest from the parent.
    pub fn build(self) -> HashMap<String, String> {
        let mut result = self.vars;

        // Build augmented PATH
        if !self.path_prepend.is_empty() {
            let current_path = env::var("PATH").unwrap_or_default();
            let new_path = build_path(&self.path_prepend, &current_path);
            result.insert("PATH".to_string(), new_path);
        }

        result
    }

    /// Apply the builder's overrides and removals to a child process command.
    pub fn apply_to_command(self, command: &mut Command) {
        let removed_vars = self.removed_vars.clone();
        let vars = self.build();
        for key in removed_vars {
            command.env_remove(key);
        }
        command.envs(vars);
    }
}

impl Default for EnvBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn find_asset_resolver_binary() -> Option<PathBuf> {
    if let Ok(exe) = env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let sibling = parent.join("amplihack-asset-resolver");
        if sibling.is_file() {
            return Some(sibling);
        }
    }

    if let Ok(path) = env::var("PATH") {
        for dir in env::split_paths(&path) {
            let candidate = dir.join("amplihack-asset-resolver");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    if let Ok(home) = env::var("HOME") {
        for suffix in [".local/bin", ".cargo/bin"] {
            let candidate = PathBuf::from(&home)
                .join(suffix)
                .join("amplihack-asset-resolver");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Build a PATH string by prepending directories and deduplicating.
fn build_path(prepend: &[PathBuf], current: &str) -> String {
    let mut seen = HashSet::new();
    let mut parts = Vec::new();

    // Prepend entries first (higher priority)
    for dir in prepend {
        let s = dir.to_string_lossy().to_string();
        if seen.insert(s.clone()) {
            parts.push(s);
        }
    }

    // Then existing PATH entries
    for entry in env::split_paths(current) {
        let s = entry.to_string_lossy().to_string();
        if seen.insert(s.clone()) {
            parts.push(s);
        }
    }

    env::join_paths(parts.iter().map(|s| s.as_str()))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Generate a simple session ID (timestamp + PID).
fn generate_session_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("rs-{}-{}", ts, std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::cwd_env_lock;

    // ── WS1: with_agent_binary ────────────────────────────────────────────────

    /// WS1-1: with_agent_binary must insert AMPLIHACK_AGENT_BINARY for each
    /// supported tool name.
    #[test]
    fn with_agent_binary_sets_env_var_for_all_tools() {
        for tool in &["claude", "copilot", "codex", "amplifier"] {
            let env = EnvBuilder::new().with_agent_binary(*tool).build();
            assert_eq!(
                env.get("AMPLIHACK_AGENT_BINARY").map(String::as_str),
                Some(*tool),
                "AMPLIHACK_AGENT_BINARY should be '{tool}'"
            );
        }
    }

    #[test]
    fn with_project_graph_db_sets_project_local_path() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

        let env = EnvBuilder::new().with_project_graph_db(temp.path()).build();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let expected = temp.path().join(".amplihack").join("graph_db");
        assert_eq!(
            env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
            Some(expected.to_str().unwrap())
        );
        assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
    }

    #[test]
    fn with_project_graph_db_preserves_existing_override() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/custom/kuzu") };

        let env = EnvBuilder::new().with_project_graph_db(temp.path()).build();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(
            env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
            Some("/custom/kuzu")
        );
        assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
    }

    /// I77-ENV-GRAPH-ONLY: When only AMPLIHACK_GRAPH_DB_PATH is set,
    /// with_project_graph_db() must preserve the backend-neutral name and avoid
    /// re-emitting the legacy Kuzu alias into child process overrides.
    #[test]
    fn with_project_graph_db_preserves_graph_db_env_without_emitting_legacy_alias() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/custom/graph-only") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

        let env = EnvBuilder::new().with_project_graph_db(temp.path()).build();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(
            env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
            Some("/custom/graph-only"),
            "AMPLIHACK_GRAPH_DB_PATH must be preserved from the process environment"
        );
        assert_eq!(
            env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str),
            None,
            "AMPLIHACK_KUZU_DB_PATH must not be re-emitted into child overrides"
        );
    }

    #[test]
    fn with_project_graph_db_prefers_backend_neutral_override() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/custom/graph") };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/custom/kuzu") };

        let env = EnvBuilder::new().with_project_graph_db(temp.path()).build();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(
            env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
            Some("/custom/graph")
        );
        assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
    }

    #[test]
    fn with_project_graph_db_rejects_relative_graph_override() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "relative/graph_db") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

        let env = EnvBuilder::new().with_project_graph_db(temp.path()).build();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let expected = temp.path().join(".amplihack").join("graph_db");
        assert_eq!(
            env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
            Some(expected.to_str().unwrap())
        );
        assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
    }

    #[test]
    fn with_project_graph_db_rejects_proc_prefixed_graph_override() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/proc/1/mem") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

        let env = EnvBuilder::new().with_project_graph_db(temp.path()).build();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let expected = temp.path().join(".amplihack").join("graph_db");
        assert_eq!(
            env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
            Some(expected.to_str().unwrap())
        );
        assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
    }

    #[test]
    fn with_project_graph_db_uses_valid_kuzu_override_when_graph_override_is_invalid() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/tmp/../etc/shadow") };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/custom/kuzu") };

        let env = EnvBuilder::new().with_project_graph_db(temp.path()).build();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(
            env.get("AMPLIHACK_GRAPH_DB_PATH").map(String::as_str),
            Some("/custom/kuzu")
        );
        assert_eq!(env.get("AMPLIHACK_KUZU_DB_PATH").map(String::as_str), None);
    }

    #[test]
    fn apply_to_command_translates_kuzu_alias_to_graph_db_path_and_removes_kuzu_var() {
        // When only AMPLIHACK_KUZU_DB_PATH is set (legacy alias), apply_to_command must:
        //   1. Translate the value to AMPLIHACK_GRAPH_DB_PATH (backend-neutral name)
        //   2. Explicitly remove AMPLIHACK_KUZU_DB_PATH so child processes receive
        //      only the backend-neutral contract and cannot observe the legacy alias.
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/inherited/legacy") };

        let mut cmd = Command::new("true");
        EnvBuilder::new()
            .with_project_graph_db(temp.path())
            .apply_to_command(&mut cmd);

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let envs: HashMap<_, _> = cmd
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        // KUZU_DB_PATH="/inherited/legacy" is valid → translated to GRAPH_DB_PATH
        assert_eq!(
            envs.get("AMPLIHACK_GRAPH_DB_PATH")
                .and_then(|value| value.as_deref()),
            Some("/inherited/legacy"),
            "Legacy KUZU_DB_PATH must be translated to GRAPH_DB_PATH in child env"
        );
        assert_eq!(
            envs.get("AMPLIHACK_KUZU_DB_PATH")
                .and_then(|value| value.as_deref()),
            None,
            "Command must explicitly remove inherited AMPLIHACK_KUZU_DB_PATH"
        );
    }

    // ── WS3: with_amplihack_home ───────────────────────────────────────────────

    /// WS3-1: with_amplihack_home should derive AMPLIHACK_HOME from HOME when
    /// AMPLIHACK_HOME is not set.
    #[test]
    fn with_amplihack_home_sets_from_home() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        let temp = tempfile::tempdir().unwrap();
        let prev_home = crate::test_support::set_home(temp.path());
        let prev_amplihack_home = std::env::var_os("AMPLIHACK_HOME");
        unsafe { std::env::remove_var("AMPLIHACK_HOME") };

        let env = EnvBuilder::new().with_amplihack_home().build();

        crate::test_support::restore_home(prev_home);
        match prev_amplihack_home {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
        }

        let expected = temp.path().join(".amplihack");
        assert_eq!(
            env.get("AMPLIHACK_HOME").map(String::as_str),
            Some(expected.to_str().unwrap()),
            "AMPLIHACK_HOME should be <HOME>/.amplihack when unset"
        );
    }

    /// WS3-2: with_amplihack_home must not overwrite an AMPLIHACK_HOME that is
    /// already set in the process environment.
    #[test]
    fn with_amplihack_home_does_not_overwrite_existing() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        let custom = "/custom/path";
        let prev = std::env::var_os("AMPLIHACK_HOME");
        unsafe { std::env::set_var("AMPLIHACK_HOME", custom) };

        let env = EnvBuilder::new().with_amplihack_home().build();

        match prev {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
        }

        assert_eq!(
            env.get("AMPLIHACK_HOME").map(String::as_str),
            Some(custom),
            "with_amplihack_home must preserve a pre-existing AMPLIHACK_HOME"
        );
    }

    /// WS3-3 (SEC-WS3-01): with_amplihack_home must reject a HOME that contains
    /// path traversal components (e.g. "..") and must NOT set AMPLIHACK_HOME.
    #[test]
    fn with_amplihack_home_rejects_traversal_path() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        let prev_home = crate::test_support::set_home(std::path::Path::new("/tmp/../../etc"));
        let prev_amplihack_home = std::env::var_os("AMPLIHACK_HOME");
        unsafe { std::env::remove_var("AMPLIHACK_HOME") };

        let env = EnvBuilder::new().with_amplihack_home().build();

        crate::test_support::restore_home(prev_home);
        match prev_amplihack_home {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
        }

        assert!(
            !env.contains_key("AMPLIHACK_HOME"),
            "with_amplihack_home must not set AMPLIHACK_HOME when HOME contains path traversal"
        );
    }

    // ── WS2: set_if ────────────────────────────────────────────────────────────

    /// WS2-2a: set_if must insert the key-value pair when condition is true.
    #[test]
    fn set_if_sets_when_condition_true() {
        let env = EnvBuilder::new().set_if(true, "MY_KEY", "MY_VALUE").build();
        assert_eq!(
            env.get("MY_KEY").map(String::as_str),
            Some("MY_VALUE"),
            "set_if(true, ...) must insert the entry"
        );
    }

    /// WS2-2b: set_if must NOT insert the key-value pair when condition is false.
    #[test]
    fn set_if_skips_when_condition_false() {
        let env = EnvBuilder::new()
            .set_if(false, "MY_KEY", "MY_VALUE")
            .build();
        assert!(
            !env.contains_key("MY_KEY"),
            "set_if(false, ...) must not insert the entry"
        );
    }

    // ── Existing tests ─────────────────────────────────────────────────────────

    #[test]
    fn empty_builder_produces_empty_map() {
        let env = EnvBuilder::new().build();
        assert!(env.is_empty());
    }

    #[test]
    fn set_adds_variable() {
        let env = EnvBuilder::new()
            .set("FOO", "bar")
            .set("BAZ", "qux")
            .build();
        assert_eq!(env.get("FOO").unwrap(), "bar");
        assert_eq!(env.get("BAZ").unwrap(), "qux");
    }

    #[test]
    fn prepend_path_deduplicates() {
        let env = EnvBuilder::new()
            .prepend_path("/opt/bin")
            .prepend_path("/opt/bin") // duplicate
            .build();

        let path = env.get("PATH").unwrap();
        let count = path.matches("/opt/bin").count();
        assert_eq!(count, 1, "PATH should not contain duplicates");
    }

    #[test]
    fn with_amplihack_session_id_sets_vars() {
        let env = EnvBuilder::new().with_amplihack_session_id().build();
        assert!(env.contains_key("AMPLIHACK_SESSION_ID"));
        assert!(env.contains_key("AMPLIHACK_DEPTH"));
    }

    #[test]
    fn with_amplihack_vars_sets_runtime_flag() {
        let env = EnvBuilder::new().with_amplihack_vars().build();
        assert_eq!(env.get("AMPLIHACK_RUST_RUNTIME").unwrap(), "1");
        assert!(env.contains_key("AMPLIHACK_VERSION"));
    }

    #[test]
    fn with_asset_resolver_sets_from_path() {
        let temp = tempfile::tempdir().unwrap();
        let resolver = temp.path().join("amplihack-asset-resolver");
        std::fs::write(&resolver, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&resolver, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let prev_path = env::var_os("PATH");
        let prev_resolver = env::var_os("AMPLIHACK_ASSET_RESOLVER");
        unsafe {
            env::set_var("PATH", temp.path());
            env::remove_var("AMPLIHACK_ASSET_RESOLVER");
        }

        let built = EnvBuilder::new().with_asset_resolver().build();

        match prev_path {
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        match prev_resolver {
            Some(value) => unsafe { env::set_var("AMPLIHACK_ASSET_RESOLVER", value) },
            None => unsafe { env::remove_var("AMPLIHACK_ASSET_RESOLVER") },
        }

        assert_eq!(
            built.get("AMPLIHACK_ASSET_RESOLVER").map(String::as_str),
            Some(resolver.to_str().unwrap())
        );
    }

    #[test]
    fn generate_session_id_format() {
        let id = generate_session_id();
        assert!(
            id.starts_with("rs-"),
            "session ID should start with 'rs-': {id}"
        );
    }

    #[test]
    fn build_path_preserves_order() {
        let path = build_path(
            &[PathBuf::from("/first"), PathBuf::from("/second")],
            "/third:/fourth",
        );
        let parts: Vec<&str> = path.split(':').collect();
        assert_eq!(parts[0], "/first");
        assert_eq!(parts[1], "/second");
        assert_eq!(parts[2], "/third");
        assert_eq!(parts[3], "/fourth");
    }
}
