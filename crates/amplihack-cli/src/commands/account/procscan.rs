//! Counting running jobs per account from the process table (issue #2006).
//!
//! A process matches an account when its exec-time environment
//! (`/proc/<pid>/environ`) sets the key (`CODEX_HOME` or
//! `CLAUDE_CONFIG_DIR`) to a value that canonicalises to the account home.
//! A matching process is a job when its parent does not match the same home,
//! so a launch and the many children it starts count once.
//!
//! Only processes of the given real uid are considered, and that is checked
//! in `status` before `environ` is opened. Only the requested key's value is
//! kept; the rest of the environment is dropped as soon as it is searched.
//! The scanner's own pid, zombies and unreadable entries are skipped. On
//! platforms other than Linux nothing is counted.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The largest `environ` read; a larger one is treated as unreadable.
const MAX_ENVIRON: u64 = 4 * 1024 * 1024;
/// The largest `status` or `stat` read.
const MAX_SMALL: u64 = 64 * 1024;
/// The longest value considered as a home, in bytes.
const PATH_MAX: usize = 4096;
/// The longest parent chain followed.
const MAX_ANCESTORS: usize = 64;

/// One process of the scanned user.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    /// The value of the requested key, when the process sets it.
    pub matched: Option<OsString>,
}

/// One job: a matching process whose parent does not match the same home.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub pid: u32,
    /// The canonical candidate home it matched.
    pub home: PathBuf,
    /// The parent chain, nearest first.
    pub ancestors: Vec<u32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobCounts {
    pub jobs: Vec<Job>,
}

impl JobCounts {
    /// The number of jobs on `home`.
    pub fn count(&self, home: &Path) -> usize {
        self.jobs.iter().filter(|j| j.home == home).count()
    }
}

/// Reads at most `max` bytes of the regular file `path` without following
/// a symlink and without blocking on a named pipe. `None` when it is not a
/// regular file, cannot be opened or read, or is larger than `max`.
pub(super) fn read_bounded_nofollow(path: &Path, max: u64) -> Option<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let file = open_nofollow(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut buf = Vec::new();
    file.take(max + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > max {
        return None;
    }
    Some(buf)
}

#[cfg(unix)]
fn open_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(not(unix))]
fn open_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// The processes of real uid `uid` under `proc_root`, except `self_pid`
/// and zombies, each with the value of `key` if it sets one.
/// Empty when `proc_root` cannot be listed.
pub fn walk(proc_root: &Path, key: &str, self_pid: u32, uid: u32) -> Vec<ProcEntry> {
    scan(proc_root, key, self_pid, uid).unwrap_or_default()
}

/// As [`walk`], but an error when `proc_root` itself cannot be listed. A
/// process that exits during the scan is skipped, not an error.
fn scan(proc_root: &Path, key: &str, self_pid: u32, uid: u32) -> std::io::Result<Vec<ProcEntry>> {
    #[cfg(target_os = "linux")]
    {
        walk_linux(proc_root, key, self_pid, uid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (proc_root, key, self_pid, uid);
        Ok(Vec::new())
    }
}

#[cfg(target_os = "linux")]
fn walk_linux(
    proc_root: &Path,
    key: &str,
    self_pid: u32,
    uid: u32,
) -> std::io::Result<Vec<ProcEntry>> {
    let rd = std::fs::read_dir(proc_root)?;
    let mut prefix = key.as_bytes().to_vec();
    prefix.push(b'=');
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let dir = entry.path();
        let Some((real_uid, zombie)) = read_status(&dir) else {
            continue;
        };
        // The uid is checked before `environ` is opened.
        if real_uid != uid || zombie {
            continue;
        }
        let Some(ppid) = read_ppid(&dir) else {
            continue;
        };
        let matched = read_bounded_nofollow(&dir.join("environ"), MAX_ENVIRON)
            .and_then(|environ| find_value(&environ, &prefix));
        out.push(ProcEntry { pid, ppid, matched });
    }
    out.sort_by_key(|e| e.pid);
    Ok(out)
}

/// The real uid and whether the process is a zombie, from `status`.
#[cfg(target_os = "linux")]
fn read_status(dir: &Path) -> Option<(u32, bool)> {
    let raw = read_bounded_nofollow(&dir.join("status"), MAX_SMALL)?;
    let text = String::from_utf8_lossy(&raw);
    let mut uid = None;
    let mut zombie = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            uid = rest.split_whitespace().next()?.parse::<u32>().ok();
        } else if let Some(rest) = line.strip_prefix("State:") {
            zombie = rest.trim_start().starts_with('Z');
        }
    }
    Some((uid?, zombie))
}

/// The parent pid from `stat`, read after the last `)` so a command name
/// containing `)` cannot shift the fields.
#[cfg(target_os = "linux")]
fn read_ppid(dir: &Path) -> Option<u32> {
    let raw = read_bounded_nofollow(&dir.join("stat"), MAX_SMALL)?;
    let close = raw.iter().rposition(|&b| b == b')')?;
    let rest = std::str::from_utf8(&raw[close + 1..]).ok()?;
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse::<u32>().ok()
}

/// The value of the first `KEY=` entry in a NUL-separated environment.
#[cfg(target_os = "linux")]
fn find_value(environ: &[u8], prefix: &[u8]) -> Option<OsString> {
    use std::os::unix::ffi::OsStrExt;
    environ
        .split(|&b| b == 0)
        .find_map(|e| e.strip_prefix(prefix))
        .map(|v| std::ffi::OsStr::from_bytes(v).to_os_string())
}

/// The jobs on each of `homes` (canonical paths) under `proc_root`, and
/// the warnings to print. When `proc_root` cannot be listed there are no
/// jobs and one warning, so the pick goes ahead on leases alone.
pub fn count_jobs(
    proc_root: &Path,
    key: &str,
    homes: &[PathBuf],
    self_pid: u32,
    uid: u32,
) -> (JobCounts, Vec<String>) {
    let entries = match scan(proc_root, key, self_pid, uid) {
        Ok(entries) => entries,
        Err(e) => {
            let warning = format!(
                "warning: account pick: job counting unavailable: could not list {}: {e}",
                proc_root.display()
            );
            return (JobCounts::default(), vec![warning]);
        }
    };
    let mut canon: HashMap<OsString, Option<PathBuf>> = HashMap::new();
    let mut home_of: HashMap<u32, PathBuf> = HashMap::new();
    let mut ppid_of: HashMap<u32, u32> = HashMap::new();
    for e in &entries {
        ppid_of.insert(e.pid, e.ppid);
        let Some(value) = &e.matched else { continue };
        let resolved = canon
            .entry(value.clone())
            .or_insert_with(|| candidate_home(value, homes))
            .clone();
        if let Some(h) = resolved {
            home_of.insert(e.pid, h);
        }
    }
    let mut jobs = Vec::new();
    for e in &entries {
        let Some(home) = home_of.get(&e.pid) else {
            continue;
        };
        if home_of.get(&e.ppid) == Some(home) {
            continue;
        }
        jobs.push(Job {
            pid: e.pid,
            home: home.clone(),
            ancestors: ancestors(e.pid, &ppid_of),
        });
    }
    (JobCounts { jobs }, Vec::new())
}

/// The candidate home `value` names, if any: it must be non-empty,
/// absolute and at most [`PATH_MAX`] bytes before it is canonicalised.
fn candidate_home(value: &OsString, homes: &[PathBuf]) -> Option<PathBuf> {
    let len = value.as_encoded_bytes().len();
    if len == 0 || len > PATH_MAX {
        return None;
    }
    let path = Path::new(value);
    if !path.is_absolute() {
        return None;
    }
    let canonical = std::fs::canonicalize(path).ok()?;
    homes.iter().find(|h| **h == canonical).cloned()
}

/// The parent chain of `pid`, nearest first, bounded and cycle-safe.
fn ancestors(pid: u32, ppid_of: &HashMap<u32, u32>) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut seen = HashSet::from([pid]);
    let mut cur = pid;
    while chain.len() < MAX_ANCESTORS {
        let Some(&parent) = ppid_of.get(&cur) else {
            break;
        };
        if parent == 0 || !seen.insert(parent) {
            break;
        }
        chain.push(parent);
        cur = parent;
    }
    chain
}
