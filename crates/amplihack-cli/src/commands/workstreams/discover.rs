//! Finding the detached runs a session started (issue #1710), from the
//! recipe log directory: each run has `<session>.log`, and runs launched
//! since #1710 also `<session>.meta.json` naming their parent session and
//! parent run. No run is matched by timing or directory.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::logptr;
use super::meta::{self, DirCheck, RunMeta};

/// Directory entries looked at at most.
const MAX_ENTRIES: usize = 4096;

pub(crate) const DETACHED_SESSION_ENV: &str = "AMPLIHACK_DETACHED_SESSION";
pub(crate) const SESSION_ID_ENV: &str = "AMPLIHACK_SESSION_ID";

/// Which runs to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    /// `--all`: every run on the host.
    All,
    /// The runs an agent session started, and their descendants.
    Session(String),
    /// The runs a detached run started, and their descendants.
    Run(String),
    /// No session id in the environment.
    None,
}

/// The scope from `--all` and the environment. The caller is a detached run
/// when `AMPLIHACK_DETACHED_SESSION` is set: it inherits its starter's
/// `AMPLIHACK_SESSION_ID`, so matching on that would list its siblings. An
/// id that is not 1–128 characters of `[A-Za-z0-9._:-]` counts as unset.
pub(crate) fn scope_from_env(all: bool, get: &dyn Fn(&str) -> Option<String>) -> Scope {
    if all {
        return Scope::All;
    }
    if let Some(run) = get(DETACHED_SESSION_ENV).and_then(|v| meta::sanitize_id(&v)) {
        return Scope::Run(run);
    }
    match get(SESSION_ID_ENV).and_then(|v| meta::sanitize_id(&v)) {
        Some(session) => Scope::Session(session),
        None => Scope::None,
    }
}

/// One discovered run.
#[derive(Debug, Clone, Default)]
pub(crate) struct Run {
    pub session: String,
    pub recipe: Option<String>,
    /// The meta file's issue, else the log pointer's.
    pub issue: Option<u64>,
    /// The meta file's PR.
    pub pr: Option<u64>,
    /// The log pointer's PR.
    pub pointer_pr: Option<u64>,
    pub branch: Option<String>,
    /// The run's directory: absolute and existing, else `None`.
    pub cwd: Option<PathBuf>,
    /// The meta file's start time, else the log pointer's; never a file time.
    pub started: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    /// The end of the log reads as a usage-limit stop.
    pub stopped_on_limit: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Discovery {
    pub runs: Vec<Run>,
    pub warnings: Vec<String>,
}

/// The runs in `log_dir` that `scope` selects. A missing directory gives no
/// runs and no warning, and is never created; one that is a symlink, not
/// owned by this user, or writable by others gives one warning and no runs.
pub(crate) fn discover(log_dir: &Path, scope: &Scope) -> Discovery {
    let mut found = Discovery::default();
    if *scope == Scope::None {
        return found;
    }
    match meta::check_dir(log_dir) {
        DirCheck::Missing => return found,
        DirCheck::Unsafe => {
            found.warnings.push(
                "warning: the recipe log directory is a symlink, not owned by you, or writable \
                 by others; no runs listed"
                    .to_string(),
            );
            return found;
        }
        DirCheck::Private => {}
    }
    let Ok(entries) = fs::read_dir(log_dir) else {
        found
            .warnings
            .push("warning: the recipe log directory cannot be listed; no runs listed".to_string());
        return found;
    };
    // One entry past the cap tells a full listing from a cut one.
    let mut listed: Vec<fs::DirEntry> = entries
        .take(MAX_ENTRIES + 1)
        .filter_map(|e| e.ok())
        .collect();
    if listed.len() > MAX_ENTRIES {
        listed.truncate(MAX_ENTRIES);
        found.warnings.push(format!(
            "warning: more than {MAX_ENTRIES} entries in the recipe log directory; \
             some runs may be missing"
        ));
    }
    let mut sessions: Vec<String> = listed
        .into_iter()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".log").map(str::to_string))
        .filter(|s| meta::sanitize_id(s).is_some())
        .collect();
    sessions.sort();

    let metas: Vec<(String, Option<RunMeta>)> = sessions
        .into_iter()
        .map(|s| {
            let m = meta::read(&log_dir.join(format!("{s}.meta.json"))).filter(|m| m.session == s);
            (s, m)
        })
        .collect();

    let selected: Vec<&(String, Option<RunMeta>)> = match scope {
        Scope::All => metas.iter().collect(),
        Scope::Session(id) => closure(&metas, |m| m.parent_session_id.as_deref() == Some(id), None),
        Scope::Run(root) => closure(
            &metas,
            |m| m.parent_run.as_deref() == Some(root),
            Some(root),
        ),
        Scope::None => Vec::new(),
    };
    found.runs = selected
        .into_iter()
        .map(|(session, m)| build_run(log_dir, session, m.as_ref()))
        .collect();
    found
}

/// The runs whose meta matches `seed`, then, until nothing changes, the runs
/// whose `parent_run` is one already chosen. `root` itself is never chosen.
fn closure<'a>(
    metas: &'a [(String, Option<RunMeta>)],
    seed: impl Fn(&RunMeta) -> bool,
    root: Option<&str>,
) -> Vec<&'a (String, Option<RunMeta>)> {
    let mut chosen: BTreeSet<&str> = BTreeSet::new();
    for (s, m) in metas {
        if m.as_ref().is_some_and(&seed) && Some(s.as_str()) != root {
            chosen.insert(s);
        }
    }
    loop {
        let before = chosen.len();
        for (s, m) in metas {
            let parent = m.as_ref().and_then(|m| m.parent_run.as_deref());
            if parent.is_some_and(|p| chosen.contains(p) || Some(p) == root)
                && Some(s.as_str()) != root
            {
                chosen.insert(s);
            }
        }
        if chosen.len() == before {
            break;
        }
    }
    metas
        .iter()
        .filter(|(s, _)| chosen.contains(s.as_str()))
        .collect()
}

fn build_run(log_dir: &Path, session: &str, m: Option<&RunMeta>) -> Run {
    let info = logptr::read(&log_dir.join(format!("{session}.log")));
    let cwd = m
        .and_then(|m| m.cwd.clone())
        .filter(|c| c.is_absolute() && c.is_dir());
    Run {
        session: session.to_string(),
        recipe: m.and_then(|m| m.recipe.clone()),
        issue: m.and_then(|m| m.issue_number).or(info.issue),
        pr: m.and_then(|m| m.pr_number),
        pointer_pr: info.pr,
        branch: m.and_then(|m| m.branch.clone()),
        cwd,
        started: m.and_then(|m| m.started_at).or(info.started),
        exit_code: info.exit_code,
        stopped_on_limit: super::stage::tail_shows_limit(&info.tail),
    }
}
