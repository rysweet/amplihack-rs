//! Once-per-state-change notices: `<root>/.auto-switch.d/notice/<session>`
//! holds the last state shown to that session (`low` or `exhausted`).
//!
//! Best effort throughout: a record that cannot be read counts as none (so
//! the notice is shown), and a record that cannot be written is skipped (so
//! the notice may be shown again). Records are written only into real,
//! private directories owned by the current user.

use super::State;
use std::path::{Path, PathBuf};

/// The record directory under the state directory.
pub const DIR: &str = "notice";
/// A record larger than this counts as no record.
pub const MAX_RECORD: u64 = 64;
/// Sanitized record names are truncated to this many bytes.
pub const MAX_NAME: usize = 128;
/// Records older than this are pruned when a record is written.
pub const PRUNE_AFTER_SECS: u64 = 7 * 86_400;
/// At most this many directory entries are examined per prune.
pub const PRUNE_SCAN: usize = 256;

/// `<root>/.auto-switch.d/notice`.
pub fn dir(root: &Path) -> PathBuf {
    root.join(super::STATE_DIR).join(DIR)
}

/// The record file name for `session_id`: sanitized and capped at
/// [`MAX_NAME`] bytes. `None` for an empty or whitespace-only id.
pub fn record_name(session_id: &str) -> Option<String> {
    // `sanitize_session_id` panics on an empty id, so it never sees one.
    if session_id.trim().is_empty() {
        return None;
    }
    let mut name = amplihack_types::sanitize_session_id(session_id);
    // Sanitized names are ASCII, so any byte offset is a char boundary.
    name.truncate(MAX_NAME);
    Some(name)
}

/// Whether to show a notice for `state` in this session, recording it when
/// shown. `Ok` removes the session's record and returns `false`. Without a
/// usable session id, `Low` and `Exhausted` always show.
pub fn should_show(root: &Path, session_id: Option<&str>, state: State) -> bool {
    let name = session_id.and_then(record_name);
    if state == State::Ok {
        if let Some(name) = name {
            imp::remove(root, &name);
        }
        return false;
    }
    let Some(name) = name else {
        return true;
    };
    if !imp::private_dirs(root) {
        return true;
    }
    let record = dir(root).join(&name);
    if imp::read_record(&record).is_some_and(|shown| shown >= state) {
        return false;
    }
    imp::write_record(&dir(root), &name, state);
    imp::prune(&dir(root));
    true
}

#[cfg(unix)]
mod imp {
    use super::{MAX_RECORD, PRUNE_AFTER_SECS, PRUNE_SCAN, State};
    use crate::claude_capacity::fs::{euid, read_owned};
    use std::fs;
    use std::io::{self, Write};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    /// Removes the session's record, but only when both directories already
    /// pass the private-directory check; it creates nothing.
    pub(super) fn remove(root: &Path, name: &str) {
        let state = root.join(crate::claude_capacity::STATE_DIR);
        let notice = state.join(super::DIR);
        if !is_private_dir(&state) || !is_private_dir(&notice) {
            return;
        }
        remove_path(&notice.join(name));
    }

    fn remove_path(path: &Path) {
        if let Err(e) = fs::remove_file(path)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::debug!(op = "capacity notice remove", kind = ?e.kind());
        }
    }

    /// Creates (0700) or checks `.auto-switch.d` and `notice/`: real
    /// directories owned by the current user with no group or other bits.
    pub(super) fn private_dirs(root: &Path) -> bool {
        let state = root.join(crate::claude_capacity::STATE_DIR);
        private_dir(&state) && private_dir(&state.join(super::DIR))
    }

    fn private_dir(path: &Path) -> bool {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                tracing::debug!(op = "capacity notice mkdir", kind = ?e.kind());
                return false;
            }
        }
        is_private_dir(path)
    }

    /// A real directory (not a symlink) owned by the current user with no
    /// group or other bits. Read-only.
    fn is_private_dir(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|m| {
            m.is_dir() && !m.file_type().is_symlink() && m.uid() == euid() && m.mode() & 0o077 == 0
        })
    }

    pub(super) fn read_record(record: &Path) -> Option<State> {
        let bytes = read_owned(record, MAX_RECORD)?;
        match std::str::from_utf8(&bytes).ok()?.trim() {
            "low" => Some(State::Low),
            "exhausted" => Some(State::Exhausted),
            _ => None,
        }
    }

    /// A new 0600 temp file in `dir`, renamed over the record. The rename
    /// replaces a symlink rather than writing through it.
    pub(super) fn write_record(dir: &Path, name: &str, state: State) {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let tmp = dir.join(format!(".{name}.tmp.{}.{nanos}", std::process::id()));
        let result = (|| -> io::Result<()> {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&tmp)?;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
            f.write_all(state.as_str().as_bytes())?;
            fs::rename(&tmp, dir.join(name))
        })();
        if let Err(e) = result {
            tracing::debug!(op = "capacity notice write", kind = ?e.kind());
            let _ = fs::remove_file(&tmp);
        }
    }

    /// Removes this user's regular files in `dir` older than a week. Looks
    /// at no more than [`PRUNE_SCAN`] entries, never follows a symlink and
    /// never recurses.
    pub(super) fn prune(dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let Some(cutoff) = SystemTime::now().checked_sub(Duration::from_secs(PRUNE_AFTER_SECS))
        else {
            return;
        };
        for entry in entries.take(PRUNE_SCAN).flatten() {
            let path = entry.path();
            let stale = fs::symlink_metadata(&path).is_ok_and(|m| {
                m.file_type().is_file()
                    && m.uid() == euid()
                    && m.modified().is_ok_and(|t| t < cutoff)
            });
            if stale {
                remove_path(&path);
            }
        }
    }
}

/// Off Unix there are no records: every notice shows.
#[cfg(not(unix))]
mod imp {
    use super::State;
    use std::path::Path;

    pub(super) fn remove(_: &Path, _: &str) {}

    pub(super) fn private_dirs(_: &Path) -> bool {
        false
    }

    pub(super) fn read_record(_: &Path) -> Option<State> {
        None
    }

    pub(super) fn write_record(_: &Path, _: &str, _: State) {}

    pub(super) fn prune(_: &Path) {}
}
