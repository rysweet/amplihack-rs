//! Finding runs in the recipe log directory and keeping the caller's
//! descendants (D4). Only reads: nothing here creates, changes or removes a
//! file or a mode.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::commands::recipe::detached_record::{self, RunRecord, valid_session};

/// Run files (records and logs) displayed from the log directory, newest run
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
/// The log directory has more run files than [`MAX_ENTRIES`]; the newest
/// runs are shown.
pub(crate) const TRUNCATED: &str =
    "warning: more than 2000 run files in the recipe log directory; older runs are not shown";
/// The log directory has more entries than [`MAX_NAMES`]. `read_dir`
/// returns entries in no particular order, so the entries left unlisted may
/// include recent runs, and the runs shown are not necessarily the newest.
pub(crate) const TOO_MANY_ENTRIES: &str = "warning: more than 20000 entries in the recipe log directory; some runs, possibly recent ones, are not shown";

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

/// The newest runs across all callers, bounded by the display file cap.
#[cfg(test)]
pub(crate) fn scan(log_dir: &Path) -> (Vec<Discovered>, Option<&'static str>) {
    scan_capped(log_dir, MAX_NAMES, MAX_ENTRIES)
}

/// Resolve caller membership before the display cap. At most MAX_NAMES
/// directory entries and their bounded records are read. Old ancestors
/// participate in membership even when their rows fall outside the cap.
pub(crate) fn scan_selected(
    log_dir: &Path,
    identity: Option<&str>,
    all: bool,
) -> (Vec<Discovered>, Option<&'static str>) {
    scan_selected_capped(log_dir, MAX_NAMES, MAX_ENTRIES, identity, all)
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

/// List at most `names` entries and display at most `cap` run
/// files.
#[cfg(test)]
pub(super) fn scan_capped(
    log_dir: &Path,
    names: usize,
    cap: usize,
) -> (Vec<Discovered>, Option<&'static str>) {
    scan_selected_capped(log_dir, names, cap, None, true)
}

/// List and resolve membership, then take the newest matching run files.
pub(super) fn scan_selected_capped(
    log_dir: &Path,
    names: usize,
    cap: usize,
    identity: Option<&str>,
    all: bool,
) -> (Vec<Discovered>, Option<&'static str>) {
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
    for entry in entries.by_ref().take(names).filter_map(Result::ok) {
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
    let names_cut = entries.next().is_some();
    let mut truncated = false;
    let mut sessions: Vec<(String, Files)> = by_session.into_iter().collect();
    sessions.sort_by(|(a, fa), (b, fb)| fb.newest.cmp(&fa.newest).then_with(|| a.cmp(b)));
    // Read records before pruning: a recent child may need an old parent
    // to connect it to the caller. The directory-entry bound also bounds
    // this graph, and detached_record::read bounds each record's bytes.
    let candidates: Vec<Discovered> = sessions
        .iter()
        .map(|(session, files)| Discovered {
            session: session.clone(),
            record: files
                .record
                .as_deref()
                .and_then(detached_record::read)
                .filter(|r| r.session == *session),
            log: files.log.clone(),
        })
        .collect();
    let mut selected: HashMap<String, Discovered> = select(candidates, identity, all)
        .into_iter()
        .map(|r| (r.session.clone(), r))
        .collect();
    let mut taken = 0;
    let mut found: BTreeMap<String, Discovered> = BTreeMap::new();
    for (session, files) in sessions {
        let Some(run) = selected.remove(&session) else {
            continue;
        };
        if taken + files.count() > cap {
            truncated = true;
            break;
        }
        taken += files.count();
        found.insert(session, run);
    }
    (
        found.into_values().collect(),
        if names_cut {
            Some(TOO_MANY_ENTRIES)
        } else {
            truncated.then_some(TRUNCATED)
        },
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
/// already selected: each run once, at most [`MAX_DEPTH`] levels. The
/// directory listing bounds the graph; the display cap is applied after
/// membership is resolved. Nothing without an identity.
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
    for _ in 0..MAX_DEPTH {
        let mut next = Vec::new();
        for parent in &frontier {
            for &i in children.get(parent.as_str()).into_iter().flatten() {
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
