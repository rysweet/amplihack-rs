//! Type-safe environment builder for launching child processes.
//!
//! Constructs the environment variables needed by launched tools,
//! including AMPLIHACK_* vars and PATH augmentation. Uses set-based
//! PATH deduplication instead of error-prone substring matching.

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Component, PathBuf};

/// Characters that are not allowed in the AMPLIHACK_AGENT_BINARY value.
///
/// Rejects path separators and shell metacharacters to prevent injection.
const AGENT_BINARY_FORBIDDEN: &[char] = &['/', '\\', ';', '|', '&', '`', '$', '(', ')'];

/// Builder for constructing the environment passed to child processes.
#[derive(Debug)]
pub struct EnvBuilder {
    vars: HashMap<String, String>,
    path_prepend: Vec<PathBuf>,
}

impl EnvBuilder {
    pub fn new() -> Self {
        Self {
            vars: HashMap::new(),
            path_prepend: Vec::new(),
        }
    }

    /// Set a specific environment variable.
    pub fn set(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.vars.insert(key.into(), value.into());
        self
    }

    /// Set an environment variable only when `condition` is true.
    ///
    /// If the key is already present in the builder this call is a no-op even
    /// when the condition is true.
    pub fn set_if(self, condition: bool, key: impl Into<String>, value: impl Into<String>) -> Self {
        if !condition {
            return self;
        }
        let k = key.into();
        if self.vars.contains_key(&k) {
            return self;
        }
        self.set(k, value)
    }

    /// Propagate `AMPLIHACK_AGENT_BINARY` from the parent environment.
    ///
    /// Empty values and values containing path separators or shell
    /// metacharacters are silently skipped.  This is a hardening measure —
    /// the variable is user-controlled and must not allow injection.
    pub fn with_agent_binary(self) -> Self {
        match env::var("AMPLIHACK_AGENT_BINARY") {
            Ok(val) if !val.is_empty() => {
                if val.contains(AGENT_BINARY_FORBIDDEN) {
                    tracing::warn!(
                        value = %val,
                        "AMPLIHACK_AGENT_BINARY contains forbidden characters; ignoring"
                    );
                    return self;
                }
                self.set("AMPLIHACK_AGENT_BINARY", val)
            }
            _ => self,
        }
    }

    /// Propagate `AMPLIHACK_HOME` from the parent environment.
    ///
    /// Lexical guards applied:
    /// - Empty values are silently ignored.
    /// - Relative paths are rejected.
    /// - Paths containing `..` components are rejected.
    /// - `fs::canonicalize` is attempted; on first-run the directory may not
    ///   yet exist, so canonicalize failure is treated as an accepted risk and
    ///   the raw (validated) path is used.  ACCEPTED RISK: pre-creation
    ///   symlink traversal is not blocked in the first-run case.
    pub fn with_amplihack_home(self) -> Self {
        let raw = match env::var("AMPLIHACK_HOME") {
            Ok(v) if !v.is_empty() => v,
            _ => return self,
        };

        let path = PathBuf::from(&raw);

        if path.is_relative() {
            tracing::warn!(
                value = %raw,
                "AMPLIHACK_HOME must be an absolute path; ignoring"
            );
            return self;
        }

        if path.components().any(|c| c == Component::ParentDir) {
            tracing::warn!(
                value = %raw,
                "AMPLIHACK_HOME contains '..' components; ignoring"
            );
            return self;
        }

        // ACCEPTED RISK: if the directory does not yet exist (first run),
        // canonicalize will fail and we fall back to the validated raw path.
        let resolved = std::fs::canonicalize(&path).unwrap_or(path);
        self.set("AMPLIHACK_HOME", resolved.to_string_lossy().to_string())
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
}

impl Default for EnvBuilder {
    fn default() -> Self {
        Self::new()
    }
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

    // ── WS1: set_if ───────────────────────────────────────────────────────────

    #[test]
    fn set_if_true_sets_var() {
        let env = EnvBuilder::new().set_if(true, "MY_VAR", "hello").build();
        assert_eq!(env.get("MY_VAR").map(String::as_str), Some("hello"));
    }

    #[test]
    fn set_if_false_skips_var() {
        let env = EnvBuilder::new().set_if(false, "MY_VAR", "hello").build();
        assert!(!env.contains_key("MY_VAR"));
    }

    #[test]
    fn set_if_does_not_overwrite_existing_key() {
        let env = EnvBuilder::new()
            .set("MY_VAR", "original")
            .set_if(true, "MY_VAR", "override")
            .build();
        assert_eq!(env.get("MY_VAR").map(String::as_str), Some("original"));
    }

    // ── WS1: with_agent_binary ────────────────────────────────────────────────

    #[test]
    fn with_agent_binary_reads_env() {
        // SAFETY: test-only env mutation, not run in parallel with other env tests.
        unsafe { std::env::set_var("AMPLIHACK_AGENT_BINARY", "my-agent") };
        let env = EnvBuilder::new().with_agent_binary().build();
        unsafe { std::env::remove_var("AMPLIHACK_AGENT_BINARY") };
        assert_eq!(
            env.get("AMPLIHACK_AGENT_BINARY").map(String::as_str),
            Some("my-agent")
        );
    }

    #[test]
    fn with_agent_binary_skips_empty() {
        unsafe { std::env::set_var("AMPLIHACK_AGENT_BINARY", "") };
        let env = EnvBuilder::new().with_agent_binary().build();
        unsafe { std::env::remove_var("AMPLIHACK_AGENT_BINARY") };
        assert!(!env.contains_key("AMPLIHACK_AGENT_BINARY"));
    }

    #[test]
    fn with_agent_binary_rejects_path_separators() {
        unsafe { std::env::set_var("AMPLIHACK_AGENT_BINARY", "/usr/bin/evil") };
        let env = EnvBuilder::new().with_agent_binary().build();
        unsafe { std::env::remove_var("AMPLIHACK_AGENT_BINARY") };
        assert!(!env.contains_key("AMPLIHACK_AGENT_BINARY"));
    }

    #[test]
    fn with_agent_binary_rejects_shell_metacharacters() {
        for bad in &["agent;rm", "agent|evil", "$(cmd)", "`backtick`"] {
            unsafe { std::env::set_var("AMPLIHACK_AGENT_BINARY", *bad) };
            let env = EnvBuilder::new().with_agent_binary().build();
            unsafe { std::env::remove_var("AMPLIHACK_AGENT_BINARY") };
            assert!(
                !env.contains_key("AMPLIHACK_AGENT_BINARY"),
                "expected rejection of '{bad}'"
            );
        }
    }

    // ── WS3: with_amplihack_home ──────────────────────────────────────────────

    #[test]
    fn with_amplihack_home_valid() {
        // Use /tmp which is guaranteed to exist so canonicalize succeeds.
        unsafe { std::env::set_var("AMPLIHACK_HOME", "/tmp") };
        let env = EnvBuilder::new().with_amplihack_home().build();
        unsafe { std::env::remove_var("AMPLIHACK_HOME") };
        // The value should be set (canonicalized /tmp may differ per OS).
        assert!(env.contains_key("AMPLIHACK_HOME"));
    }

    #[test]
    fn with_amplihack_home_rejects_dotdot() {
        unsafe { std::env::set_var("AMPLIHACK_HOME", "/home/user/../other") };
        let env = EnvBuilder::new().with_amplihack_home().build();
        unsafe { std::env::remove_var("AMPLIHACK_HOME") };
        assert!(!env.contains_key("AMPLIHACK_HOME"));
    }

    #[test]
    fn with_amplihack_home_rejects_relative() {
        unsafe { std::env::set_var("AMPLIHACK_HOME", "relative/path") };
        let env = EnvBuilder::new().with_amplihack_home().build();
        unsafe { std::env::remove_var("AMPLIHACK_HOME") };
        assert!(!env.contains_key("AMPLIHACK_HOME"));
    }

    #[test]
    fn with_amplihack_home_unset_is_noop() {
        // SAFETY: test-only env mutation.
        unsafe { std::env::remove_var("AMPLIHACK_HOME") };
        let env = EnvBuilder::new().with_amplihack_home().build();
        assert!(!env.contains_key("AMPLIHACK_HOME"));
    }
}
