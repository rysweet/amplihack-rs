//! Per-binary hook resolution with hard-error semantics.
//!
//! Looks up the canonical hook file path for an `(agent_binary, event)` pair
//! under `<root>/<binary>/hooks/<EventName>.py`. When the hook is missing the
//! resolver returns a structured [`HookError::MissingHookForBinary`] — never a
//! silent fallback to another binary's hook, never a stub-file write.
//!
//! ## Security
//!
//! * The `binary` parameter is allowlisted via [`amplihack_utils::agent_binary::validate_binary_name`]
//!   so callers cannot pass `..`, paths, or arbitrary names.
//! * The constructed path is purely relative-join under the caller-supplied
//!   `root`; no globs, no symlink following at construct time.

use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// All hook event variants that the amplihack runtime understands. The
/// `filename_stem()` matches the on-disk filename (without `.py` extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    Stop,
    SessionStart,
    SessionEnd,
    SessionStop,
    UserPromptSubmit,
    PreCompact,
}

impl HookEvent {
    /// Canonical filename stem (no extension) for this event.
    pub fn filename_stem(self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::Stop => "Stop",
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::SessionStop => "SessionStop",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::PreCompact => "PreCompact",
        }
    }
}

impl fmt::Display for HookEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.filename_stem())
    }
}

/// Errors returned by [`expected_hook_path`] and [`resolve_hook`].
#[derive(Debug, Error)]
pub enum HookError {
    /// `binary` was not on the agent-binary allowlist (e.g. `..`, `bash`,
    /// `claude/../sh`, empty string).
    #[error("invalid agent binary name: {0:?}")]
    InvalidBinary(String),

    /// The hook file is absent for the active binary. NO claude fallback is
    /// applied — the operator must remediate.
    #[error(
        "No {event} hook registered for active agent binary '{binary}'. \
         Expected at: {expected_display}. To fix: either install the hook, \
         switch binaries via 'amplihack launch --tool <other>', or set \
         AMPLIHACK_AGENT_BINARY explicitly. {remediation}"
    )]
    MissingHookForBinary {
        binary: String,
        event: HookEvent,
        expected_path: PathBuf,
        remediation: String,
        /// Pre-formatted display string (PathBuf does not implement Display).
        expected_display: String,
    },
}

/// Construct the canonical hook path: `<root>/<binary>/hooks/<EventName>.py`.
///
/// Returns [`HookError::InvalidBinary`] if `binary` is not on the agent-binary
/// allowlist (which structurally blocks path-traversal escapes).
pub fn expected_hook_path(
    root: &Path,
    binary: &str,
    event: HookEvent,
) -> Result<PathBuf, HookError> {
    let validated = amplihack_utils::agent_binary::validate_binary_name(binary)
        .ok_or_else(|| HookError::InvalidBinary(binary.to_string()))?;
    Ok(root
        .join(validated)
        .join("hooks")
        .join(format!("{}.py", event.filename_stem())))
}

/// Resolve `(binary, event)` to a concrete hook file path.
///
/// Returns the path when the file exists. Returns
/// [`HookError::MissingHookForBinary`] when it does not — never falls back to
/// another binary's hook, never creates a stub.
pub fn resolve_hook(root: &Path, binary: &str, event: HookEvent) -> Result<PathBuf, HookError> {
    let expected = expected_hook_path(root, binary, event)?;
    if expected.is_file() {
        return Ok(expected);
    }
    let validated = amplihack_utils::agent_binary::validate_binary_name(binary)
        .ok_or_else(|| HookError::InvalidBinary(binary.to_string()))?;
    let remediation = format!(
        "Install the hook at {}, switch binaries via 'amplihack launch --tool <other>', \
         or set AMPLIHACK_AGENT_BINARY explicitly.",
        expected.display()
    );
    Err(HookError::MissingHookForBinary {
        binary: validated,
        event,
        expected_display: expected.display().to_string(),
        expected_path: expected,
        remediation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_stems_are_canonical() {
        assert_eq!(HookEvent::PreToolUse.filename_stem(), "PreToolUse");
        assert_eq!(HookEvent::SessionEnd.filename_stem(), "SessionEnd");
    }

    #[test]
    fn display_matches_filename_stem() {
        assert_eq!(format!("{}", HookEvent::SessionEnd), "SessionEnd");
    }

    #[test]
    fn expected_path_rejects_invalid_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let err = expected_hook_path(tmp.path(), "..", HookEvent::Stop).unwrap_err();
        assert!(matches!(err, HookError::InvalidBinary(_)));
    }
}
