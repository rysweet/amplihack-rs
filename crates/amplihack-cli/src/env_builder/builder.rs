use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use super::helpers::{
    build_path, find_asset_resolver_binary, generate_session_id, resolve_session_tree_depth,
    resolve_session_tree_id, session_tree_context_present,
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
    /// Prefer an existing backend-neutral `AMPLIHACK_GRAPH_DB_PATH`, then accept
    /// a legacy `AMPLIHACK_KUZU_DB_PATH` as input-only compatibility. Child
    /// processes receive `AMPLIHACK_GRAPH_DB_PATH` and have the legacy alias
    /// explicitly removed so the neutral contract is what propagates forward.
    pub fn with_project_graph_db(self, project_root: &Path) -> Result<Self> {
        debug_assert!(
            project_root.is_absolute(),
            "project_root must be an absolute path; got: {}",
            project_root.display()
        );
        fn validate_graph_db_path(candidate: &str, env_var: &str) -> Result<String> {
            let path = Path::new(candidate);
            if !path.is_absolute() {
                bail!(
                    "invalid {env_var} override: graph DB path must be absolute: {}",
                    path.display()
                );
            }
            if path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
            {
                bail!(
                    "invalid {env_var} override: graph DB path must not contain parent traversal: {}",
                    path.display()
                );
            }
            for blocked in [Path::new("/proc"), Path::new("/sys"), Path::new("/dev")] {
                if path.starts_with(blocked) {
                    bail!(
                        "invalid {env_var} override: graph DB path uses blocked prefix {}: {}",
                        blocked.display(),
                        path.display()
                    );
                }
            }
            Ok(candidate.to_string())
        }

        if let Ok(existing) = env::var("AMPLIHACK_GRAPH_DB_PATH")
            && !existing.is_empty()
        {
            let existing = validate_graph_db_path(&existing, "AMPLIHACK_GRAPH_DB_PATH")?;
            return Ok(self
                .unset("AMPLIHACK_KUZU_DB_PATH")
                .set("AMPLIHACK_GRAPH_DB_PATH", existing));
        }
        if let Ok(existing) = env::var("AMPLIHACK_KUZU_DB_PATH")
            && !existing.is_empty()
        {
            let existing = validate_graph_db_path(&existing, "AMPLIHACK_KUZU_DB_PATH")?;
            return Ok(self
                .unset("AMPLIHACK_KUZU_DB_PATH")
                .set("AMPLIHACK_GRAPH_DB_PATH", existing));
        }

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
