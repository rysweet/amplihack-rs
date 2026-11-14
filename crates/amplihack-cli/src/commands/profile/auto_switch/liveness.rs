//! Which profile directories a running process is using, from
//! `/proc/<pid>/environ`. A directory is live when a process of the same user,
//! other than this one and not a zombie, that may be Claude has
//! `CLAUDE_CONFIG_DIR` resolving to it. Any doubt counts as live.
//!
//! Issue #2040 splits the scan: [`collect`] records each same-user process
//! as a [`ProcRecord`] that the shared snapshot can hold, and [`derive`]
//! turns the records into a [`Liveness`] for one reader. Production reads go
//! through `profile::proc_snapshot::ProcSource`.

use super::fsx;
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
    /// A Claude-like process's environment could not be read.
    Everything,
    /// `/proc` cannot be used here.
    Unsupported,
}

impl Liveness {
    pub fn is_live(&self, name: &ProfileName) -> bool {
        match self {
            Self::Known(set) => set.contains(name.as_str()),
            Self::Everything | Self::Unsupported => true,
        }
    }

    pub fn supported(&self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

/// One same-user process as the scan saw it (issue #2040, D2). Only these
/// scalar fields are kept: no exe path, command line, environment or raw
/// `CLAUDE_CONFIG_DIR` value, so a record can be written to the shared
/// snapshot as it is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProcRecord {
    pub pid: u32,
    pub zombie: bool,
    /// The exe or command line names Claude or amplihack.
    pub claude_like: bool,
    /// The profile directory under the root its `CLAUDE_CONFIG_DIR` names.
    #[serde(default)]
    pub profile: Option<String>,
    /// A Claude-like process whose environment could not be read.
    pub environ_unreadable: bool,
}

/// What one pass over `/proc` found, before the reader drops itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collected {
    /// `/proc` cannot be used here.
    Unsupported,
    /// Every process of the user, the caller included.
    Procs(Vec<ProcRecord>),
}

/// `root` must be canonical. The live set as `self_pid` sees it. Production
/// reads go through the snapshot; the tests use this directly.
#[cfg(test)]
pub fn scan(proc_root: &Path, root: &Path, self_pid: u32, uid: u32) -> Liveness {
    derive(&collect(proc_root, root, uid), self_pid)
}

/// The answer `reader_pid` draws from `collected` (D4): a process never
/// counts itself, a zombie is never live, and another process that looks
/// like Claude but whose environment could not be read makes every
/// directory live.
pub fn derive(collected: &Collected, reader_pid: u32) -> Liveness {
    let procs = match collected {
        Collected::Unsupported => return Liveness::Unsupported,
        Collected::Procs(procs) => procs,
    };
    let others = || procs.iter().filter(|r| r.pid != reader_pid);
    if others().any(|r| r.claude_like && r.environ_unreadable) {
        return Liveness::Everything;
    }
    Liveness::Known(
        others()
            .filter(|r| !r.zombie)
            .filter_map(|r| r.profile.clone())
            .collect(),
    )
}

/// Every process of `uid` under `proc_root`, the caller included. `root`
/// must be canonical. For each process the reads are, in order: `status`
/// (skipped unless it is the user's), the `exe` link, `cmdline`
/// only for an interpreter or an unreadable `exe`, and `environ` only for a
/// [`candidate`] (R7, D9).
pub fn collect(proc_root: &Path, root: &Path, uid: u32) -> Collected {
    // hidepid: our own environment must be readable for the rest to mean
    // anything.
    if !matches!(
        fsx::read_bounded_nofollow(&proc_root.join("self/environ"), MAX_ENVIRON),
        Ok(Some(_))
    ) {
        return Collected::Unsupported;
    }
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Collected::Unsupported;
    };
    let mut procs = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(r) = record(&entry.path(), pid, root, uid) {
            procs.push(r);
        }
    }
    Collected::Procs(procs)
}

/// One process's record, or `None` when it is another user's, has exited
/// or its `status` cannot be parsed.
fn record(dir: &Path, pid: u32, root: &Path, uid: u32) -> Option<ProcRecord> {
    let st = status(dir)?;
    if st.uid != uid {
        return None;
    }
    let mut r = ProcRecord {
        pid,
        zombie: st.state == 'Z',
        claude_like: false,
        profile: None,
        environ_unreadable: false,
    };
    if r.zombie {
        return Some(r);
    }
    let exe = std::fs::read_link(dir.join("exe")).ok();
    let cmdline = || match fsx::read_bounded_nofollow(&dir.join("cmdline"), MAX_SMALL) {
        Ok(Some((raw, _))) => Some(raw),
        _ => None,
    };
    if !candidate(exe.as_deref(), cmdline) {
        return Some(r);
    }
    // A readable exe that passed the gate names Claude, or is an interpreter
    // running it (or whose arguments could not be read). An unreadable exe
    // is judged by its command line, as before.
    r.claude_like = exe.is_some() || looks_like_claude(dir);
    match fsx::read_bounded_nofollow(&dir.join("environ"), MAX_ENVIRON) {
        Ok(Some((environ, _))) => {
            r.profile = environ
                .split(|&b| b == 0)
                .find_map(|e| e.strip_prefix(KEY))
                .and_then(|value| resolve(dir, value, root));
        }
        Ok(None) => return None, // exited
        Err(e) if exited_mid_read(&e) => return None,
        Err(_) => r.environ_unreadable = r.claude_like,
    }
    Some(r)
}

/// The process exited between its `exe` read and its `environ` read: the
/// kernel answers `ESRCH`. Such a process is gone, not unreadable, so it
/// must not make every directory live (and stay so in the snapshot for 30
/// seconds). A refused read (`EACCES`, `EPERM`) is still unreadable.
pub(crate) fn exited_mid_read(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::ESRCH)
}

/// Whether a process whose `exe` link reads `exe` (`None`: unreadable) may
/// be a Claude session, so its environment is worth reading: any component
/// of the exe names Claude or amplihack, the exe cannot be read, or the
/// exe's base name is `node`, `bun` or `deno` and `cmdline` names Claude or
/// amplihack or cannot be read (`None`). `cmdline` is called only in that
/// last case. A `" (deleted)"` suffix on the exe is ignored.
pub fn candidate(exe: Option<&Path>, cmdline: impl FnOnce() -> Option<Vec<u8>>) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Some(exe) = exe else {
        return true;
    };
    if exe
        .components()
        .any(|c| has_marker(c.as_os_str().as_bytes()))
    {
        return true;
    }
    let base = exe.file_name().map_or(&b""[..], |b| b.as_bytes());
    let base = base.strip_suffix(b" (deleted)").unwrap_or(base);
    if !matches!(base, b"node" | b"bun" | b"deno") {
        return false;
    }
    cmdline().is_none_or(|raw| has_marker(&raw))
}

/// The bytes name Claude or amplihack, ignoring ASCII case.
fn has_marker(raw: &[u8]) -> bool {
    [&b"claude"[..], &b"amplihack"[..]]
        .iter()
        .any(|m| raw.windows(m.len()).any(|w| w.eq_ignore_ascii_case(m)))
}

/// The `CLAUDE_CONFIG_DIR` of every process of `uid` other than `self_pid`
/// that is not a zombie, for the session archive's live check (issue #1746).
/// A relative value is taken from the process's working directory.
/// Only the value is kept. Fails closed, like [`scan`]: an unreadable
/// `proc_root`, or a Claude-like process whose environment or working
/// directory cannot be read, is an error. Other unreadable processes (such
/// as a non-dumpable `ssh-agent`) and processes that exited are skipped.
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
        if pid == self_pid || !matches!(status(&dir), Some(st) if st.uid == uid && st.state != 'Z')
        {
            continue;
        }
        let environ = match fsx::read_bounded_nofollow(&dir.join("environ"), MAX_ENVIRON) {
            Ok(Some((raw, _))) => raw,
            Ok(None) => continue, // exited
            Err(e) if looks_like_claude(&dir) => return Err(e),
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

/// Whether `pid` may still be a running process of `uid` under
/// `proc_root`: false only when it has certainly exited (no `/proc/<pid>`,
/// or the kernel answers `ESRCH`), is a zombie, or now belongs to another
/// user. Any other doubt answers true. A snapshot record whose process
/// fails this no longer makes a directory live (issue #2040): a session that
/// ended must not keep its borrowed login away for the rest of the
/// freshness period. A reused pid of the same user still counts, which
/// errs towards live.
pub fn may_be_running(proc_root: &Path, pid: u32, uid: u32) -> bool {
    let path = proc_root.join(pid.to_string()).join("status");
    match fsx::read_bounded_nofollow(&path, MAX_SMALL) {
        Ok(None) => false,
        Ok(Some((raw, _))) => match parse_status(&raw) {
            Some(st) => st.uid == uid && st.state != 'Z',
            None => true,
        },
        Err(e) => !exited_mid_read(&e),
    }
}

struct Status {
    state: char,
    uid: u32,
}

/// State letter and real uid from `/proc/<pid>/status`. A missing or
/// unparsable state or uid is `None`.
fn status(dir: &Path) -> Option<Status> {
    let (raw, _) = fsx::read_bounded_nofollow(&dir.join("status"), MAX_SMALL).ok()??;
    parse_status(&raw)
}

fn parse_status(raw: &[u8]) -> Option<Status> {
    let text = String::from_utf8_lossy(raw);
    let mut state = None;
    let mut uid = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("State:") {
            state = v.trim().chars().next();
        } else if let Some(v) = line.strip_prefix("Uid:") {
            uid = v.split_whitespace().next().and_then(|u| u.parse().ok());
        }
    }
    Some(Status {
        state: state?,
        uid: uid?,
    })
}

fn looks_like_claude(dir: &Path) -> bool {
    match fsx::read_bounded_nofollow(&dir.join("cmdline"), MAX_SMALL) {
        Ok(Some((raw, _))) => {
            let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
            text.contains("claude") || text.contains("amplihack")
        }
        _ => false,
    }
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
