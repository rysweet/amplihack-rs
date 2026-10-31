//! Reading `/proc` (R3, R4, S2, S11): bounded parsers that never panic on
//! hostile input, what counts as a `claude` process (A5), one snapshot of
//! this user's live processes per pass (A11), and the test seam (S27).

pub use super::claude_names::{CLAUDE, ClaudeNames, is_claude, is_claude_with};
use amplihack_utils::secure_files::writable_by_others;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const STAT_LIMIT: u64 = 64 * 1024;
const CMDLINE_LIMIT: u64 = 256 * 1024;
const ENVIRON_LIMIT: u64 = 16 * 1024 * 1024;
/// The first buffer for a `/proc` file. `/proc` reports a size of 0, so an
/// empty buffer would grow from 32 bytes and take eight reads for a
/// `status`; one page holds a whole `stat`, `status` or typical `cmdline`.
const FIRST_READ: usize = 4096;

/// `/proc`, or the synthetic root a test names in `AMPLIHACK_TEST_PROC_ROOT`
/// when every condition of [`proc_root_from`] holds.
pub fn proc_root() -> PathBuf {
    proc_root_from(
        std::env::var_os("AMPLIHACK_TEST_PROC_ROOT").as_deref(),
        std::env::var_os(crate::user_config::TEST_HOME_GUARD_ENV).as_deref(),
        &std::env::temp_dir(),
    )
}

/// The seam is honoured only under a non-empty test guard, for a path whose
/// canonical form lies inside the canonical temp dir, and for a directory
/// owned by this user that others cannot write (S27).
pub fn proc_root_from(var: Option<&OsStr>, guard: Option<&OsStr>, temp_dir: &Path) -> PathBuf {
    let real = PathBuf::from("/proc");
    let (Some(var), true) = (var, guard.is_some_and(|g| !g.is_empty())) else {
        return real;
    };
    let (Ok(root), Ok(temp)) = (Path::new(var).canonicalize(), temp_dir.canonicalize()) else {
        return real;
    };
    let private = fs::metadata(&root).is_ok_and(|m| {
        m.is_dir() && m.uid() == amplihack_utils::secure_files::euid() && !writable_by_others(&m)
    });
    if root.starts_with(&temp) && root != temp && private {
        root
    } else {
        real
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub comm: String,
    pub state: char,
    pub ppid: u32,
    pub starttime: u64,
}

/// `/proc/<pid>/stat`. The fields after the last `)` are read, so a `comm`
/// holding `) ` cannot shift them; `starttime` is field 22.
pub fn parse_stat(raw: &[u8]) -> Option<Stat> {
    let open = raw.iter().position(|&b| b == b'(')?;
    let close = raw.iter().rposition(|&b| b == b')')?;
    if close < open {
        return None;
    }
    let comm = String::from_utf8_lossy(&raw[open + 1..close]).into_owned();
    let rest = std::str::from_utf8(&raw[close + 1..]).ok()?;
    let fields: Vec<&str> = rest.split_ascii_whitespace().collect();
    let state = single_char(fields.first()?)?;
    let ppid = fields.get(1)?.parse().ok()?;
    let starttime = fields.get(19)?.parse().ok()?;
    Some(Stat {
        comm,
        state,
        ppid,
        starttime,
    })
}

fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: char,
    pub ruid: u32,
    pub euid: u32,
}

/// `State:` and the real and effective uids from `/proc/<pid>/status`.
pub fn parse_status(raw: &[u8]) -> Option<Status> {
    let text = std::str::from_utf8(raw).ok()?;
    let (mut state, mut uids) = (None, None);
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("State:") {
            state = v.split_ascii_whitespace().next().and_then(single_char);
        } else if let Some(v) = line.strip_prefix("Uid:") {
            let mut it = v.split_ascii_whitespace().map(str::parse::<u32>);
            uids = match (it.next(), it.next()) {
                (Some(Ok(r)), Some(Ok(e))) => Some((r, e)),
                _ => None,
            };
        }
    }
    let (ruid, euid) = uids?;
    Some(Status {
        state: state?,
        ruid,
        euid,
    })
}

/// NUL-separated strings (`cmdline`, `environ`); a final NUL ends the list.
pub fn split_nul(raw: &[u8]) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }
    let body = raw.strip_suffix(b"\0").unwrap_or(raw);
    body.split(|&b| b == 0)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub starttime: u64,
    pub comm: String,
    /// `Err` when the command line of a live process could not be read.
    pub argv: Result<Vec<String>, String>,
}

/// This user's live processes at one moment (S2).
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub self_pid: u32,
    pub procs: BTreeMap<u32, Proc>,
    pub children: HashMap<u32, Vec<u32>>,
    /// The pids that pass [`is_claude`].
    pub claude: Vec<u32>,
}

/// Every process whose real and effective uids are both `uid` and that is
/// not a zombie. Fails when the root cannot be listed or `self` read.
pub fn snapshot(root: &Path, uid: u32) -> Result<Snapshot, String> {
    let self_pid = fs::read_link(root.join("self"))
        .ok()
        .and_then(|p| p.to_str()?.parse::<u32>().ok())
        .filter(|&pid| root.join(pid.to_string()).join("stat").is_file())
        .ok_or("cannot read the process table")?;
    let entries = fs::read_dir(root).map_err(|e| format!("cannot list the process table: {e}"))?;
    let mut procs = BTreeMap::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(p) = read_proc(root, pid, uid) {
            procs.insert(pid, p);
        }
    }
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs.values() {
        children.entry(p.ppid).or_default().push(p.pid);
    }
    let claude = procs
        .values()
        .filter(|p| is_claude(&p.comm, p.argv.as_deref().unwrap_or(&[])))
        .map(|p| p.pid)
        .collect();
    Ok(Snapshot {
        self_pid,
        procs,
        children,
        claude,
    })
}

/// One process, or `None` when it vanished, belongs to someone else or is a
/// zombie.
fn read_proc(root: &Path, pid: u32, uid: u32) -> Option<Proc> {
    let dir = root.join(pid.to_string());
    let status = parse_status(&read_limited(&dir.join("status"), STAT_LIMIT).ok()?)?;
    if status.ruid != uid || status.euid != uid || status.state == 'Z' {
        return None;
    }
    let stat = parse_stat(&read_limited(&dir.join("stat"), STAT_LIMIT).ok()?)?;
    if stat.state == 'Z' {
        return None;
    }
    let argv = read_limited(&dir.join("cmdline"), CMDLINE_LIMIT)
        .map(|raw| split_nul(&raw))
        .map_err(|e| format!("cannot read the command line of process {pid}: {e}"));
    Some(Proc {
        pid,
        ppid: stat.ppid,
        starttime: stat.starttime,
        comm: stat.comm,
        argv,
    })
}

impl Snapshot {
    /// Whether `pid` is in the snapshot with start time `start`.
    pub fn alive(&self, pid: u32, start: u64) -> bool {
        self.procs.get(&pid).is_some_and(|p| p.starttime == start)
    }

    /// The parent, its parent and so on, nearest first, while they are in
    /// the snapshot. Stops at pid 1 or a cycle.
    pub fn ancestors(&self, pid: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut seen = HashSet::from([pid]);
        let mut cur = pid;
        while let Some(p) = self.procs.get(&cur) {
            let parent = p.ppid;
            if parent <= 1 || !seen.insert(parent) || !self.procs.contains_key(&parent) {
                break;
            }
            out.push(parent);
            cur = parent;
        }
        out
    }

    /// Every process below `pid`, breadth first, nearest first.
    pub fn descendants(&self, pid: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut seen = HashSet::from([pid]);
        let mut queue = VecDeque::from([pid]);
        while let Some(cur) = queue.pop_front() {
            for &child in self.children.get(&cur).into_iter().flatten() {
                if seen.insert(child) {
                    out.push(child);
                    queue.push_back(child);
                }
            }
        }
        out
    }

    /// Whether `pid` counts as Claude (A5) and its command line was read.
    pub fn readable_claude(&self, pid: u32) -> bool {
        self.claude.contains(&pid) && self.argv(pid).is_some()
    }

    /// The command line of `pid`, when it could be read.
    pub fn argv(&self, pid: u32) -> Option<&[String]> {
        self.procs.get(&pid)?.argv.as_deref().ok()
    }
}

/// `KEY=VALUE` pairs from `/proc/<pid>/environ`. Entries that are not UTF-8
/// are skipped. The buffer is never formatted into an error (S11).
pub fn read_environ(root: &Path, pid: u32) -> Result<Vec<(String, String)>, String> {
    let raw = read_limited(&root.join(pid.to_string()).join("environ"), ENVIRON_LIMIT)
        .map_err(|_| format!("could not read the environment of process {pid}"))?;
    Ok(raw
        .split(|&b| b == 0)
        .filter_map(|kv| std::str::from_utf8(kv).ok())
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

/// The `cwd` link of `pid`.
pub fn cwd(root: &Path, pid: u32) -> Option<PathBuf> {
    fs::read_link(root.join(pid.to_string()).join("cwd")).ok()
}

/// The start time of `pid` now, read again after other reads to detect a
/// reused pid (S11).
pub fn start_time(root: &Path, pid: u32) -> Option<u64> {
    let raw = read_limited(&root.join(pid.to_string()).join("stat"), STAT_LIMIT).ok()?;
    parse_stat(&raw).map(|s| s.starttime)
}

/// At most `limit` bytes of `path`; a longer file is an error.
fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|e| e.kind().to_string())?;
    let mut buf = Vec::with_capacity(FIRST_READ);
    file.take(limit + 1)
        .read_to_end(&mut buf)
        .map_err(|e| e.kind().to_string())?;
    if buf.len() as u64 > limit {
        return Err("too large".into());
    }
    Ok(buf)
}
