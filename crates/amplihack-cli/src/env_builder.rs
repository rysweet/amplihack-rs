//! Type-safe environment builder for launching child processes.
//!
//! Constructs the environment variables needed by launched tools,
//! including AMPLIHACK_* vars and PATH augmentation. Uses set-based
//! PATH deduplication instead of error-prone substring matching.

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;

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
