//! Finding runs in the recipe log directory and keeping the caller's
//! descendants (D4). Only reads: nothing here creates, changes or removes a
//! file or a mode.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::commands::recipe::run_record::{self, RunRecord, valid_session};

/// Run files (records and logs) read from the log directory, newest run
/// first.
const MAX_ENTRIES: usize = 2_000;
/// Directory entries listed, by name and metadata only, to find the newest
/// runs. Nothing past this is looked at.
const MAX_NAMES: usize = 20_000;
/// Mode bits that make the log directory shared.
const SHARED_DIR_BITS: u32 = 0o022;
/// The log directory exists but cannot be listed.
pub(crate) const UNREADABLE: &str = "warning: recipe log directory unreadable";
/// The log directory is not owned by this user, or others may write to it.
pub(crate) const NOT_PRIVATE: &str = "warning: recipe log directory is not private; ignoring it";
/// The log directory has more run files than [`MAX_ENTRIES`], or more
/// entries than [`MAX_NAMES`].
pub(crate) const TRUNCATED: &str =
    "warning: more than 2000 run files in the recipe log directory; older runs are not shown";

/// Runs a walk selects at most.
const MAX_SELECTED: usize = 2_000;
/// Levels of descendants a walk follows at most.
const MAX_DEPTH: usize = 32;

/// A run found in the log directory.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Discovered {
    pub session: String,
    /// `None` for a run that predates run records, or whose record failed
    /// the file or field checks.
    pub record: Option<RunRecord>,
    pub log: Option<PathBuf>,
}

/// One entry per session that has a `<session>.run.json` or a
/// `<session>.log` owned by this user, and the warning to print, if any.
/// Temporary files and the secret spec and context files are skipped.
///
/// At most [`MAX_NAMES`] entries are listed, by name and metadata only.
/// Runs are then taken newest first, by the latest modification time of
/// their files, until [`MAX_ENTRIES`] files are taken; only those records
/// are read. A live run writes to its log, so it is among the newest and is
/// not the one left out. Runs come back ordered by session name.
///
/// A missing directory has no runs and no warning. One that cannot be read,
/// or that is not owned by this user or is writable by others, has no runs
/// and a fixed warning that names neither the path nor the OS error. One
/// with more than the cap gives the newest runs and [`TRUNCATED`].
pub(crate) fn scan(log_dir: &Path) -> (Vec<Discovered>, Option<&'static str>) {
    scan_capped(log_dir, MAX_ENTRIES)
}

/// The files of one session, found by name.
#[derive(Default)]
struct Files {
    record: Option<PathBuf>,
    log: Option<PathBuf>,
    newest: (i64, i64),
}

impl Files {
    fn count(&self) -> usize {
        usize::from(self.record.is_some()) + usize::from(self.log.is_some())
    }
}

/// [`scan`] taking at most `cap` run files.
pub(super) fn scan_capped(log_dir: &Path, cap: usize) -> (Vec<Discovered>, Option<&'static str>) {
    let meta = match fs::metadata(log_dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => return (Vec::new(), None),
        Err(_) => return (Vec::new(), Some(UNREADABLE)),
    };
    if !meta.is_dir() {
        return (Vec::new(), Some(UNREADABLE));
    }
    if meta.uid() != euid() || meta.mode() & SHARED_DIR_BITS != 0 {
        return (Vec::new(), Some(NOT_PRIVATE));
    }
    let Ok(mut entries) = fs::read_dir(log_dir) else {
        return (Vec::new(), Some(UNREADABLE));
    };
    let mut by_session: HashMap<String, Files> = HashMap::new();
    for entry in entries.by_ref().take(MAX_NAMES).filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.contains(".tmp.") {
            continue;
        }
        let (session, is_record) = if let Some(s) = name.strip_suffix(".run.json") {
            (s, true)
        } else if let Some(s) = name.strip_suffix(".log") {
            (s, false)
        } else {
            continue;
        };
        if !valid_session(session) {
            continue;
        }
        // Not followed: this is the entry itself, as `read_dir` found it.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        // Another user's log is not one of this user's runs. A record gets
        // the same check, on the opened file, when it is read.
        if !is_record && !owned_by(&meta, euid()) {
            continue;
        }
        let files = by_session.entry(session.to_string()).or_default();
        files.newest = files.newest.max((meta.mtime(), meta.mtime_nsec()));
        if is_record {
            files.record = Some(entry.path());
        } else {
            files.log = Some(entry.path());
        }
    }
    let mut truncated = entries.next().is_some();
    let mut sessions: Vec<(String, Files)> = by_session.into_iter().collect();
    sessions.sort_by(|(a, fa), (b, fb)| fb.newest.cmp(&fa.newest).then_with(|| a.cmp(b)));
    let mut taken = 0;
    let mut found: BTreeMap<String, Discovered> = BTreeMap::new();
    for (session, files) in sessions {
        if taken + files.count() > cap {
            truncated = true;
            break;
        }
        taken += files.count();
        let record = files
            .record
            .as_deref()
            .and_then(run_record::read)
            .filter(|r| r.session == session);
        found.insert(
            session.clone(),
            Discovered {
                session,
                record,
                log: files.log,
            },
        );
    }
    (
        found.into_values().collect(),
        truncated.then_some(TRUNCATED),
    )
}

/// Whether `meta` belongs to the user `uid`.
pub(super) fn owned_by(meta: &fs::Metadata, uid: u32) -> bool {
    meta.uid() == uid
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// With `all`, every run. Otherwise the runs whose `parent` is `identity`,
/// then, level by level, the runs whose `parent` is `run:<session>` of a run
/// already selected: each run once, at most [`MAX_SELECTED`] runs and
/// [`MAX_DEPTH`] levels. Nothing without an identity.
pub(crate) fn select(runs: Vec<Discovered>, identity: Option<&str>, all: bool) -> Vec<Discovered> {
    if all {
        return runs;
    }
    let Some(identity) = identity else {
        return Vec::new();
    };
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, run) in runs.iter().enumerate() {
        if let Some(parent) = run.record.as_ref().and_then(|r| r.parent.as_deref()) {
            children.entry(parent).or_default().push(i);
        }
    }
    let mut picked: Vec<usize> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    let mut frontier: Vec<String> = vec![identity.to_string()];
    'walk: for _ in 0..MAX_DEPTH {
        let mut next = Vec::new();
        for parent in &frontier {
            for &i in children.get(parent.as_str()).into_iter().flatten() {
                if picked.len() >= MAX_SELECTED {
                    break 'walk;
                }
                if seen.insert(i) {
                    picked.push(i);
                    next.push(format!("run:{}", runs[i].session));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    let mut runs: Vec<Option<Discovered>> = runs.into_iter().map(Some).collect();
    picked.into_iter().filter_map(|i| runs[i].take()).collect()
}
