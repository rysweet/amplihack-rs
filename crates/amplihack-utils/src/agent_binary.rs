//! Single source of truth for resolving the active agent binary.
//!
//! Resolution precedence:
//! 1. `AMPLIHACK_AGENT_BINARY` env var (explicit override; CI/testing).
//! 2. A live session marker in this process's environment -- the CLI actually
//!    hosting this process, which outranks any file on disk.
//! 3. `<cwd-or-ancestor>/.claude/runtime/launcher_context.json` `launcher` field
//!    (persisted state, possibly written by a different session).
//! 4. Built-in default: `"copilot"`.
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
use tracing::{debug, warn};

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

/// Where a resolved agent-binary name came from.
///
/// Only [`ResolutionSource::Env`] means "the caller told us". The other two are
/// inferences, and an inference that lands on the wrong vendor is silent and
/// expensive: agents then run under a different CLI with a different tool
/// timeout policy than the session the user is actually sitting in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionSource {
    /// `AMPLIHACK_AGENT_BINARY` was set and valid.
    Env,
    /// Determined from a live session marker in this process's environment.
    /// The session that is actually running, and it outranks any file.
    SessionMarker,
    /// Read from a persisted `launcher_context.json`, possibly written by an
    /// unrelated earlier session in the same repo.
    LauncherContext,
    /// Nothing said anything; [`DEFAULT_BINARY`] was assumed.
    Default,
}

impl ResolutionSource {
    /// `true` when the name was inferred rather than observed.
    ///
    /// A session marker counts as observed: it is exported by the CLI actually
    /// hosting this process, so it is evidence about the present, not a guess
    /// from a file that may describe someone else's session.
    pub fn is_inferred(self) -> bool {
        !matches!(
            self,
            ResolutionSource::Env | ResolutionSource::SessionMarker
        )
    }

    /// Short stable label for logs and run headers.
    pub fn label(self) -> &'static str {
        match self {
            ResolutionSource::Env => "env",
            ResolutionSource::SessionMarker => "session_marker",
            ResolutionSource::LauncherContext => "launcher_context",
            ResolutionSource::Default => "default",
        }
    }
}

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
    resolve_with_source(cwd).map(|(name, _)| name)
}

/// Resolve the active agent binary and report which layer supplied it.
///
/// Prefer this over [`resolve`] anywhere the answer is about to be shown to a
/// user or used to launch agents: an inferred result is worth surfacing, and
/// callers cannot tell the difference from the name alone.
///
/// A fallback is logged at WARN, not DEBUG. Issue #1335: a run whose
/// environment did not survive a `tmux new-session` silently resolved to the
/// vendor default and executed every step under a different CLI, with a
/// different tool-timeout policy, for hours. Nothing in the output said so.
pub fn resolve_with_source(cwd: &Path) -> Result<(String, ResolutionSource), ResolveError> {
    // Both lookups run unconditionally so the precedence rule lives in exactly
    // one place, `resolve_layers`. Gating the second on the first being None
    // would encode the ordering twice -- once here and once there -- and the
    // two could then drift without any test noticing.
    let from_env = std::env::var("AMPLIHACK_AGENT_BINARY")
        .ok()
        .and_then(|raw| validate_binary_name(&raw));
    let from_marker = session_marker();
    let from_persisted = lookup_persisted_launcher(cwd);

    let (name, source) = resolve_layers(from_env, from_marker, from_persisted);

    match source {
        ResolutionSource::Env | ResolutionSource::SessionMarker => {
            debug!(binary = %name, source = source.label(), "agent binary resolved");
        }
        ResolutionSource::LauncherContext => warn!(
            binary = %name,
            source = source.label(),
            "AMPLIHACK_AGENT_BINARY is unset; using the value recorded in \
             launcher_context.json, which may have been written by a different \
             session"
        ),
        ResolutionSource::Default => warn!(
            binary = %name,
            source = source.label(),
            "AMPLIHACK_AGENT_BINARY is unset and no launcher_context.json was \
             found; assuming the built-in default, which may not be the CLI you \
             are running"
        ),
    }
    Ok((name, source))
}

/// Pure precedence rule, separated from the three lookups that feed it.
///
/// Kept free of environment and filesystem access so the ordering can be
/// tested directly, and so it is the single place the precedence is written
/// down.
pub fn resolve_layers(
    from_env: Option<String>,
    from_marker: Option<String>,
    from_persisted: Option<String>,
) -> (String, ResolutionSource) {
    if let Some(name) = from_env {
        return (name, ResolutionSource::Env);
    }
    if let Some(name) = from_marker {
        return (name, ResolutionSource::SessionMarker);
    }
    if let Some(name) = from_persisted {
        return (name, ResolutionSource::LauncherContext);
    }
    (DEFAULT_BINARY.to_string(), ResolutionSource::Default)
}

#[derive(Deserialize)]
struct LauncherContextSnippet {
    launcher: String,
    /// RFC3339, written by `write_launcher_context`. Absent in files written
    /// before the field existed, which are by definition old -- treated stale.
    #[serde(default)]
    timestamp: Option<String>,
}

/// Identify the agent CLI hosting this process from its own environment.
///
/// Each vendor's CLI exports markers that every child inherits, so this
/// answers "which session am I actually inside" without reading any file.
/// It outranks the persisted layer deliberately: a launcher context is
/// per-directory and last-writer-wins, so on a host running both CLIs it can
/// name a different vendor than the session reading it (issue #1342).
///
/// Unlike a process-ancestry walk this needs no `/proc`, so it behaves the
/// same on every platform.
/// Environment variables that identify the CLI hosting this process, paired
/// with the binary each implies.
///
/// Exported so there is exactly one list. A test that needs to observe a lower
/// layer must clear all of these, and a hand-copied list in a fixture is how
/// that silently stops happening the next time a marker is added.
pub const SESSION_MARKERS: &[(&str, &str)] = &[
    // Claude Code exports CLAUDECODE; the others are older spellings that
    // llm_client already recognised.
    ("CLAUDECODE", "claude"),
    ("CLAUDE_CODE", "claude"),
    ("CLAUDE_CODE_SESSION_ID", "claude"),
    ("CLAUDE_PROJECT_DIR", "claude"),
    ("COPILOT_CLI", "copilot"),
    ("GITHUB_COPILOT", "copilot"),
    ("GITHUB_COPILOT_AGENT", "copilot"),
    ("COPILOT_AGENT", "copilot"),
];

fn session_marker() -> Option<String> {
    SESSION_MARKERS.iter().find_map(|(key, binary)| {
        std::env::var_os(key)
            .is_some_and(|v| !v.is_empty())
            .then(|| (*binary).to_string())
    })
}

/// Returns `true` when a launcher context found in `dir` cannot be trusted.
///
/// Two conditions disqualify a directory:
///
/// * **World-writable** (`o+w`, e.g. `/tmp` at `1777`) -- any local user can
///   drop a `.claude/runtime/launcher_context.json` there, and the walk-up
///   would then pick the agent binary for every working directory beneath it.
/// * **Owned by another user** -- the context reflects someone else's session.
///
/// Group-writable is deliberately *not* disqualifying: a `umask 002` setup
/// makes a user's own directories `0775`, and treating those as hostile would
/// break ordinary installs.
#[cfg(unix)]
fn is_untrusted_context_dir(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match fs::metadata(dir) {
        Ok(meta) => meta.mode() & 0o002 != 0 || meta.uid() != nix_getuid(),
        // Unreadable metadata is not a licence to trust the directory.
        Err(_) => true,
    }
}

#[cfg(unix)]
fn nix_getuid() -> u32 {
    // SAFETY: `getuid` is always safe; it takes no arguments, reads process
    // credentials, and cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn is_untrusted_context_dir(_dir: &Path) -> bool {
    false
}

/// Walk up from `start` looking for `.claude/runtime/launcher_context.json`.
///
/// Stops at any `.git` directory boundary, at the first world-writable or
/// foreign-owned directory, or after [`ANCESTOR_WALK_LIMIT`] hops.
///
/// The shared-directory boundary matters (issue #1335). Workflow worktrees are
/// created under the system temp directory, which has no `.git` anywhere above
/// it, so the walk used to continue into `/tmp` and `/`. A stale
/// `/tmp/.claude/runtime/launcher_context.json` -- days old, written by an
/// unrelated session -- then decided which agent CLI every step ran under, for
/// any working directory beneath `/tmp`.
fn lookup_persisted_launcher(start: &Path) -> Option<String> {
    let anchor = start.canonicalize().ok()?;
    let mut current: PathBuf = anchor.clone();
    for _ in 0..ANCESTOR_WALK_LIMIT {
        if is_untrusted_context_dir(&current) {
            debug!(
                dir = %current.display(),
                "stopping launcher_context walk-up at an untrusted directory"
            );
            return None;
        }
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
    // A launcher context describes a session, and sessions end. The hooks
    // reader has always applied a staleness bound; this one never did, so a
    // file written days earlier by an unrelated session kept deciding which
    // agent CLI ran (issue #1335).
    let stale = parsed
        .timestamp
        .as_deref()
        .map(crate::launcher_context::is_timestamp_stale)
        .unwrap_or(true);
    if stale {
        debug!(
            path = %canonical.display(),
            timestamp = ?parsed.timestamp,
            "ignoring launcher context older than the staleness bound"
        );
        return None;
    }
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

    // ---------------------------------------------------------------------
    // Issue #1335: the caller must be able to tell a supplied answer from a
    // guessed one. `resolve` alone cannot -- both return a bare String.
    // ---------------------------------------------------------------------

    #[test]
    fn source_labels_are_stable_and_distinct() {
        let labels = [
            ResolutionSource::Env.label(),
            ResolutionSource::LauncherContext.label(),
            ResolutionSource::Default.label(),
        ];
        let unique: std::collections::HashSet<&&str> = labels.iter().collect();
        assert_eq!(labels.len(), unique.len());
        assert_eq!(ResolutionSource::Env.label(), "env");
    }

    // ---------------------------------------------------------------------
    // Issue #1335 -- precedence, tested through the pure `resolve_layers`
    // seam so the rule is exercised without touching env or filesystem.
    // ---------------------------------------------------------------------

    #[test]
    fn env_wins_over_everything() {
        let (name, source) = resolve_layers(
            Some("claude".into()),
            Some("copilot".into()),
            Some("codex".into()),
        );
        assert_eq!(name, "claude");
        assert_eq!(source, ResolutionSource::Env);
        assert!(!source.is_inferred());
    }

    #[test]
    fn persisted_file_is_used_only_when_nothing_better_exists() {
        let (name, source) = resolve_layers(None, None, Some("codex".into()));
        assert_eq!(name, "codex");
        assert_eq!(source, ResolutionSource::LauncherContext);
        assert!(source.is_inferred());
    }

    #[test]
    fn default_is_the_last_resort_and_is_marked_inferred() {
        let (name, source) = resolve_layers(None, None, None);
        assert_eq!(name, DEFAULT_BINARY);
        assert_eq!(source, ResolutionSource::Default);
        assert!(source.is_inferred());
    }

    // ---------------------------------------------------------------------
    // Trust boundary on the persisted layer.
    // ---------------------------------------------------------------------

    /// A context is only consulted while it is fresh, so the fixture has to
    /// carry a real timestamp. Writing one without it exercises the
    /// no-timestamp-is-stale path, not the walk-up these tests are about.
    fn write_launcher_context(dir: &Path, launcher: &str) {
        let runtime = dir.join(".claude").join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        fs::write(
            runtime.join("launcher_context.json"),
            format!(r#"{{"launcher":"{launcher}","timestamp":"{now}"}}"#),
        )
        .unwrap();
    }

    /// Workflow worktrees live under the system temp directory, which has no
    /// `.git` above it, so the walk-up used to reach `/tmp` -- where a
    /// five-day-old file written by an unrelated session was deciding the
    /// agent binary for every run beneath it.
    #[test]
    #[cfg(unix)]
    fn world_writable_ancestor_context_is_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let shared = tempfile::tempdir().unwrap();
        fs::set_permissions(shared.path(), fs::Permissions::from_mode(0o1777)).unwrap();
        write_launcher_context(shared.path(), "copilot");
        let work = shared.path().join("work");
        fs::create_dir_all(&work).unwrap();
        fs::set_permissions(&work, fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(
            lookup_persisted_launcher(&work),
            None,
            "a context under a world-writable ancestor must not be consulted"
        );
    }

    /// The boundary must not be so strict that ordinary installs break:
    /// `umask 002` leaves a user's own directories group-writable at 0775.
    #[test]
    #[cfg(unix)]
    fn group_writable_owned_ancestor_is_still_trusted() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o775)).unwrap();
        write_launcher_context(root.path(), "codex");
        let work = root.path().join("repo");
        fs::create_dir_all(&work).unwrap();

        assert_eq!(lookup_persisted_launcher(&work).as_deref(), Some("codex"));
    }

    /// Issue #1342 / crusty B1. Symmetric writes let the persisted layer say
    /// "claude" for the first time. Without a marker layer above it, a Copilot
    /// session whose environment variable was lost would read a claude-written
    /// file and spawn the wrong binary -- a failure that could not exist while
    /// the file could only ever say copilot.
    #[test]
    fn a_live_session_marker_beats_a_file_written_by_another_vendor() {
        let (name, source) = resolve_layers(None, Some("copilot".into()), Some("claude".into()));
        assert_eq!(name, "copilot", "a copilot session must not spawn claude");
        assert_eq!(source, ResolutionSource::SessionMarker);

        let (name, source) = resolve_layers(None, Some("claude".into()), Some("copilot".into()));
        assert_eq!(
            name, "claude",
            "and a claude session must not spawn copilot"
        );
        assert_eq!(source, ResolutionSource::SessionMarker);
    }

    /// The requirement in the maintainer's words: "if started with `amplihack
    /// claude` you must stick with claude". With no environment variable and no
    /// file at all, the marker alone must carry it.
    #[test]
    fn a_session_marker_alone_decides_when_nothing_else_speaks() {
        let (name, source) = resolve_layers(None, Some("claude".into()), None);
        assert_eq!(name, "claude");
        assert_eq!(source, ResolutionSource::SessionMarker);
        assert!(!source.is_inferred(), "a live marker is not an inference");
    }

    /// An explicit override still wins over everything, including the marker.
    #[test]
    fn env_still_beats_the_session_marker() {
        let (name, source) = resolve_layers(
            Some("codex".into()),
            Some("claude".into()),
            Some("copilot".into()),
        );
        assert_eq!(name, "codex");
        assert_eq!(source, ResolutionSource::Env);
    }
}
