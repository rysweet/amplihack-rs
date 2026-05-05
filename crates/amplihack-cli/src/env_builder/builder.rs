use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::helpers::{
    build_path, find_asset_resolver_binary, generate_session_id, is_file_python_script,
    is_python_amplihack_path, resolve_session_tree_depth, resolve_session_tree_id,
    session_tree_context_present,
};

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

    /// Propagate orchestration tree context without changing depth.
    pub fn with_session_tree_context(self) -> Self {
        if !session_tree_context_present() {
            return self;
        }

        self.set("AMPLIHACK_TREE_ID", resolve_session_tree_id())
            .set("AMPLIHACK_SESSION_DEPTH", resolve_session_tree_depth(false))
            .set(
                "AMPLIHACK_MAX_DEPTH",
                env::var("AMPLIHACK_MAX_DEPTH").unwrap_or_else(|_| "3".to_string()),
            )
            .set(
                "AMPLIHACK_MAX_SESSIONS",
                env::var("AMPLIHACK_MAX_SESSIONS").unwrap_or_else(|_| "10".to_string()),
            )
    }

    /// Propagate orchestration tree context while incrementing child session depth.
    pub fn with_incremented_session_tree_context(self) -> Self {
        if !session_tree_context_present() {
            return self;
        }

        self.set("AMPLIHACK_TREE_ID", resolve_session_tree_id())
            .set("AMPLIHACK_SESSION_DEPTH", resolve_session_tree_depth(true))
            .set(
                "AMPLIHACK_MAX_DEPTH",
                env::var("AMPLIHACK_MAX_DEPTH").unwrap_or_else(|_| "3".to_string()),
            )
            .set(
                "AMPLIHACK_MAX_SESSIONS",
                env::var("AMPLIHACK_MAX_SESSIONS").unwrap_or_else(|_| "10".to_string()),
            )
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
    /// The explicit `project_root` argument is authoritative — it always wins
    /// over any inherited `AMPLIHACK_GRAPH_DB_PATH` / `AMPLIHACK_KUZU_DB_PATH`
    /// in the parent environment (issue #250). The legacy alias is unset so
    /// only the neutral contract propagates forward.
    pub fn with_project_graph_db(self, project_root: &Path) -> Result<Self> {
        debug_assert!(
            project_root.is_absolute(),
            "project_root must be an absolute path; got: {}",
            project_root.display()
        );

        let path = project_root.join(".amplihack").join("graph_db");
        let path = path.to_string_lossy().into_owned();
        Ok(self
            .unset("AMPLIHACK_KUZU_DB_PATH")
            .set("AMPLIHACK_GRAPH_DB_PATH", path))
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

    /// Sanitize the child process environment to prevent re-entry into the
    /// Python amplihack stack.
    ///
    /// When agent subprocesses are spawned by the Rust recipe runner, they may
    /// inherit PATH/PYTHONPATH entries that cause them to pick up the Python
    /// `amplihack` package instead of the Rust binary. This method:
    ///
    /// 1. Removes PYTHONPATH entries containing `amplihack` (but not `amplihack-rs`)
    /// 2. Filters PATH entries that contain a Python `amplihack` script that would
    ///    shadow the Rust binary
    /// 3. Unsets `PYTHONSTARTUP` if it references amplihack
    pub fn with_python_sanitization(mut self) -> Self {
        // Sanitize PYTHONPATH: remove entries referencing Python amplihack
        if let Ok(pythonpath) = env::var("PYTHONPATH") {
            let filtered: Vec<&str> = pythonpath
                .split(':')
                .filter(|entry| !is_python_amplihack_path(entry))
                .collect();
            if filtered.is_empty() {
                self.removed_vars.insert("PYTHONPATH".to_string());
            } else {
                let cleaned = filtered.join(":");
                if cleaned != pythonpath {
                    self = self.set("PYTHONPATH", cleaned);
                }
            }
        }

        // Sanitize PYTHONSTARTUP if it references amplihack
        if let Ok(startup) = env::var("PYTHONSTARTUP")
            && startup.contains("amplihack")
            && !startup.contains("amplihack-rs")
        {
            self.removed_vars.insert("PYTHONSTARTUP".to_string());
        }

        // Filter PATH: remove directories containing a Python amplihack that
        // would shadow the Rust binary. We mark dirs for removal if they contain
        // an `amplihack` file that is a Python script (not an ELF binary).
        if let Ok(path_var) = env::var("PATH") {
            let filtered: Vec<&str> = path_var
                .split(':')
                .filter(|dir| {
                    if dir.is_empty() {
                        return true;
                    }
                    let candidate = Path::new(dir).join("amplihack");
                    if !candidate.is_file() {
                        return true; // no amplihack binary here, keep dir
                    }
                    // Keep the dir if the amplihack binary is NOT a Python script
                    !is_file_python_script(&candidate)
                })
                .collect();
            let cleaned = filtered.join(":");
            if cleaned != path_var {
                self = self.set("PATH", cleaned);
            }
        }

        self
    }

    /// Add standard AMPLIHACK_* variables and NODE_OPTIONS.
    pub fn with_amplihack_vars(self) -> Self {
        // Merge NODE_OPTIONS: append if existing (and not already present), set fresh otherwise
        let ambient_node_options = env::var("NODE_OPTIONS").ok();
        self.with_amplihack_vars_with_node_options(ambient_node_options.as_deref())
    }

    /// Add standard AMPLIHACK_* variables with an explicit NODE_OPTIONS value.
    pub fn with_amplihack_vars_with_node_options(self, node_options: Option<&str>) -> Self {
        let max_old_space = "--max-old-space-size=32768";
        let node_opts_value = match node_options {
            Some(existing) if !existing.is_empty() => {
                if existing.contains("--max-old-space-size=") {
                    // Already has a --max-old-space-size setting, don't duplicate
                    existing.to_string()
                } else {
                    format!("{existing} {max_old_space}")
                }
            }
            Some(_) => String::new(),
            _ => max_old_space.to_string(),
        };

        self.set("AMPLIHACK_RUST_RUNTIME", "1")
            .set("AMPLIHACK_VERSION", crate::VERSION)
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
        let EnvBuilder { removed_vars, .. } = &self;
        for key in removed_vars {
            command.env_remove(key);
        }
        command.envs(self.build());
    }
}

impl Default for EnvBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_builder_is_empty() {
        let env = EnvBuilder::new().build();
        assert!(env.is_empty());
    }

    #[test]
    fn set_adds_variable() {
        let env = EnvBuilder::new().set("KEY", "value").build();
        assert_eq!(env.get("KEY").unwrap(), "value");
    }

    #[test]
    fn set_overwrites_previous() {
        let env = EnvBuilder::new()
            .set("KEY", "old")
            .set("KEY", "new")
            .build();
        assert_eq!(env.get("KEY").unwrap(), "new");
    }

    #[test]
    fn unset_removes_variable() {
        let env = EnvBuilder::new()
            .set("KEY", "value")
            .unset("KEY")
            .build();
        assert!(!env.contains_key("KEY"));
    }

    #[test]
    fn set_if_true_adds() {
        let env = EnvBuilder::new().set_if(true, "KEY", "value").build();
        assert_eq!(env.get("KEY").unwrap(), "value");
    }

    #[test]
    fn set_if_false_skips() {
        let env = EnvBuilder::new().set_if(false, "KEY", "value").build();
        assert!(!env.contains_key("KEY"));
    }

    #[test]
    fn prepend_path_adds_to_path() {
        let env = EnvBuilder::new()
            .prepend_path("/custom/bin")
            .build();
        let path = env.get("PATH").unwrap();
        assert!(path.starts_with("/custom/bin"));
    }

    #[test]
    fn multiple_prepend_path() {
        let env = EnvBuilder::new()
            .prepend_path("/first")
            .prepend_path("/second")
            .build();
        let path = env.get("PATH").unwrap();
        assert!(path.contains("/first"));
        assert!(path.contains("/second"));
    }

    #[test]
    fn with_agent_binary_sets_var() {
        let env = EnvBuilder::new().with_agent_binary("copilot").build();
        assert_eq!(env.get("AMPLIHACK_AGENT_BINARY").unwrap(), "copilot");
    }

    #[test]
    fn default_matches_new() {
        let from_new = EnvBuilder::new().build();
        let from_default = EnvBuilder::default().build();
        assert_eq!(from_new, from_default);
    }

    #[test]
    fn chaining_works() {
        let env = EnvBuilder::new()
            .set("A", "1")
            .set("B", "2")
            .unset("A")
            .set("C", "3")
            .build();
        assert!(!env.contains_key("A"));
        assert_eq!(env.get("B").unwrap(), "2");
        assert_eq!(env.get("C").unwrap(), "3");
    }
}
