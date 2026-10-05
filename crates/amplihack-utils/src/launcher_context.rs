//! Shared launcher context persistence for launcher commands and hooks.

use amplihack_types::ProjectDirs;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const DEFAULT_STALE_HOURS: i64 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LauncherKind {
    Claude,
    Copilot,
    Codex,
    Amplifier,
    Unknown,
}

impl LauncherKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Copilot => "copilot",
            Self::Codex => "codex",
            Self::Amplifier => "amplifier",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LauncherContext {
    pub launcher: LauncherKind,
    pub command: String,
    pub timestamp: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

/// Record which launcher started a session in `project_root`.
///
/// The file is replaced, never rewritten in place: the body goes to a
/// temporary file beside it, which is then renamed over it. `fs::write`
/// truncates first and writes second, so a write cut short -- a full disk
/// (`/tmp` on a RAM-backed host), a killed process -- left a 0-byte file. The
/// agent-binary resolver then reported that file as empty and told the user to
/// fix or delete it (crusty review of #1490). A reader now sees the previous
/// file or the new one.
pub fn write_launcher_context(
    project_root: &Path,
    launcher: LauncherKind,
    command: impl Into<String>,
    environment: BTreeMap<String, String>,
) -> Result<PathBuf> {
    let dirs = ProjectDirs::from_root(project_root);
    fs::create_dir_all(&dirs.runtime)
        .with_context(|| format!("failed to create {}", dirs.runtime.display()))?;
    let context_path = dirs.launcher_context_file();
    let context = LauncherContext {
        launcher,
        command: command.into(),
        timestamp: Utc::now().to_rfc3339(),
        environment,
    };
    let body =
        serde_json::to_string_pretty(&context).context("failed to encode launcher context")?;
    replace_file(&context_path, |file| file.write_all(body.as_bytes()))
        .with_context(|| format!("failed to write {}", context_path.display()))?;
    Ok(context_path)
}

/// Replace `path` with what `write` puts in a new file, so that `path` holds
/// either its old contents or the complete new ones, never a part.
///
/// The temporary file is created in `path`'s own directory, because
/// `rename(2)` is atomic only within one file system. If `write`, the flush or
/// the rename fails, the temporary file is removed and `path` is untouched.
///
/// The file is owner read and write only, because it records the command line
/// and environment a session was started with. On Unix `tempfile` asks for
/// that when it creates the file: it passes mode 0600 to `open(2)`, so the
/// file is never readable by anyone else, and `rename(2)` keeps the mode.
/// There is deliberately no `chmod` after that. It would repeat what `open`
/// already did, and on a file system that refuses `chmod` (vfat mounted
/// without `quiet`, some FUSE mounts) it fails, which would stop
/// `amplihack claude` and `amplihack copilot` from starting. Before the
/// atomic replacement such a failure was only a warning (crusty review of
/// #1490).
fn replace_file(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> io::Result<()>,
) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", path.display()),
        )
    })?;
    let mut temp = tempfile::Builder::new()
        .prefix(".launcher_context.")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    write(temp.as_file_mut())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub fn read_launcher_context(project_root: &Path) -> Option<LauncherContext> {
    let path = launcher_context_path(project_root);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                "failed reading launcher context: {error}"
            );
            return None;
        }
    };
    match serde_json::from_str::<LauncherContext>(&raw) {
        Ok(context) => Some(context),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                "invalid launcher context file: {error}"
            );
            None
        }
    }
}

pub fn launcher_context_path(project_root: &Path) -> PathBuf {
    ProjectDirs::from_root(project_root).launcher_context_file()
}

pub fn is_launcher_context_stale(context: &LauncherContext) -> bool {
    is_launcher_context_stale_with(context, DEFAULT_STALE_HOURS)
}

/// Whether an RFC3339 timestamp is older than the launcher-context staleness
/// bound, or is unparseable.
///
/// Exposed so every reader of a launcher context applies the same bound. The
/// agent-binary resolver has its own hardened parser (size cap, symlink-escape
/// check, allowlist) and so cannot go through [`read_launcher_context`], but it
/// must not therefore get its own idea of how old is too old. It previously had
/// none at all, and honoured a five-day-old file (issue #1335).
pub fn is_timestamp_stale(timestamp: &str) -> bool {
    rfc3339_timestamp_is_stale(timestamp).unwrap_or(true)
}

/// [`is_timestamp_stale`] for a reader that must tell "old" from "not a
/// timestamp": `None` when `timestamp` is not RFC 3339.
///
/// Both still mean "do not use the file", but they are different findings. An
/// old file is expected, because sessions end. A file whose timestamp cannot be
/// read was hand-written or written by something else, and it will never be
/// used however recent it is. The agent-binary resolver names such a file
/// rather than passing over it as old (issue #1525).
pub fn rfc3339_timestamp_is_stale(timestamp: &str) -> Option<bool> {
    let parsed = DateTime::parse_from_rfc3339(timestamp).ok()?;
    Some(
        Utc::now().signed_duration_since(parsed.with_timezone(&Utc))
            > Duration::hours(DEFAULT_STALE_HOURS),
    )
}

/// One rule, one place. `max_age_hours` had a single caller passing a single
/// value, and a second copy of the same arithmetic is a drift surface, not
/// shared code.
fn is_launcher_context_stale_with(context: &LauncherContext, _max_age_hours: i64) -> bool {
    is_timestamp_stale(&context.timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_launcher_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut environment = BTreeMap::new();
        environment.insert("AMPLIHACK_LAUNCHER".to_string(), "copilot".to_string());

        let path = write_launcher_context(
            dir.path(),
            LauncherKind::Copilot,
            "amplihack copilot --model opus",
            environment.clone(),
        )
        .unwrap();

        assert_eq!(path, launcher_context_path(dir.path()));
        let restored = read_launcher_context(dir.path()).unwrap();
        assert_eq!(restored.launcher, LauncherKind::Copilot);
        assert_eq!(restored.command, "amplihack copilot --model opus");
        assert_eq!(restored.environment, environment);
        assert!(!is_launcher_context_stale(&restored));
    }

    /// Crusty review of #1490: `fs::write` truncated the file before writing,
    /// so a write cut short left it empty. A failed replacement must leave the
    /// previous file whole and no temporary file behind.
    #[test]
    fn a_failed_write_leaves_the_previous_context_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_launcher_context(
            dir.path(),
            LauncherKind::Claude,
            "amplihack claude",
            BTreeMap::new(),
        )
        .unwrap();
        let before = fs::read(&path).unwrap();

        // Half a body, then the failure a full disk gives.
        let error = replace_file(&path, |file| {
            file.write_all(b"{\"launcher\":")?;
            Err(io::Error::new(io::ErrorKind::StorageFull, "no space left"))
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
        assert_eq!(fs::read(&path).unwrap(), before, "the old file was touched");
        let restored = read_launcher_context(dir.path()).unwrap();
        assert_eq!(restored.launcher, LauncherKind::Claude);
        let left: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, [std::ffi::OsString::from("launcher_context.json")]);
    }

    /// The replacement is owner-only from creation. The mode comes from the
    /// `open(2)` that creates the temporary file, not from a `chmod` that can
    /// fail on its own; see [`replace_file`].
    #[cfg(unix)]
    #[test]
    fn a_written_context_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = write_launcher_context(
            dir.path(),
            LauncherKind::Copilot,
            "amplihack copilot",
            BTreeMap::new(),
        )
        .unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{mode:o}");
    }

    #[test]
    fn treats_old_context_as_stale() {
        let context = LauncherContext {
            launcher: LauncherKind::Copilot,
            command: "amplihack copilot".to_string(),
            timestamp: (Utc::now() - Duration::hours(25)).to_rfc3339(),
            environment: BTreeMap::new(),
        };
        assert!(is_launcher_context_stale(&context));
    }

    #[test]
    fn invalid_context_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = launcher_context_path(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not-json").unwrap();

        assert!(read_launcher_context(dir.path()).is_none());
    }
}
