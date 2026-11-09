//! Session ids, the `--events` file of `codex-resume`, and the rollout
//! link. A rollout is found by listing directories only and is linked,
//! never opened or copied.

use super::limit::{limit_hit_event, limit_hit_text};
use crate::commands::profile::ProfileError;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, symlink};
use std::path::{Path, PathBuf};

/// The most of an events file read.
const MAX_EVENTS_BYTES: u64 = 16 * 1024 * 1024;
/// The most lines of an events file read.
const MAX_EVENTS_LINES: usize = 100_000;
/// How deep `sessions/` is searched for a rollout.
const MAX_DEPTH: usize = 8;
const MAX_ID: usize = 64;

/// `^[A-Za-z0-9][A-Za-z0-9-]{0,63}$`: it cannot start with `-` and be read
/// as a flag, and cannot hold `/` or `..`.
pub(super) fn valid_session_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_ID
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
}

/// What an events file shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EventsScan {
    /// The first `thread.started` `thread_id` or `session_configured`
    /// `session_id`, unchecked.
    pub session: Option<String>,
    /// Some event shows a usage limit.
    pub limit_hit: bool,
}

fn refused(path: &Path, why: &str) -> ProfileError {
    ProfileError::RotateConfig(format!(
        "codex-resume: events file {}: {why}",
        super::lines::clean_path(path)
    ))
}

/// The session id one event names, if it is a start event.
fn session_of(v: &serde_json::Value) -> Option<String> {
    let s = |v: &serde_json::Value| v.as_str().map(String::from);
    if v.get("type").and_then(|t| t.as_str()) == Some("thread.started") {
        return v.get("thread_id").and_then(s);
    }
    if v.get("type").and_then(|t| t.as_str()) == Some("session_configured") {
        return v.get("session_id").and_then(s);
    }
    let msg = v.get("msg")?;
    (msg.get("type").and_then(|t| t.as_str()) == Some("session_configured"))
        .then(|| msg.get("session_id").and_then(s))
        .flatten()
}

/// Reads the events file at `path`: a regular file (checked before it is
/// opened, and opened non-blocking, so a FIFO fails at once), not named
/// `auth.json` as given or once resolved, and not under any of `refused_under`. At most 16 MiB and
/// 100,000 lines are read; no line is ever logged.
pub(super) fn read_events(
    path: &Path,
    refused_under: &[&Path],
) -> Result<EventsScan, ProfileError> {
    if path.file_name().is_some_and(|n| n == "auth.json") {
        return Err(refused(path, "a file named auth.json is never read"));
    }
    let meta = fs::metadata(path).map_err(|e| refused(path, &e.to_string()))?;
    if !meta.is_file() {
        return Err(refused(path, "not a regular file"));
    }
    let canon = fs::canonicalize(path).map_err(|e| refused(path, &e.to_string()))?;
    if canon.file_name().is_some_and(|n| n == "auth.json") {
        return Err(refused(path, "a file named auth.json is never read"));
    }
    for dir in refused_under {
        let dir_c = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if canon.starts_with(&dir_c) {
            return Err(refused(
                path,
                "it is inside a Codex account directory or the profiles root",
            ));
        }
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&canon)
        .map_err(|e| refused(path, &e.to_string()))?;
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(refused(path, "not a regular file"));
    }
    let mut reader = BufReader::new(file.take(MAX_EVENTS_BYTES));
    let mut scan = EventsScan {
        session: None,
        limit_hit: false,
    };
    let mut line = Vec::new();
    for _ in 0..MAX_EVENTS_LINES {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => return Err(refused(path, &e.to_string())),
        }
        let text = String::from_utf8_lossy(&line);
        match serde_json::from_str::<serde_json::Value>(text.trim()) {
            Ok(v) => {
                if scan.session.is_none() {
                    scan.session = session_of(&v);
                }
                scan.limit_hit = scan.limit_hit || limit_hit_event(&v);
            }
            Err(_) => scan.limit_hit = scan.limit_hit || limit_hit_text(&text),
        }
        if scan.session.is_some() && scan.limit_hit {
            break;
        }
    }
    Ok(scan)
}

/// What [`ensure_rollout`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Rollout {
    /// A symlink was made at this path, relative to the target home.
    Linked(PathBuf),
    /// The target home already has a rollout for the session.
    Present,
}

/// The first file under `home/sessions` whose name ends in `-<id>.jsonl`,
/// relative to `home`. Directories are listed only; symlinked directories
/// are not entered and no file is opened.
fn find_rollout(home: &Path, id: &str) -> io::Result<Option<PathBuf>> {
    let suffix = format!("-{id}.jsonl");
    let mut stack = vec![(PathBuf::from("sessions"), 0usize)];
    while let Some((rel, depth)) = stack.pop() {
        let entries = match fs::read_dir(home.join(&rel)) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        let mut names = Vec::new();
        for e in entries {
            let e = e?;
            names.push((e.file_name(), e.file_type()?));
        }
        names.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, kind) in names {
            if kind.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((rel.join(&name), depth + 1));
                }
            } else if name.to_str().is_some_and(|n| n.ends_with(&suffix)) {
                return Ok(Some(rel.join(name)));
            }
        }
    }
    Ok(None)
}

/// Makes the session's rollout reachable under `dst_home`: when no rollout
/// for `id` is there yet, the one under `src_home/sessions` is linked at the
/// same relative path, parents created with mode 0700; a symlinked folder
/// on that path is an error, never followed. No rollout under
/// `src_home` is `NotFound`, and nothing is created.
pub(super) fn ensure_rollout(src_home: &Path, dst_home: &Path, id: &str) -> io::Result<Rollout> {
    if find_rollout(dst_home, id)?.is_some() {
        return Ok(Rollout::Present);
    }
    let Some(rel) = find_rollout(src_home, id)? else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no rollout for the session under the account it used",
        ));
    };
    let mut dir = dst_home.to_path_buf();
    for c in rel.parent().unwrap_or(Path::new("")).components() {
        dir.push(c);
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            // A symlinked folder is never followed: the rollout link stays
            // inside the target account.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "a folder in the rollout path is a symlink or not a folder",
                    ));
                }
            }
            Err(e) => return Err(e),
        }
    }
    symlink(src_home.join(&rel), dst_home.join(&rel))?;
    Ok(Rollout::Linked(rel))
}
