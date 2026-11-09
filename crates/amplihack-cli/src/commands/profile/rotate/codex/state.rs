//! `<rotate dir>/codex-state.json`: the Codex swap journal, the time of the
//! last Codex swap and the `codex-resume` records; and the Codex lock,
//! `<rotate dir>/lock`. The Claude ledger and journal never hold Codex
//! data.

use crate::commands::profile::auto_switch::kit::{
    euid, read_bounded_nofollow, write_private_atomic,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

pub(super) const FILE: &str = "codex-state.json";
const VERSION: u32 = 1;
/// The largest state file read.
const MAX_STATE: u64 = 256 * 1024;
/// Resume records older than this are dropped.
const WINDOW_SECS: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum CodexPhase {
    Prepared,
    Linked,
}

impl CodexPhase {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Linked => "linked",
        }
    }
}

/// A Codex swap in progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CodexJournal {
    pub from: String,
    pub to: String,
    pub link: PathBuf,
    pub old_target: PathBuf,
    pub new_target: PathBuf,
    pub from_ino: u64,
    pub to_ino: u64,
    pub phase: CodexPhase,
}

/// One `codex-resume` of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ResumeRecord {
    pub session: String,
    pub at: i64,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CodexState {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<CodexJournal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_swap_at: Option<i64>,
    #[serde(default)]
    pub resumes: Vec<ResumeRecord>,
}

impl Default for CodexState {
    fn default() -> Self {
        Self {
            version: VERSION,
            journal: None,
            last_swap_at: None,
            resumes: Vec::new(),
        }
    }
}

impl CodexState {
    /// This session's resumes in the 24 hours before `now`.
    pub(super) fn resumes_in_window(&self, session: &str, now: i64) -> u32 {
        let n = self
            .resumes
            .iter()
            .filter(|r| r.session == session && in_window(r.at, now))
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }
}

fn in_window(at: i64, now: i64) -> bool {
    now.saturating_sub(at) < WINDOW_SECS
}

fn unreadable(why: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{FILE}: {why}"))
}

/// The state in `dir`: the default when there is no file. A file that is
/// not a regular file, is not ours, is wider than 0600, does not parse or
/// has another version is `InvalidData`, and is left as it is.
pub(super) fn load(dir: &Path) -> io::Result<CodexState> {
    let (raw, meta) = match read_bounded_nofollow(&dir.join(FILE), MAX_STATE) {
        Ok(None) => return Ok(CodexState::default()),
        Ok(Some(found)) => found,
        Err(e) => return Err(unreadable(e)),
    };
    if meta.uid() != euid() {
        return Err(unreadable("owned by another user"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(unreadable("not private (mode 0600 or narrower)"));
    }
    let s: CodexState = serde_json::from_slice(&raw)
        .map_err(|e| unreadable(format!("line {} column {}", e.line(), e.column())))?;
    if s.version != VERSION {
        return Err(unreadable("unknown version"));
    }
    Ok(s)
}

/// Writes `s` to `dir` atomically with mode 0600, never through a symlink,
/// after dropping resume records older than 24 hours.
pub(super) fn save(dir: &Path, s: &CodexState, now: i64) -> io::Result<()> {
    let mut s = s.clone();
    s.resumes.retain(|r| in_window(r.at, now));
    let bytes = serde_json::to_vec_pretty(&s).map_err(unreadable)?;
    write_private_atomic(dir, FILE, &bytes)
}

/// The Codex lock, held until dropped.
pub(super) struct CodexLock {
    _file: fs::File,
}

/// Takes `<dir>/lock` without waiting: `None` when another process holds
/// it. The lock file must be a regular file of ours.
pub(super) fn try_lock(dir: &Path) -> io::Result<Option<CodexLock>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(dir.join("lock"))?;
    let m = file.metadata()?;
    if !m.is_file() || m.uid() != euid() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "lock is not a private regular file",
        ));
    }
    // SAFETY: flock on a descriptor this function owns.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(Some(CodexLock { _file: file }));
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(None)
    } else {
        Err(e)
    }
}
