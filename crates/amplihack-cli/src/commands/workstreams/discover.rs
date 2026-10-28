//! Finding runs in the recipe log directory and keeping the caller's
//! descendants (D4). Only reads: nothing here creates, changes or removes a
//! file or a mode.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::commands::recipe::run_record::{self, RunRecord, valid_session};

/// Directory entries read from the log directory.
const MAX_ENTRIES: usize = 2_000;
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
/// `<session>.log`, reading at most [`MAX_ENTRIES`] entries. Temporary
/// files and the secret spec and context files are skipped.
pub(crate) fn scan(log_dir: &Path) -> Vec<Discovered> {
    let Ok(entries) = fs::read_dir(log_dir) else {
        return Vec::new();
    };
    let mut found: BTreeMap<String, Discovered> = BTreeMap::new();
    for entry in entries.take(MAX_ENTRIES).filter_map(Result::ok) {
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
        let slot = found
            .entry(session.to_string())
            .or_insert_with(|| Discovered {
                session: session.to_string(),
                record: None,
                log: None,
            });
        if is_record {
            slot.record = run_record::read(&entry.path()).filter(|r| r.session == session);
        } else {
            slot.log = Some(entry.path());
        }
    }
    found.into_values().collect()
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
