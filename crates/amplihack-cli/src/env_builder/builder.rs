use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use super::helpers::{
    build_path, find_asset_resolver_binary, generate_session_id, is_file_python_script,
    is_python_amplihack_path, resolve_session_tree_depth, resolve_session_tree_id,
    session_tree_context_present,
};

/// `AMPLIHACK_AGENT_BINARY` and its tag, which only the agent-binary methods
/// export, so that every export says what its value is.
fn is_agent_binary_variable(key: &str) -> bool {
    use amplihack_utils::agent_binary::{BINARY_ENV, SOURCE_ENV};
    key == BINARY_ENV || key == SOURCE_ENV
}

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
    ///
    /// Not for `AMPLIHACK_AGENT_BINARY` or its `AMPLIHACK_AGENT_BINARY_SOURCE`
    /// tag. Export those through [`EnvBuilder::with_launched_agent_binary`] or
    /// [`EnvBuilder::with_resolved_agent_binary`], which make the caller say
    /// what the value is. A debug assertion enforces this in debug and test
    /// builds.
    pub fn set(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let key = key.into();
        debug_assert!(
            !is_agent_binary_variable(&key),
            "export {key} through with_launched_agent_binary or with_resolved_agent_binary, \
             which tag it with what it is"
        );
        self.vars.insert(key, value.into());
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

    /// Export `tool` as the binary of the launcher that is starting this child,
    /// tagged as describing that session, and remove every other CLI's session
    /// markers from the child.
    ///
    /// Issue #1481: a launcher started on an inherited default guess naming
    /// itself hands the guess on still tagged, so no launcher nested below it
    /// persists the guess. Any other launch is tagged `session:<tool>`, which
    /// ranks with the session markers instead of above them (crusty review of
    /// #1490 at baaafb18). See [`super::launch_binary_source`].
    ///
    /// The other CLIs' markers have to go for that ranking to hold. A Copilot
    /// session launched from a Claude Code shell would otherwise inherit
    /// `CLAUDECODE`, which outranks `session:copilot`, and resolve to claude.
    pub fn with_launched_agent_binary(self, tool: &str) -> Self {
        self.with_launched_agent_binary_from(tool, &|key| std::env::var(key).ok())
    }

    /// [`EnvBuilder::with_launched_agent_binary`] reading the inherited
    /// environment through `var`.
    pub fn with_launched_agent_binary_from(
        self,
        tool: &str,
        var: &dyn Fn(&str) -> Option<String>,
    ) -> Self {
        let this = self.with_resolved_agent_binary(tool, super::launch_binary_source(tool, var));
        amplihack_utils::agent_binary::other_clis_markers(tool)
            .fold(this, |builder, marker| builder.unset(marker))
    }

    /// Export a binary the resolver chose, together with where it came from.
    ///
    /// With [`EnvBuilder::with_launched_agent_binary`], this is the only way
    /// to export `AMPLIHACK_AGENT_BINARY`; [`EnvBuilder::set`] refuses it. No
    /// method exports a bare name, so every caller says what its value is.
    /// The public `with_agent_binary` exported one untagged while its doc
    /// described a launch, and the fleet reasoner used it to start `claude`
    /// (crusty review of #1490 at 960eaacb). A caller starting a CLI wants
    /// [`EnvBuilder::with_launched_agent_binary`].
    ///
    /// Only [`ResolutionSource::Env`], the user's own `AMPLIHACK_AGENT_BINARY`,
    /// is exported as an instruction, untagged, and it removes any tag the
    /// child would otherwise inherit. Issue #1481: a value from the built-in
    /// default is tagged `default:<tool>`, so that no descendant treats it as
    /// an instruction or persists it as a session's choice. A value from a
    /// session marker or a launcher context is tagged `session:<tool>`, so a
    /// descendant ranks it with its own session markers: a step that starts a
    /// tmux server must not hand this run's answer to every later session on
    /// it as an instruction (crusty review of #1490 at baaafb18). Either way a
    /// descendant that later sets a different binary is still obeyed. See
    /// [`amplihack_utils::agent_binary::export_tag`].
    ///
    /// # Security (SEC-WS1-01)
    ///
    /// `tool` must be one of `claude`, `copilot`, `codex` or `amplifier`. A
    /// `debug_assert!` checks this in debug and test builds. It is compiled out
    /// in release builds, so callers pass a resolver answer or a launcher's
    /// own name, never free text.
    ///
    /// [`ResolutionSource::Env`]: amplihack_utils::agent_binary::ResolutionSource::Env
    pub fn with_resolved_agent_binary(
        mut self,
        tool: impl Into<String>,
        source: amplihack_utils::agent_binary::ResolutionSource,
    ) -> Self {
        use amplihack_utils::agent_binary::{BINARY_ENV, SOURCE_ENV, export_tag};
        let tool = tool.into();
        debug_assert!(
            matches!(tool.as_str(), "claude" | "copilot" | "codex" | "amplifier"),
            "AMPLIHACK_AGENT_BINARY must be one of: claude, copilot, codex, amplifier; got: {tool}"
        );
        let tag = export_tag(&tool, source);
        self.vars.insert(BINARY_ENV.to_string(), tool);
        match tag {
            Some(tag) => {
                self.vars.insert(SOURCE_ENV.to_string(), tag);
                self
            }
            // An instruction: an inherited tag no longer describes the value.
            None => self.unset(SOURCE_ENV),
        }
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
    /// 2. Walk up from the current directory for an `amplifier-bundle/` root.
    /// 3. If `HOME` is set → use `$HOME/.amplihack`.
    /// 4. If `std::env::current_exe()` succeeds → use the parent directory of
    ///    the running executable.
    /// 5. All attempts fail → return `self` unchanged (silent degradation).
    ///
    /// # Security (SEC-WS3-01/02/03)
    ///
    /// Paths containing `..` (parent directory) components are rejected with a
    /// `tracing::warn!` and the variable is NOT set. This prevents an attacker
    /// who controls `$HOME` from injecting traversal paths such as
    /// `/tmp/../../etc`. Non-absolute paths are also rejected.
    ///
    /// Note: `$HOME/.amplihack` and executable-parent fallbacks are not
    /// existence-checked — the consumer can create them on demand. The repo
    /// root path is selected only when an `amplifier-bundle/` marker exists.
    pub fn with_amplihack_home(self) -> Self {
        match env::current_dir() {
            Ok(cwd) => self.with_amplihack_home_from(&cwd),
            Err(_) => self.resolve_amplihack_home(None),
        }
    }

    /// Resolve and set `AMPLIHACK_HOME` using `start_dir` as the authoritative
    /// bundle-root search anchor before falling back to the user install.
    pub fn with_amplihack_home_from(self, start_dir: &Path) -> Self {
        self.resolve_amplihack_home(Some(start_dir))
    }

    fn resolve_amplihack_home(self, start_dir: Option<&Path>) -> Self {
        // Step 1: already set in environment — preserve it.
        if let Ok(existing) = env::var("AMPLIHACK_HOME")
            && !existing.is_empty()
        {
            return self.set("AMPLIHACK_HOME", existing);
        }

        // Step 2: prefer the checked-out bundle root when running from a repo
        // or worktree so recipe subprocesses resolve assets from the same code.
        if let Some(start_dir) = start_dir
            && let Some(root) = find_bundle_root(start_dir)
            && let Some(value) = validate_amplihack_home_path(&root)
        {
            return self.set("AMPLIHACK_HOME", value);
        }

        // Step 3: derive from HOME env var → $HOME/.amplihack
        if let Ok(home) = env::var("HOME")
            && !home.is_empty()
        {
            let candidate = PathBuf::from(&home).join(".amplihack");
            if let Some(value) = validate_amplihack_home_path(&candidate) {
                return self.set("AMPLIHACK_HOME", value);
            }
            // Path failed security check — do NOT fall through to exe-based
            // strategy, as a poisoned HOME should not silently resolve to
            // the binary directory (which may be controlled by the attacker).
            return self;
        }

        // Step 4: fall back to the parent directory of the running executable.
        if let Ok(exe) = env::current_exe()
            && let Some(parent) = exe.parent()
        {
            let candidate = parent.to_path_buf();
            if let Some(value) = validate_amplihack_home_path(&candidate) {
                return self.set("AMPLIHACK_HOME", value);
            }
        }

        // Step 5: all strategies exhausted — return unchanged (SEC-WS3-03 silent).
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

    /// Prevent child git/gh/less invocations from blocking on interactive pagers.
    pub fn with_pager_safe_defaults(self) -> Self {
        self.set("GIT_PAGER", "cat")
            .set("GH_PAGER", "cat")
            .set("PAGER", "cat")
            .set("LESS", "FRX")
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

fn find_bundle_root(start_dir: &Path) -> Option<PathBuf> {
    let start = if start_dir.is_absolute() {
        start_dir.to_path_buf()
    } else {
        env::current_dir().ok()?.join(start_dir)
    };
    let start = start.canonicalize().unwrap_or(start);

    start
        .ancestors()
        .find(|candidate| candidate.join("amplifier-bundle").is_dir())
        .map(Path::to_path_buf)
}

fn validate_amplihack_home_path(candidate: &Path) -> Option<String> {
    if !candidate.is_absolute() {
        tracing::warn!(
            path = %candidate.display(),
            "AMPLIHACK_HOME resolution produced a non-absolute path — skipping"
        );
        return None;
    }
    if candidate.components().any(|c| c == Component::ParentDir) {
        tracing::warn!(
            path = %candidate.display(),
            "AMPLIHACK_HOME resolution produced a path with '..' components — skipping (SEC-WS3-01)"
        );
        return None;
    }
    Some(candidate.to_string_lossy().into_owned())
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
        let env = EnvBuilder::new().set("KEY", "value").unset("KEY").build();
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
        let env = EnvBuilder::new().prepend_path("/custom/bin").build();
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

    /// Crusty review of #1490 at 960eaacb: the generic setter cannot export
    /// the agent binary or its tag, so no caller can hand a bare name on as an
    /// instruction without saying that is what it is.
    #[test]
    #[cfg(debug_assertions)]
    fn set_refuses_the_agent_binary_and_its_tag() {
        use amplihack_utils::agent_binary::{BINARY_ENV, SOURCE_ENV};
        for key in [BINARY_ENV, SOURCE_ENV] {
            let refused = std::panic::catch_unwind(|| EnvBuilder::new().set(key, "claude"));
            assert!(refused.is_err(), "set({key}) must be refused");
        }
    }

    /// Issue #1481 and crusty review of #1490 at baaafb18: only an explicit
    /// answer is exported untagged, as an instruction, and it clears a tag
    /// that would otherwise be inherited. A guess is tagged as one, and an
    /// answer from a session marker or a launcher context as a description of
    /// a session.
    #[test]
    fn resolved_agent_binary_is_untagged_only_for_an_explicit_answer() {
        use amplihack_utils::agent_binary::{ResolutionSource, SOURCE_ENV};

        for (source, tag) in [
            (ResolutionSource::Default, "default:claude"),
            (ResolutionSource::SessionMarker, "session:claude"),
            (ResolutionSource::LauncherContext, "session:claude"),
        ] {
            let env = EnvBuilder::new()
                .with_resolved_agent_binary("claude", source)
                .build();
            assert_eq!(env.get("AMPLIHACK_AGENT_BINARY").unwrap(), "claude");
            assert_eq!(env.get(SOURCE_ENV).map(String::as_str), Some(tag));
        }

        let builder = EnvBuilder::new().with_resolved_agent_binary("claude", ResolutionSource::Env);
        assert!(
            builder.removed_vars.contains(SOURCE_ENV),
            "an inherited tag must be removed from the child"
        );
        let env = builder.build();
        assert_eq!(env.get("AMPLIHACK_AGENT_BINARY").unwrap(), "claude");
        assert!(!env.contains_key(SOURCE_ENV), "an instruction is untagged");
    }

    /// A launcher tags its own name as a session description and removes
    /// every other CLI's markers from its child, so that a Copilot session
    /// started from a Claude Code shell is not taken for a Claude one.
    #[test]
    fn a_launcher_describes_its_session_and_strips_other_clis_markers() {
        use amplihack_utils::agent_binary::{SESSION_MARKERS, SOURCE_ENV};

        let nothing_inherited = |_: &str| None;
        for tool in ["claude", "copilot", "codex", "amplifier"] {
            let builder =
                EnvBuilder::new().with_launched_agent_binary_from(tool, &nothing_inherited);
            for &(marker, implies) in SESSION_MARKERS {
                assert_eq!(
                    builder.removed_vars.contains(marker),
                    implies != tool,
                    "{tool}: {marker}"
                );
            }
            let env = builder.build();
            assert_eq!(
                env.get("AMPLIHACK_AGENT_BINARY").map(String::as_str),
                Some(tool)
            );
            assert_eq!(
                env.get(SOURCE_ENV).cloned(),
                Some(format!("session:{tool}"))
            );
        }
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

    #[test]
    fn pager_safe_defaults_are_set() {
        let env = EnvBuilder::new().with_pager_safe_defaults().build();
        assert_eq!(env.get("GIT_PAGER").map(String::as_str), Some("cat"));
        assert_eq!(env.get("GH_PAGER").map(String::as_str), Some("cat"));
        assert_eq!(env.get("PAGER").map(String::as_str), Some("cat"));
        assert_eq!(env.get("LESS").map(String::as_str), Some("FRX"));
    }
}
