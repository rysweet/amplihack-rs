//! Which sessions are running. The main safety property of `sessions
//! archive`: a session whose pid file says it is live is never archived,
//! whichever Claude config directory holds the pid file.
//!
//! Sources: `<HOME>/.claude/sessions`, `<profile>/sessions` for every
//! profile (profiles share `projects/` but each has its own `sessions/`),
//! and `<CLAUDE_CONFIG_DIR>/sessions` of this user's processes. The build
//! fails closed: only a missing directory counts as empty.

use super::fmt::esc;
use super::scan::valid_name;
use crate::commands::profile::{ProfileError, ProfileStore};
use amplihack_utils::secure_files::read_bounded_nofollow;
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MAX_PID_FILE: u64 = 64 * 1024;

/// What the live check asks the system; a fake in tests.
pub trait Probe {
    /// Whether `pid` (always in `1..=i32::MAX`) is a running process.
    fn alive(&self, pid: i32) -> bool;
    /// `CLAUDE_CONFIG_DIR` of this user's running processes; an error when
    /// a Claude-like process cannot be read, so the build fails closed.
    fn config_dirs(&self) -> io::Result<Vec<PathBuf>>;
}

/// `kill(pid, 0)` and, on Linux, `/proc`.
pub struct SystemProbe;

impl Probe for SystemProbe {
    fn alive(&self, pid: i32) -> bool {
        if pid < 1 {
            return false;
        }
        // SAFETY: signal 0 only checks that `pid` exists; `pid` is positive,
        // so this never addresses a process group or every process.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        // EPERM: it exists but belongs to someone else. Only ESRCH is dead.
        io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    fn config_dirs(&self) -> io::Result<Vec<PathBuf>> {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: getuid has no preconditions.
            let uid = unsafe { libc::getuid() };
            crate::commands::profile::claude_config_dirs(
                Path::new("/proc"),
                std::process::id(),
                uid,
            )
        }
        #[cfg(not(target_os = "linux"))]
        Ok(Vec::new())
    }
}

/// Session ids with a live pid file, or registered to an agent that is not
/// `ended`.
#[derive(Debug, Default, Clone)]
pub struct LiveSet {
    ids: HashSet<String>,
    /// Pid files whose body did not parse or named no valid session.
    pub ignored: BTreeSet<PathBuf>,
}

impl LiveSet {
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// One line per ignored pid file. Called after the first build of a
    /// run only, so a refresh does not repeat the lines.
    pub fn report_ignored(&self, warn: &mut dyn FnMut(&str)) {
        for p in &self.ignored {
            warn(&format!("ignored unreadable pid file {}", esc(p)));
        }
    }
}

/// Why a live-session source could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// An I/O error, shown as its kind.
    Kind(io::ErrorKind),
    /// The agent registry's own message, which names the file or directory
    /// at fault and says what is wrong with it.
    Agents(String),
}

impl From<io::ErrorKind> for Why {
    fn from(k: io::ErrorKind) -> Self {
        Self::Kind(k)
    }
}

impl PartialEq<io::ErrorKind> for Why {
    fn eq(&self, k: &io::ErrorKind) -> bool {
        *self == Self::Kind(*k)
    }
}

/// The path that could not be read and why.
pub type LiveError = (PathBuf, Why);

/// The `sessions/` directory of every `CLAUDE_CONFIG_DIR` that this user's
/// running processes set. This is the costly source (a walk of `/proc`), so
/// the archive repeats it less often than the others.
pub fn process_dirs(probe: &dyn Probe) -> Result<Vec<PathBuf>, LiveError> {
    let dirs = probe
        .config_dirs()
        .map_err(|e| (PathBuf::from("/proc"), e.kind().into()))?;
    Ok(dirs.into_iter().map(|d| d.join("sessions")).collect())
}

/// The live set from every source: the home and profile `sessions/`
/// directories, read now, `process_dirs` from [`process_dirs`], and the
/// agent registry under `home`. Any error but a missing directory or
/// registry fails closed.
pub fn build_from(
    home: &Path,
    profile_root: &Path,
    process_dirs: &[PathBuf],
    probe: &dyn Probe,
) -> Result<LiveSet, LiveError> {
    let mut dirs = vec![home.join(".claude/sessions")];
    dirs.extend(
        profiles(profile_root)?
            .into_iter()
            .map(|n| profile_root.join(n).join("sessions")),
    );
    dirs.extend(process_dirs.iter().cloned());
    let mut seen = BTreeSet::new();
    let mut set = LiveSet::default();
    for dir in dirs {
        let canonical = match dir.canonicalize() {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err((dir, e.kind().into())),
        };
        if seen.insert(canonical.clone()) {
            read_dir_into(&dir, &canonical, probe, &mut set)?;
        }
    }
    set.ids
        .extend(crate::commands::agent::unended_conversations(home).map_err(from_agents)?);
    Ok(set)
}

/// The agent registry's error as this module's: an I/O error by kind, a
/// refusal by the path it names and its problem, without the path again.
fn from_agents(e: crate::commands::agent::Unreadable) -> LiveError {
    use crate::commands::agent::Unreadable;
    match e {
        Unreadable::Io(path, kind) => (path, kind.into()),
        Unreadable::Refused(r) => (r.path, Why::Agents(r.problem)),
    }
}

/// Profile names in the root, as `profile list` sees them.
fn profiles(root: &Path) -> Result<Vec<String>, LiveError> {
    ProfileStore::new(root.to_path_buf())
        .list()
        .map_err(|e| match e {
            ProfileError::Io(e) => (root.to_path_buf(), e.kind().into()),
            _ => (root.to_path_buf(), io::ErrorKind::InvalidData.into()),
        })
}

fn read_dir_into(
    shown: &Path,
    dir: &Path,
    probe: &dyn Probe,
    set: &mut LiveSet,
) -> Result<(), LiveError> {
    let fail = |e: io::Error| -> LiveError { (shown.to_path_buf(), e.kind().into()) };
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(fail(e)),
    };
    for entry in entries {
        let entry = entry.map_err(fail)?;
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
            continue;
        };
        // DirEntry::file_type does not follow symlinks.
        if !entry.file_type().map_err(fail)?.is_file() {
            continue;
        }
        let raw = match read_bounded_nofollow(&entry.path(), MAX_PID_FILE) {
            Ok(Some((raw, _))) => raw,
            // Gone since the listing.
            Ok(None) => continue,
            // A symlink, a non-regular file or one over the limit.
            Err(e) if e.kind() == io::ErrorKind::InvalidData => continue,
            Err(e) => return Err((entry.path(), e.kind().into())),
        };
        match live_id(&raw, stem, probe) {
            Ok(Some(id)) => {
                set.ids.insert(id);
            }
            Ok(None) => {}
            Err(()) => {
                set.ignored.insert(shown.join(&name));
            }
        }
    }
    Ok(())
}

/// The session id of a pid file whose body pid or numeric stem is alive.
/// A body that does not parse, or names no valid session, protects nothing
/// and is `Err`.
fn live_id(raw: &[u8], stem: &str, probe: &dyn Probe) -> Result<Option<String>, ()> {
    let v: serde_json::Value = serde_json::from_slice(raw).map_err(|_| ())?;
    let id = v.get("sessionId").and_then(|i| i.as_str()).ok_or(())?;
    if !valid_name(id) {
        return Err(());
    }
    let body = v
        .get("pid")
        .and_then(serde_json::Value::as_i64)
        .and_then(in_range);
    let from_stem = (!stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit()))
        .then(|| stem.parse::<i64>().ok().and_then(in_range))
        .flatten();
    let mut pids: Vec<i32> = body.into_iter().chain(from_stem).collect();
    pids.dedup();
    Ok(pids
        .into_iter()
        .any(|p| probe.alive(p))
        .then(|| id.to_string()))
}

fn in_range(pid: i64) -> Option<i32> {
    i32::try_from(pid).ok().filter(|&p| p >= 1)
}
