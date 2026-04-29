//! CLI-side thin wrapper around [`amplihack_utils::agent_binary::resolve`].
//!
//! The resolver lives in `amplihack-utils` so that every crate (CLI, hooks,
//! workflows, utils) reaches the same single source of truth. This module
//! exists so CLI consumers can write
//!
//! ```ignore
//! use amplihack_cli::env_builder::agent_binary_resolver;
//! let binary = agent_binary_resolver::resolve(&cwd);
//! ```
//!
//! …without taking a direct dependency on the utils-internal API surface
//! beyond what the documented contract specifies.
//!
//! Resolution precedence (issue #489):
//!   1. `AMPLIHACK_AGENT_BINARY` env var (allowlist-validated)
//!   2. `<repo>/.claude/runtime/launcher_context.json` `launcher` field
//!   3. Default: `"copilot"`

use std::path::Path;

use amplihack_utils::agent_binary;

/// Resolve the active agent binary for `cwd` using the canonical precedence.
///
/// Always returns an allowlisted binary name (one of `amplifier`, `claude`,
/// `codex`, `copilot`). Invalid env var values, unreadable launcher context
/// files, and bogus payloads all fall through to the default `"copilot"`.
pub fn resolve(cwd: &Path) -> String {
    match agent_binary::resolve(cwd) {
        Ok(name) => name,
        Err(err) => {
            tracing::warn!(error = %err, "agent_binary resolver failed; falling back to default");
            agent_binary::DEFAULT_BINARY.to_string()
        }
    }
}

/// Return the built-in default binary name (`"copilot"`).
pub fn default_binary() -> &'static str {
    agent_binary::DEFAULT_BINARY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_copilot() {
        assert_eq!(default_binary(), "copilot");
    }

    #[test]
    fn resolve_returns_allowlisted_value() {
        let tmp = tempfile::tempdir().unwrap();
        // Without env override or launcher context file, the result must be
        // the built-in default.
        let result = resolve(tmp.path());
        assert!(
            agent_binary::ALLOWED_BINARIES.contains(&result.as_str()),
            "resolver returned non-allowlisted binary: {result}"
        );
    }
}
