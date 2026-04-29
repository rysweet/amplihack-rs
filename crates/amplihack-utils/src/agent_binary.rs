//! Single source of truth for resolving the active agent binary.
//!
//! Resolution precedence:
//! 1. `AMPLIHACK_AGENT_BINARY` env var (explicit override; CI/testing).
//! 2. `<cwd-or-ancestor>/.claude/runtime/launcher_context.json` `launcher` field
//!    (canonical persisted state written by the launcher on every run).
//! 3. Built-in default: `"copilot"`.
//!
//! All inputs are validated against a strict allowlist to prevent the resolved
//! value from being used as an arbitrary `Command::new` target by downstream
//! callers. Untrusted values silently fall through to the next layer.
//!
//! ## Security
//!
//! * Allowlist is exactly `{claude, copilot, codex, amplifier}` — case-insensitive
//!   on input, lowercase on output.
//! * Env-var input is length-capped (32 bytes) and rejects path separators,
//!   control characters, and any name not in the allowlist.
//! * `launcher_context.json` is read with a 64 KiB size cap and parsed as a
//!   typed struct (extra fields ignored) — malformed input falls back.
//! * Walk-up ancestor search is capped at 32 levels and stops at any `.git`
//!   boundary. Symlink escape is rejected by canonicalizing the resolved path
//!   and verifying it stays within the anchor tree.
//! * No shell invocation, no subprocess execution.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;
use tracing::debug;

/// Allowlist of valid agent binary names. Keep alphabetical and lowercase.
pub const ALLOWED_BINARIES: &[&str] = &["amplifier", "claude", "codex", "copilot"];

/// Built-in default when no override is present and no launcher_context exists.
pub const DEFAULT_BINARY: &str = "copilot";

/// Maximum bytes accepted from the `AMPLIHACK_AGENT_BINARY` env var.
const ENV_VALUE_MAX_LEN: usize = 32;

/// Maximum bytes read from `launcher_context.json` before rejecting.
const LAUNCHER_CONTEXT_MAX_BYTES: u64 = 64 * 1024;

/// Maximum number of ancestor directories to inspect during walk-up.
const ANCESTOR_WALK_LIMIT: usize = 32;

/// Errors returned by the resolver. Resolution is infallible from the caller's
/// perspective today — this type exists for future-proofing and to give tests a
/// concrete `Err` variant to bind against.
#[derive(Debug, Error)]
pub enum ResolveError {
    /// I/O failure that prevented even the default-fallback path from running.
    #[error("agent-binary resolver i/o failure: {0}")]
    Io(#[from] std::io::Error),
}

/// Returns `Some(canonicalized lowercase name)` when `name` is on the allowlist
/// and free of dangerous characters; `None` otherwise.
///
/// The check is case-insensitive but the returned value is always the canonical
/// lowercase form, suitable for direct use as a `Command` target identifier.
pub fn validate_binary_name(name: &str) -> Option<String> {
    // Reject any control char, NUL, path separator, dot, semicolon, or
    // whitespace anywhere in the *raw* input — these would otherwise be
    // smuggled past `trim()` and used as `Command::new` targets.
    if name.bytes().any(|b| {
        b.is_ascii_control() || b == b'/' || b == b'\\' || b == b'\0' || b == b';' || b == b'.'
    }) {
        return None;
    }
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > ENV_VALUE_MAX_LEN {
        return None;
    }
    // After trim there must be no internal whitespace.
    if trimmed.bytes().any(|b| b == b' ' || b == b'\t') {
        return None;
    }
    let lowered = trimmed.to_ascii_lowercase();
    if ALLOWED_BINARIES.iter().any(|allowed| *allowed == lowered) {
        Some(lowered)
    } else {
        None
    }
}

/// Resolve the active agent binary for the given working directory.
///
/// Always returns an allowlisted name. On any failure mode (rejected env value,
/// missing/oversized/malformed `launcher_context.json`, walk-up limit reached,
/// symlink escape) the function falls through to the next precedence layer and
/// ultimately to [`DEFAULT_BINARY`].
pub fn resolve(cwd: &Path) -> Result<String, ResolveError> {
    // Layer 1: explicit env-var override.
    if let Ok(raw) = std::env::var("AMPLIHACK_AGENT_BINARY")
        && let Some(valid) = validate_binary_name(&raw)
    {
        debug!(binary = %valid, source = "env", "agent binary resolved");
        return Ok(valid);
    }

    // Layer 2: persisted launcher_context.json (walk up ancestors).
    if let Some(name) = lookup_persisted_launcher(cwd) {
        debug!(binary = %name, source = "launcher_context", "agent binary resolved");
        return Ok(name);
    }

    // Layer 3: built-in default.
    debug!(
        binary = DEFAULT_BINARY,
        source = "default",
        "agent binary resolved"
    );
    Ok(DEFAULT_BINARY.to_string())
}

#[derive(Deserialize)]
struct LauncherContextSnippet {
    launcher: String,
}

/// Walk up from `start` looking for `.claude/runtime/launcher_context.json`.
/// Stops at any `.git` directory boundary or after [`ANCESTOR_WALK_LIMIT`] hops.
fn lookup_persisted_launcher(start: &Path) -> Option<String> {
    let anchor = start.canonicalize().ok()?;
    let mut current: PathBuf = anchor.clone();
    for _ in 0..ANCESTOR_WALK_LIMIT {
        // Stop at git boundary (but still inspect this dir on this iteration).
        let runtime_file = current
            .join(".claude")
            .join("runtime")
            .join("launcher_context.json");
        if runtime_file.is_file()
            && let Some(name) = read_launcher_field(&runtime_file, &current)
        {
            return Some(name);
        }
        // Don't walk past a .git boundary.
        if current.join(".git").exists() {
            return None;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return None,
        }
    }
    None
}

/// Read and validate the `launcher` field. The file is size-capped, parsed as a
/// typed struct (rejects unexpected JSON shapes), and the value is allowlisted.
/// The path is canonicalized and verified to stay within `anchor` to defend
/// against symlink escape.
fn read_launcher_field(path: &Path, anchor: &Path) -> Option<String> {
    let canonical = path.canonicalize().ok()?;
    let canonical_anchor = anchor.canonicalize().ok()?;
    if !canonical.starts_with(&canonical_anchor) {
        debug!(
            path = %canonical.display(),
            anchor = %canonical_anchor.display(),
            "launcher_context path escapes anchor; ignoring"
        );
        return None;
    }
    let metadata = fs::metadata(&canonical).ok()?;
    if metadata.len() > LAUNCHER_CONTEXT_MAX_BYTES {
        debug!(
            size = metadata.len(),
            cap = LAUNCHER_CONTEXT_MAX_BYTES,
            "launcher_context exceeds size cap; ignoring"
        );
        return None;
    }
    let body = fs::read_to_string(&canonical).ok()?;
    let parsed: LauncherContextSnippet = serde_json::from_str(&body).ok()?;
    validate_binary_name(&parsed.launcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_allowlisted_lowercase() {
        for name in ALLOWED_BINARIES {
            assert_eq!(validate_binary_name(name).as_deref(), Some(*name));
        }
    }

    #[test]
    fn validate_is_case_insensitive_returns_lowercase() {
        assert_eq!(validate_binary_name("CLAUDE").as_deref(), Some("claude"));
        assert_eq!(validate_binary_name("CoPiLoT").as_deref(), Some("copilot"));
    }

    #[test]
    fn validate_trims_whitespace() {
        assert_eq!(
            validate_binary_name("  claude  ").as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn validate_rejects_dangerous_inputs() {
        for bad in &[
            "",
            "x",
            "claudex",
            "/bin/sh",
            "..",
            "../claude",
            "claude\n",
            "claude\t",
            "cla ude",
            "cla\0ude",
            "claude;rm",
            "rm -rf /",
        ] {
            assert!(
                validate_binary_name(bad).is_none(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_rejects_oversized_input() {
        let s = "a".repeat(ENV_VALUE_MAX_LEN + 1);
        assert!(validate_binary_name(&s).is_none());
    }

    #[test]
    fn allowlist_is_exactly_the_four_known_binaries() {
        let mut sorted = ALLOWED_BINARIES.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, vec!["amplifier", "claude", "codex", "copilot"]);
    }

    #[test]
    fn default_binary_is_copilot() {
        assert_eq!(DEFAULT_BINARY, "copilot");
    }
}
