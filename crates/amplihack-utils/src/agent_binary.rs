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
//! An `AMPLIHACK_AGENT_BINARY` that a parent exported from layer 4 carries
//! [`SOURCE_ENV`]`=default:<binary>` beside it. While the two still agree, the
//! value is a guess handed down, not an instruction, so layer 1 ignores it and
//! the lower layers answer again (issue #1481). Anyone who later sets a
//! different binary has made a choice, and the stale tag no longer applies.
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
//!   typed struct (extra fields ignored) — malformed input falls back, and
//!   is reported with its path in [`Resolution::unusable_contexts`] (#1525).
//!   A reason never echoes the file's contents.
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

/// Environment variable naming the agent binary.
pub const BINARY_ENV: &str = "AMPLIHACK_AGENT_BINARY";

/// Companion to [`BINARY_ENV`], exported beside it when the value came from the
/// built-in default rather than from anything that observed a session.
///
/// Issue #1481: `amplihack recipe run` exports the binary to recipe-runner-rs
/// so every agent step agrees. When all it had was the vendor default, that
/// export used to be indistinguishable from an instruction: each step then
/// launched `amplihack copilot`, which persisted a launcher context saying
/// copilot, which pinned every later run in the checkout. The tag keeps the
/// guess a guess all the way down.
///
/// The value is `default:<binary>` -- see [`default_guess_tag`]. It names the
/// binary it describes because every descendant inherits it: a bash step that
/// sets `AMPLIHACK_AGENT_BINARY=codex` without clearing the tag has still
/// chosen codex, and must not be overruled by a tag describing an earlier
/// guess.
pub const SOURCE_ENV: &str = "AMPLIHACK_AGENT_BINARY_SOURCE";

/// The [`SOURCE_ENV`] value marking `binary` as a default-layer guess.
pub fn default_guess_tag(binary: &str) -> String {
    format!("{}:{binary}", ResolutionSource::Default.label())
}

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
/// callers cannot tell the difference from the name alone. Use
/// [`resolve_detailed`] when the user is about to be told *why*.
pub fn resolve_with_source(cwd: &Path) -> Result<(String, ResolutionSource), ResolveError> {
    resolve_detailed(cwd).map(|resolution| (resolution.binary, resolution.source))
}

/// A `launcher_context.json` the walk-up found but could not use.
///
/// Issue #1525: an empty or malformed file used to be dropped without a word,
/// and the walk then carried on into ancestors, so a parent directory's file
/// could answer instead. Nothing the user saw named either file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusableContext {
    /// The file, as the walk-up found it.
    pub path: PathBuf,
    /// Why it could not be used, e.g. "is empty". Never echoes its contents.
    pub reason: String,
}

/// Everything [`resolve_detailed`] learned on the way to its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// The allowlisted binary name.
    pub binary: String,
    /// The layer that supplied it.
    pub source: ResolutionSource,
    /// The `launcher_context.json` that supplied it. `Some` exactly when
    /// `source` is [`ResolutionSource::LauncherContext`]. The walk-up visits
    /// ancestors, so this need not be in the directory resolution started from.
    pub context_file: Option<PathBuf>,
    /// Launcher contexts the walk-up passed over because they could not be
    /// used, nearest first. The fall-through is kept; these say what was
    /// skipped on the way. Stale files are not listed: sessions end, and an
    /// old file is expected, not broken.
    pub unusable_contexts: Vec<UnusableContext>,
}

/// [`resolve_with_source`], plus the persisted-layer evidence a user needs
/// when the answer was inferred: which file decided, and which were skipped.
///
/// A fallback is logged at WARN, not DEBUG. Issue #1335: a run whose
/// environment did not survive a `tmux new-session` silently resolved to the
/// vendor default and executed every step under a different CLI, with a
/// different tool-timeout policy, for hours. Nothing in the output said so.
/// WARN is still hidden at the default filter, so a caller about to launch
/// agents on an inferred answer must say so itself (see `recipe run`).
pub fn resolve_detailed(cwd: &Path) -> Result<Resolution, ResolveError> {
    // Both lookups run unconditionally so the precedence rule lives in exactly
    // one place, `resolve_layers`. Gating the second on the first being None
    // would encode the ordering twice -- once here and once there -- and the
    // two could then drift without any test noticing.
    let from_env = if inherited_binary_is_default_guess() {
        debug!("ignoring an inherited AMPLIHACK_AGENT_BINARY that a parent guessed");
        None
    } else {
        std::env::var(BINARY_ENV)
            .ok()
            .and_then(|raw| validate_binary_name(&raw))
    };
    let from_marker = session_marker();
    let persisted = lookup_persisted_launcher(cwd);

    let (name, source) = resolve_layers(
        from_env,
        from_marker,
        persisted.found.as_ref().map(|(name, _)| name.clone()),
    );
    let context_file = match source {
        ResolutionSource::LauncherContext => persisted.found.map(|(_, path)| path),
        _ => None,
    };

    for unusable in &persisted.unusable {
        warn!(
            path = %unusable.path.display(),
            reason = %unusable.reason,
            "ignoring an unusable launcher_context.json"
        );
    }
    match source {
        ResolutionSource::Env | ResolutionSource::SessionMarker => {
            debug!(binary = %name, source = source.label(), "agent binary resolved");
        }
        ResolutionSource::LauncherContext => warn!(
            binary = %name,
            source = source.label(),
            "no usable AMPLIHACK_AGENT_BINARY (unset, rejected, or a parent's \
             default guess) and no session marker; using the value recorded in \
             launcher_context.json, which may have been written by a different \
             session"
        ),
        ResolutionSource::Default => warn!(
            binary = %name,
            source = source.label(),
            "no usable AMPLIHACK_AGENT_BINARY (unset, rejected, or a parent's \
             default guess), no session marker and no usable launcher_context.json \
             was found; assuming the built-in default, which may not be the CLI you \
             are running"
        ),
    }
    Ok(Resolution {
        binary: name,
        source,
        context_file,
        unusable_contexts: persisted.unusable,
    })
}

/// `true` when the inherited [`BINARY_ENV`] is tagged as a parent's fallback
/// to the built-in default (see [`SOURCE_ENV`]).
///
/// Such a value must neither outrank a session marker this process can see nor
/// be persisted as though a session had chosen it.
pub fn inherited_binary_is_default_guess() -> bool {
    is_default_guess(
        std::env::var(BINARY_ENV).ok().as_deref(),
        std::env::var(SOURCE_ENV).ok().as_deref(),
    )
}

/// Pure form of [`inherited_binary_is_default_guess`].
///
/// The tag counts only while it describes the binary actually set: a tag with
/// no value, a tag naming a different binary, or an unrecognised tag all mean
/// the current value was chosen by someone, so it is honoured.
pub fn is_default_guess(binary: Option<&str>, tag: Option<&str>) -> bool {
    match (binary.and_then(validate_binary_name), tag) {
        (Some(binary), Some(tag)) => tag.trim() == default_guess_tag(&binary),
        _ => false,
    }
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
    /// RFC3339, written by `write_launcher_context`, which has always written
    /// it. A file without one, or with one in another format, is unusable and
    /// named (issue #1525); see `read_launcher_field`.
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
///
/// The one copy that cannot import it is `detect_cli` in the migrate skill's
/// `migrate.sh` (issue #1525). `tests/issue_1525_migrate_detect_cli_parity.sh`
/// fails in CI unless that copy has these entries in this order, so a marker
/// added here must be added there too.
pub const SESSION_MARKERS: &[(&str, &str)] = &[
    // Claude Code exports CLAUDECODE; the others are older spellings that
    // llm_client already recognised.
    ("CLAUDECODE", "claude"),
    ("CLAUDE_CODE", "claude"),
    ("CLAUDE_CODE_SESSION_ID", "claude"),
    ("CLAUDE_PROJECT_DIR", "claude"),
    // Issue #1481: exported by Claude Code in every mode (cli, sdk, remote).
    // The dev-orchestrator skill used to tell callers to `env -u CLAUDECODE`,
    // and on a host where CLAUDECODE was the only marker in this list that
    // left nothing to say which CLI was running.
    ("CLAUDE_CODE_ENTRYPOINT", "claude"),
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
fn lookup_persisted_launcher(start: &Path) -> PersistedLookup {
    let mut lookup = PersistedLookup::default();
    let Ok(anchor) = start.canonicalize() else {
        return lookup;
    };
    let mut current: PathBuf = anchor;
    for _ in 0..ANCESTOR_WALK_LIMIT {
        if is_untrusted_context_dir(&current) {
            debug!(
                dir = %current.display(),
                "stopping launcher_context walk-up at an untrusted directory"
            );
            return lookup;
        }
        // Stop at git boundary (but still inspect this dir on this iteration).
        let runtime_file = current
            .join(".claude")
            .join("runtime")
            .join("launcher_context.json");
        if runtime_file.is_file() {
            match read_launcher_field(&runtime_file, &current) {
                ContextRead::Usable(name) => {
                    lookup.found = Some((name, runtime_file));
                    return lookup;
                }
                ContextRead::Stale => {}
                // Keep walking, as before, but keep the evidence: this file
                // was meant to answer, and something above it may now do so.
                ContextRead::Unusable(reason) => lookup.unusable.push(UnusableContext {
                    path: runtime_file,
                    reason,
                }),
            }
        }
        // Don't walk past a .git boundary.
        if current.join(".git").exists() {
            return lookup;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return lookup,
        }
    }
    lookup
}

/// What the walk-up found in the persisted layer.
#[derive(Debug, Default)]
struct PersistedLookup {
    /// The first usable launcher, and the file it came from.
    found: Option<(String, PathBuf)>,
    /// Files passed over on the way, nearest first.
    unusable: Vec<UnusableContext>,
}

/// The outcome of reading one `launcher_context.json`.
#[derive(Debug, PartialEq, Eq)]
enum ContextRead {
    /// Fresh, well-formed, and naming an allowlisted CLI.
    Usable(String),
    /// Well-formed but older than the staleness bound. Expected, not broken.
    Stale,
    /// Cannot be used. The reason never echoes the file's contents.
    Unusable(String),
}

/// Read and validate the `launcher` field. The file is size-capped, parsed as a
/// typed struct (rejects unexpected JSON shapes), and the value is allowlisted.
/// The path is canonicalized and verified to stay within `anchor` to defend
/// against symlink escape.
fn read_launcher_field(path: &Path, anchor: &Path) -> ContextRead {
    let (canonical, canonical_anchor) = match (path.canonicalize(), anchor.canonicalize()) {
        (Ok(canonical), Ok(anchor)) => (canonical, anchor),
        (Err(error), _) | (_, Err(error)) => {
            return ContextRead::Unusable(format!("could not be resolved ({error})"));
        }
    };
    if !canonical.starts_with(&canonical_anchor) {
        debug!(
            path = %canonical.display(),
            anchor = %canonical_anchor.display(),
            "launcher_context path escapes anchor; ignoring"
        );
        return ContextRead::Unusable("is a link to a file outside its directory".to_string());
    }
    let metadata = match fs::metadata(&canonical) {
        Ok(metadata) => metadata,
        Err(error) => return ContextRead::Unusable(format!("could not be read ({error})")),
    };
    if metadata.len() > LAUNCHER_CONTEXT_MAX_BYTES {
        return ContextRead::Unusable(format!(
            "is larger than the {} KiB limit",
            LAUNCHER_CONTEXT_MAX_BYTES / 1024
        ));
    }
    let body = match fs::read_to_string(&canonical) {
        Ok(body) => body,
        Err(error) => return ContextRead::Unusable(format!("could not be read ({error})")),
    };
    if body.trim().is_empty() {
        return ContextRead::Unusable("is empty".to_string());
    }
    // serde_json's own messages can quote a string from the input, so only the
    // category and position are reported.
    let parsed: LauncherContextSnippet = match serde_json::from_str(&body) {
        Ok(parsed) => parsed,
        Err(error) if error.is_data() => {
            return ContextRead::Unusable(format!(
                "is JSON but not a launcher context, which needs a string \
                 \"launcher\" field (line {}, column {})",
                error.line(),
                error.column()
            ));
        }
        Err(error) => {
            return ContextRead::Unusable(format!(
                "is not valid JSON (line {}, column {})",
                error.line(),
                error.column()
            ));
        }
    };
    // A launcher context describes a session, and sessions end. The hooks
    // reader has always applied a staleness bound; this one never did, so a
    // file written days earlier by an unrelated session kept deciding which
    // agent CLI ran (issue #1335).
    //
    // A file whose age cannot be known still fails closed (#1342), but it is
    // named, not passed over as old (#1525). `write_launcher_context` has
    // always written an RFC 3339 timestamp, so a file without one, or with one
    // in another format, was written by hand or by something else. However
    // recent it is, it will never be used, and saying nothing left the user
    // with no way to find out why.
    let Some(timestamp) = parsed.timestamp.as_deref() else {
        return ContextRead::Unusable("has no timestamp, so its age is unknown".to_string());
    };
    let Some(stale) = crate::launcher_context::rfc3339_timestamp_is_stale(timestamp) else {
        return ContextRead::Unusable("has a timestamp that is not RFC 3339".to_string());
    };
    if stale {
        debug!(
            path = %canonical.display(),
            timestamp = ?parsed.timestamp,
            "ignoring launcher context older than the staleness bound"
        );
        return ContextRead::Stale;
    }
    match validate_binary_name(&parsed.launcher) {
        Some(name) => ContextRead::Usable(name),
        None => ContextRead::Unusable(
            "does not name amplifier, claude, codex or copilot as its launcher".to_string(),
        ),
    }
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

    /// The launcher the persisted layer would answer with from `dir`.
    fn found_launcher(dir: &Path) -> Option<String> {
        lookup_persisted_launcher(dir).found.map(|(name, _)| name)
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
            found_launcher(&work),
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

        assert_eq!(found_launcher(&work).as_deref(), Some("codex"));
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

    // ---------------------------------------------------------------------
    // Issue #1481 -- the default-guess tag is bound to the value it describes.
    // ---------------------------------------------------------------------

    #[test]
    fn a_tag_matching_the_value_marks_it_as_a_guess() {
        assert!(is_default_guess(Some("copilot"), Some("default:copilot")));
        assert!(is_default_guess(Some(" Copilot "), Some("default:copilot")));
        // The tag's surrounding whitespace is trimmed too, as migrate.sh's
        // detect_cli does (tests/issue_1481_migrate_detect_cli_default_tag.sh).
        assert!(is_default_guess(Some("copilot"), Some(" default:copilot ")));
        // ...but not its inside, and not its case.
        assert!(!is_default_guess(Some("copilot"), Some("default: copilot")));
        assert!(!is_default_guess(Some("copilot"), Some("DEFAULT:copilot")));
    }

    /// Every step of a default-guess run inherits the tag. A step that then
    /// names a binary on purpose has made a choice the stale tag must not veto.
    #[test]
    fn a_tag_describing_a_different_binary_does_not_veto_an_explicit_choice() {
        assert!(!is_default_guess(Some("codex"), Some("default:copilot")));
    }

    #[test]
    fn a_bare_or_unknown_tag_is_not_a_guess() {
        assert!(!is_default_guess(Some("copilot"), Some("default")));
        assert!(!is_default_guess(Some("copilot"), Some("session_marker")));
        assert!(!is_default_guess(Some("copilot"), None));
        assert!(!is_default_guess(None, Some("default:copilot")));
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

    // ---------------------------------------------------------------------
    // Issue #1525 -- an unusable launcher context is reported, not dropped.
    // ---------------------------------------------------------------------

    /// Write `body` as the launcher context in `dir`, returning its path.
    fn write_raw_context(dir: &Path, body: &str) -> PathBuf {
        let runtime = dir.join(".claude").join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        let path = runtime.join("launcher_context.json");
        fs::write(&path, body).unwrap();
        path
    }

    fn read_raw(body: &str) -> ContextRead {
        let dir = tempfile::tempdir().unwrap();
        let path = write_raw_context(dir.path(), body);
        read_launcher_field(&path, dir.path())
    }

    #[test]
    fn an_empty_context_is_unusable_and_says_so() {
        assert_eq!(read_raw(""), ContextRead::Unusable("is empty".into()));
        assert_eq!(read_raw(" \n\t"), ContextRead::Unusable("is empty".into()));
    }

    #[test]
    fn a_context_that_is_not_json_is_unusable_with_a_position() {
        let ContextRead::Unusable(reason) = read_raw("{\"launcher\": claude") else {
            panic!("invalid JSON must be unusable");
        };
        assert!(
            reason.starts_with("is not valid JSON (line 1, column"),
            "{reason}"
        );
    }

    /// serde_json would quote the input in its message; the reason must not,
    /// because it ends up on a terminal.
    #[test]
    fn a_json_context_of_the_wrong_shape_is_unusable_and_not_echoed() {
        for body in [
            r#"{"timestamp":"2026-01-01T00:00:00Z"}"#,
            r#"{"launcher":5}"#,
            r#""\u001b[31mclaude""#,
        ] {
            let ContextRead::Unusable(reason) = read_raw(body) else {
                panic!("{body:?} must be unusable");
            };
            assert!(
                reason.starts_with("is JSON but not a launcher context"),
                "{reason}"
            );
            assert!(
                !reason.contains("claude") && !reason.contains('\u{1b}'),
                "{reason}"
            );
        }
    }

    #[test]
    fn a_fresh_context_naming_no_known_cli_is_unusable() {
        let now = chrono::Utc::now().to_rfc3339();
        assert_eq!(
            read_raw(&format!(r#"{{"launcher":"vim","timestamp":"{now}"}}"#)),
            ContextRead::Unusable(
                "does not name amplifier, claude, codex or copilot as its launcher".into()
            )
        );
    }

    /// Sessions end; an old file is expected, not broken, and is not reported.
    #[test]
    fn a_stale_context_is_stale_not_unusable() {
        assert_eq!(
            read_raw(r#"{"launcher":"claude","timestamp":"2001-01-01T00:00:00Z"}"#),
            ContextRead::Stale
        );
    }

    /// Issue #1525 review: a file whose age cannot be known is still not used,
    /// but it is named. It used to be passed over as stale, with nothing to
    /// say which file was skipped or why, while migrate.sh's `date -d` read
    /// the same "2026-10-04 13:13:46" as fresh and answered `claude`.
    #[test]
    fn a_context_whose_age_cannot_be_known_is_unusable_not_stale() {
        let no_timestamp = ContextRead::Unusable("has no timestamp, so its age is unknown".into());
        assert_eq!(read_raw(r#"{"launcher":"claude"}"#), no_timestamp);
        assert_eq!(
            read_raw(r#"{"launcher":"claude","timestamp":null}"#),
            no_timestamp
        );

        let not_rfc3339 = ContextRead::Unusable("has a timestamp that is not RFC 3339".into());
        let now = chrono::Utc::now();
        for timestamp in [
            // No offset: the case from the review.
            now.format("%Y-%m-%d %H:%M:%S").to_string(),
            now.format("%Y-%m-%dT%H:%M:%S").to_string(),
            // Offset without a colon.
            now.format("%Y-%m-%dT%H:%M:%S+0000").to_string(),
            now.format("%s").to_string(),
            String::new(),
            "yesterday".to_string(),
            // Out of range.
            "2026-02-30T00:00:00Z".to_string(),
            "2026-10-04T24:00:00Z".to_string(),
            "2026-10-04T13:13:46+24:00".to_string(),
        ] {
            assert_eq!(
                read_raw(&format!(
                    r#"{{"launcher":"claude","timestamp":"{timestamp}"}}"#
                )),
                not_rfc3339,
                "{timestamp:?}"
            );
        }
    }

    /// The forms chrono's RFC 3339 parser accepts, which migrate.sh's
    /// `_detect_cli_epoch` mirrors (tests/issue_1525_migrate_detect_cli_parity.sh).
    #[test]
    fn a_fresh_context_in_any_rfc3339_form_is_usable() {
        let now = chrono::Utc::now();
        for timestamp in [
            now.to_rfc3339(),
            now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            now.format("%Y-%m-%dt%H:%M:%Sz").to_string(),
            now.format("%Y-%m-%d %H:%M:%S+00:00").to_string(),
            now.format("%Y-%m-%dT%H:%M:%S.%f-00:00").to_string(),
        ] {
            assert_eq!(
                read_raw(&format!(
                    r#"{{"launcher":"claude","timestamp":"{timestamp}"}}"#
                )),
                ContextRead::Usable("claude".into()),
                "{timestamp:?}"
            );
        }
    }

    #[test]
    fn an_oversized_context_is_unusable() {
        let body = format!(
            r#"{{"launcher":"claude","pad":"{}"}}"#,
            "x".repeat(LAUNCHER_CONTEXT_MAX_BYTES as usize)
        );
        assert_eq!(
            read_raw(&body),
            ContextRead::Unusable("is larger than the 64 KiB limit".into())
        );
    }

    /// The fall-through is kept: a bad file nearer the start does not stop the
    /// walk-up. But the bad file is recorded, and the file that did answer is
    /// the one reported -- not a fixed relative path that names neither.
    #[test]
    fn the_walk_up_records_a_bad_file_and_names_the_one_that_answered() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".git")).unwrap();
        write_launcher_context(root.path(), "codex");
        let sub = root.path().join("sub");
        let bad = write_raw_context(&sub, "");

        let lookup = lookup_persisted_launcher(&sub);
        let (name, path) = lookup.found.expect("the ancestor still answers");
        assert_eq!(name, "codex");
        assert_eq!(
            path,
            root.path()
                .canonicalize()
                .unwrap()
                .join(".claude/runtime/launcher_context.json")
        );
        assert_eq!(
            lookup.unusable,
            vec![UnusableContext {
                path: sub
                    .canonicalize()
                    .unwrap()
                    .join(".claude/runtime/launcher_context.json"),
                reason: "is empty".into(),
            }]
        );
        assert!(bad.exists());
    }

    /// A stale file is passed over without being listed.
    #[test]
    fn the_walk_up_does_not_list_a_stale_file() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".git")).unwrap();
        write_raw_context(
            root.path(),
            r#"{"launcher":"claude","timestamp":"2001-01-01T00:00:00Z"}"#,
        );
        let lookup = lookup_persisted_launcher(root.path());
        assert!(lookup.found.is_none());
        assert!(lookup.unusable.is_empty(), "{:?}", lookup.unusable);
    }
}
