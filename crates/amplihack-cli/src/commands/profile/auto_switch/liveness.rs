//! Which profile directories a running process is using, from
//! `/proc/<pid>/environ`. A directory is live when a process of the same user,
//! other than this one and not a zombie, has `CLAUDE_CONFIG_DIR` resolving to
//! it. Only processes with this process's real user ID are read.
//!
//! Issue #2820: the decision is per profile, on what is known. A process
//! whose environment cannot be read is judged by `unreadable::judge` from its
//! command line and `status` name: a Claude or amplihack executable makes
//! doubtful, and so live, only the profiles its arguments name or that hold
//! Claude Code's `sessions/<pid>.json` for it, and any other is no evidence.
//! One tied to no profile protects none; `unreadable.rs` says why that is
//! the remaining gap. One unreadable process never makes every profile live;
//! only a process table that cannot be used at all does
//! ([`Liveness::Unsupported`]). Every unreadable process the scan noted is
//! kept in [`Liveness::Partial`] so the rotate log can name it.
//!
//! The decisions that depend on this take a [`SessionView`] (issue #2935),
//! so a test of a decision supplies the answer and reads no process table.
//! [`HostSessions`] is the host's view. A system with no `/proc` has none,
//! and its answer is [`Liveness::Unsupported`].

use super::fsx;
use super::unreadable::{self, Unreadable, Verdict};
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MAX_ENVIRON: u64 = 16 * 1024 * 1024;
const MAX_SMALL: u64 = 64 * 1024;
const KEY: &[u8] = b"CLAUDE_CONFIG_DIR=";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// Profile names with a live process.
    Known(BTreeSet<String>),
    /// Issue #2820: as `Known`, after some processes' environments could
    /// not be read. `live` holds the names readable processes gave and the
    /// names unreadable Claude or amplihack executables were tied to by
    /// their arguments or session files (doubtful counts as live). `unreadable` is every unreadable process the scan
    /// noted, in pid order, and is never empty. It is for the log only and
    /// never changes a decision.
    Partial {
        live: BTreeSet<String>,
        unreadable: Vec<Unreadable>,
    },
    /// `/proc` cannot be used here.
    Unsupported,
}

impl Liveness {
    pub fn is_live(&self, name: &ProfileName) -> bool {
        match self {
            Self::Known(set) | Self::Partial { live: set, .. } => set.contains(name.as_str()),
            Self::Unsupported => true,
        }
    }

    /// The unreadable processes the scan noted, in pid order; empty unless
    /// [`Liveness::Partial`].
    pub fn unreadable(&self) -> &[Unreadable] {
        match self {
            Self::Partial { unreadable, .. } => unreadable,
            Self::Known(_) | Self::Unsupported => &[],
        }
    }

    pub fn supported(&self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

/// The view of running sessions: which profiles under a profile root a
/// running process is using. One question, so that the code that decides
/// what to move never reads a process table itself.
pub trait SessionView {
    fn liveness(&self, root: &Path) -> Liveness;
}

/// A function is a view. `rotate`'s cycle context holds one (`Ctx::scan`),
/// and a test sets it to a function that returns a table the test wrote.
impl SessionView for fn(&Path) -> Liveness {
    fn liveness(&self, root: &Path) -> Liveness {
        self(root)
    }
}

/// The host's view: the process table [`readable_table`] names. That is
/// `/proc` on Linux. A system without `/proc` has no table and the answer is
/// `Unsupported`, unless a test supplied a synthetic table through the
/// process-table seam, which is the only way a test that starts the binary
/// can supply one.
///
/// [`readable_table`]: crate::commands::agent::procfs::readable_table
pub struct HostSessions;

impl SessionView for HostSessions {
    fn liveness(&self, root: &Path) -> Liveness {
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        seen_in(
            crate::commands::agent::procfs::readable_table().as_deref(),
            root,
            std::process::id(),
            uid,
        )
    }
}

/// The pure part of [`HostSessions`]: the sessions under `root` that the
/// process table `table` shows. With no table, sessions cannot be seen.
pub(super) fn seen_in(table: Option<&Path>, root: &Path, self_pid: u32, uid: u32) -> Liveness {
    let Some(table) = table else {
        return Liveness::Unsupported;
    };
    match root.canonicalize() {
        Ok(root) => scan(table, &root, self_pid, uid),
        Err(_) => Liveness::Unsupported,
    }
}

/// A view a test holds in memory: the answer it was built with, whatever
/// the root.
#[cfg(test)]
pub struct FakeSessions(pub Liveness);

#[cfg(test)]
impl FakeSessions {
    /// Sessions can be seen, and exactly `names` have one.
    pub fn running(names: &[&str]) -> Self {
        Self(Liveness::Known(
            names.iter().map(|n| (*n).to_string()).collect(),
        ))
    }

    /// The answer of a system where sessions cannot be seen.
    pub fn unseen() -> Self {
        Self(Liveness::Unsupported)
    }
}

#[cfg(test)]
impl SessionView for FakeSessions {
    fn liveness(&self, _root: &Path) -> Liveness {
        self.0.clone()
    }
}

/// [`HostSessions`] for `root`, for a caller that holds no view of its own.
pub fn scan_system(root: &Path) -> Liveness {
    HostSessions.liveness(root)
}

/// `root` must be canonical.
pub fn scan(proc_root: &Path, root: &Path, self_pid: u32, uid: u32) -> Liveness {
    if !matches!(
        fsx::read_bounded_nofollow(&proc_root.join("self/environ"), MAX_ENVIRON),
        Ok(Some(_))
    ) {
        return Liveness::Unsupported;
    }
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Liveness::Unsupported;
    };
    let mut live = BTreeSet::new();
    let mut noted: Vec<Unreadable> = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let dir = entry.path();
        let Some((state, owner, comm)) = status(&dir) else {
            continue; // gone
        };
        if owner != uid || state == 'Z' {
            continue;
        }
        let environ = match fsx::read_bounded_nofollow(&dir.join("environ"), MAX_ENVIRON) {
            Ok(Some((raw, _))) => raw,
            Ok(None) => continue, // exited
            Err(_) => {
                // Not dumpable, changed credentials, or over the bound.
                if let Some(u) = unreadable::judge(&dir, pid, &comm, root) {
                    if let Verdict::Counted(names) = &u.verdict {
                        live.extend(names.iter().cloned());
                    }
                    noted.push(u);
                }
                continue;
            }
        };
        if let Some(value) = environ.split(|&b| b == 0).find_map(|e| e.strip_prefix(KEY))
            && let Some(name) = resolve(&dir, value, root)
        {
            live.insert(name);
        }
    }
    if noted.is_empty() {
        return Liveness::Known(live);
    }
    noted.sort_by_key(|u| u.pid);
    Liveness::Partial {
        live,
        unreadable: noted,
    }
}

/// The `CLAUDE_CONFIG_DIR` of every process of `uid` other than `self_pid`
/// that is not a zombie, for the session archive's live check (issue #1746).
/// A relative value is taken from the process's working directory.
/// Only the value is kept. Fails closed: an unreadable `proc_root`, or a
/// Claude or amplihack executable (`unreadable::claude_executable`, judged
/// by its `status` name and `argv[0]`, never by other text in its command
/// line) whose environment or working directory cannot be read, is an
/// error. The archive cannot tell which directory such a process uses, and
/// the cost is one deferred archive. Other unreadable processes (such as a
/// non-dumpable `ssh-agent` or `tailscaled`) and processes that exited are
/// skipped.
pub(crate) fn claude_config_dirs(
    proc_root: &Path,
    self_pid: u32,
    uid: u32,
) -> std::io::Result<Vec<PathBuf>> {
    use std::os::unix::ffi::OsStrExt;
    let entries = std::fs::read_dir(proc_root)?;
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let dir = entry.path();
        if pid == self_pid {
            continue;
        }
        let comm = match status(&dir) {
            Some((state, owner, comm)) if owner == uid && state != 'Z' => comm,
            _ => continue,
        };
        let environ = match fsx::read_bounded_nofollow(&dir.join("environ"), MAX_ENVIRON) {
            Ok(Some((raw, _))) => raw,
            Ok(None) => continue, // exited
            Err(e) if unreadable::claude_executable(&comm, &unreadable::argv(&dir)) => {
                return Err(e);
            }
            Err(_) => continue,
        };
        let value = environ
            .split(|&b| b == 0)
            .find_map(|e| e.strip_prefix(KEY))
            .filter(|v| !v.is_empty())
            .map(|v| PathBuf::from(std::ffi::OsStr::from_bytes(v)));
        drop(environ);
        match value {
            Some(p) if p.is_absolute() => dirs.push(p),
            Some(p) => match std::fs::read_link(dir.join("cwd")) {
                Ok(cwd) => dirs.push(cwd.join(p)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // exited
                Err(e) => return Err(e),
            },
            None => {}
        }
    }
    Ok(dirs)
}

/// `(state letter, real uid, name)` from `/proc/<pid>/status`. The name is
/// the `Name:` field (the kernel's `comm`), empty when there is none.
fn status(dir: &Path) -> Option<(char, u32, String)> {
    let (raw, _) = fsx::read_bounded_nofollow(&dir.join("status"), MAX_SMALL).ok()??;
    let text = String::from_utf8_lossy(&raw);
    let mut state = None;
    let mut uid = None;
    let mut name = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Name:") {
            name = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("State:") {
            state = v.trim().chars().next();
        } else if let Some(v) = line.strip_prefix("Uid:") {
            uid = v.split_whitespace().next().and_then(|u| u.parse().ok());
        }
    }
    Some((state?, uid?, name))
}

/// The profile name `value` points at, if it is a directory under `root`.
/// A path that cannot be resolved falls back to its last component.
fn resolve(dir: &Path, value: &[u8], root: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(value));
    let path: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::fs::read_link(dir.join("cwd")).ok()?.join(path)
    };
    match path.canonicalize() {
        Ok(canonical) => {
            if canonical.parent() != Some(root) {
                return None;
            }
            canonical.file_name()?.to_str().map(str::to_string)
        }
        // fail-safe: unresolvable → treated as live. A wrong name here can
        // only keep a directory counted as live, which blocks a swap.
        Err(_) => path.file_name()?.to_str().map(str::to_string),
    }
}
