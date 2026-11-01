//! SessionEnd markers (R6.3, A6, S14): `~/.amplihack/agents/ended/<id>`,
//! written by the `amplihack-hook session-stop-event` hook when a session is
//! exited on purpose, read and deleted by the supervisor. Markers need no
//! lock: the hook writes by atomic rename and the supervisor is the only
//! reader and deleter.

use super::registry::{Registry, State, parse_ts};
use amplihack_utils::agent_marker::{CLEAN_REASONS, is_conversation_id};
use amplihack_utils::secure_files::{check_private_dir, owned_by_euid, read_bounded_nofollow};
use serde::Deserialize;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const MARKER_LIMIT: u64 = 4 * 1024;
/// How long an unmatched or invalid marker is kept.
const KEEP_SECS: i64 = 7 * 24 * 60 * 60;

/// One valid marker.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub session_id: String,
    /// Unix seconds.
    pub at: i64,
    pub path: PathBuf,
}

#[derive(Deserialize)]
struct Raw {
    session_id: String,
    reason: String,
    at: i64,
}

/// The valid markers directly in `dir`. Names starting with `.` (writes in
/// progress) are skipped. Any other entry that fails the S14 checks is
/// unlinked once its `lstat` mtime is more than 7 days before `now`, and
/// left otherwise. Nothing is followed or recursed into. A missing `dir`
/// holds no markers; one that is not a private directory of this user (a
/// symlink, say) is an error, and nothing in it is read or removed.
pub fn read(dir: &Path, now: i64) -> Result<Vec<Marker>, String> {
    match check_private_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    }
    let entries = fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.as_encoded_bytes().first() == Some(&b'.') {
            continue;
        }
        let path = entry.path();
        match name.to_str().and_then(|n| parse(&path, n)) {
            Some(m) => out.push(m),
            None => remove_if_old(&path, now),
        }
    }
    Ok(out)
}

/// A marker named `name` at `path`: a valid id, a regular private file of
/// this user of at most 4 KiB, and JSON naming the same id with a clean
/// reason.
fn parse(path: &Path, name: &str) -> Option<Marker> {
    is_conversation_id(name).then_some(())?;
    let (bytes, meta) = read_bounded_nofollow(path, MARKER_LIMIT).ok()??;
    if !owned_by_euid(&meta) || meta.mode() & 0o077 != 0 {
        return None;
    }
    let raw: Raw = serde_json::from_slice(&bytes).ok()?;
    let valid = raw.session_id == name && CLEAN_REASONS.contains(&raw.reason.as_str());
    valid.then(|| Marker {
        session_id: raw.session_id,
        at: raw.at,
        path: path.to_path_buf(),
    })
}

/// Unlinks a file or symlink in `ended/` whose own mtime is older than 7
/// days. Directories and other types are left alone.
fn remove_if_old(path: &Path, now: i64) {
    let Ok(m) = fs::symlink_metadata(path) else {
        return;
    };
    let kind = m.file_type();
    if !(kind.is_file() || kind.is_symlink()) || now - m.mtime() <= KEEP_SECS {
        return;
    }
    if let Err(e) = fs::remove_file(path) {
        tracing::debug!(path = %path.display(), error = %e, "could not remove an invalid marker");
    }
}

/// What to do with one marker.
#[derive(Debug, Clone, PartialEq)]
pub enum Disposition {
    /// End this agent (`ended_by: session-end`), then delete the marker.
    End(String),
    /// Delete the marker without changing any agent.
    Delete,
    /// Leave the marker for a later pass.
    Keep,
}

/// Design 4.14. The `active` agent with the marker's conversation is
/// matched first: a marker later than its `launched_at` ends it unless its
/// recorded process is still alive (Keep); an earlier one is stale (Delete).
/// A marker matching only agents that are not active is deleted. An
/// unmatched marker is kept for 7 days.
pub fn decide(
    m: &Marker,
    reg: &Registry,
    alive: &dyn Fn(u32, u64) -> bool,
    now: i64,
) -> Disposition {
    let id = Some(m.session_id.as_str());
    let active = reg
        .agents
        .iter()
        .find(|(_, e)| e.state == State::Active && e.conversation_id.as_deref() == id);
    if let Some((name, e)) = active {
        let later = parse_ts(&e.launched_at).is_some_and(|launched| m.at > launched);
        if !later {
            return Disposition::Delete;
        }
        if e.recorded().is_some_and(|(pid, start)| alive(pid, start)) {
            return Disposition::Keep;
        }
        return Disposition::End(name.clone());
    }
    if reg
        .agents
        .values()
        .any(|e| e.conversation_id.as_deref() == id)
    {
        return Disposition::Delete;
    }
    if now - m.at > KEEP_SECS {
        Disposition::Delete
    } else {
        Disposition::Keep
    }
}
