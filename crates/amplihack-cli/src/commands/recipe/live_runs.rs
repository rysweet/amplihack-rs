//! Issue #3161: the live recipe runs for one pull request, whatever their
//! work root and state folder.
//!
//! `recipe guide <PR URL>` writes the entry into each live run's own
//! `runs/<id>/guidance/` folder, which every run lists first, and
//! `recipe guidance show <PR URL>` reads the same folders, plus those of
//! ended runs still on record. Both find them here, so the read and the
//! write resolve one set of folders.
//!
//! A run is found from the session pointer it wrote at start
//! ([`guide_records::scan`]), which records the pull request of its context
//! (`pr_url`, `pr_number`) since #761. A pointer is a claim, so:
//!
//! - its pull request is validated by the same resolver `recipe run` uses
//!   ([`pr_ref_from_context`]) and matched on owner, repository (both
//!   case-insensitively) and number, never on the number alone. A
//!   number-only pointer resolves against the `origin` of its recorded
//!   worktree, or of its working folder; with neither, it matches nothing;
//! - a run is live when its folder holds a trusted process record
//!   ([`run_record::read_in`]) for the pointer's run id, or for an attempt
//!   `run.json` lists, whose runner is still the process that wrote it (the
//!   start time in clock ticks matches, which guards against a reused pid),
//!   and `run.json` records no end. A run that is not live is left out;
//! - a live run is reachable when its folder passes the checks `recipe guide`
//!   applies to every recorded folder (`accept_recorded`: the work root, the
//!   `runs` folder, the run folder and `run.json`) and its `guidance` folder
//!   is trusted or missing under it. A live run that is not reachable is
//!   still listed, with the reason, so the sender can be told.
//!
//! Nothing here writes or creates anything, and nothing fails: what cannot
//! be read leaves a run out (not live) or marks it unreachable.
//!
//! Known limit: a run whose context names no pull request (a drive that
//! opened the pull request itself) is not found, and a run whose session log
//! was removed is not found either. The pull request folder is
//! still written for those, as before.

use super::guide::{RootChecks, accept_recorded};
use super::guide_records::{self, Pointer};
use crate::run_guidance::GUIDANCE_DIR;
use crate::run_guidance::env::pr_ref_from_context;
use crate::run_guidance::fs_safety::{DirState, dir_state};
use crate::run_guidance::pr_key::{PrRef, same_repo_ref};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One live run for the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveRun {
    /// The detached session the run's log is named after.
    pub session: String,
    /// The run folder, as recorded.
    pub run_dir: PathBuf,
    /// The run's guidance folder, or why it cannot be written or read.
    pub reachable: Result<PathBuf, String>,
}

/// The pull request a pointer records, through the shared resolver.
fn pointer_pr(record: &Pointer) -> Option<PrRef> {
    let mut context = BTreeMap::new();
    if let Some(url) = &record.pr_url {
        context.insert("pr_url".to_string(), url.clone());
    }
    let working_dir = record
        .worktree
        .as_deref()
        .or(record.cwd.as_deref())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir());
    // A number with no folder to resolve it against names no repository.
    if let (Some(number), Some(_)) = (&record.pr_number, &working_dir) {
        context.insert("pr_number".to_string(), number.clone());
    }
    if context.is_empty() {
        return None;
    }
    let working_dir = working_dir.unwrap_or_else(|| PathBuf::from("/"));
    pr_ref_from_context(&context, &working_dir)
}

/// The live runs that record `pr`, in the order their logs were last
/// written (newest first), each run once. `recipe guide <PR URL>` writes to
/// these.
pub(crate) fn live_runs_for_pr(pr: &PrRef) -> Vec<LiveRun> {
    runs_for_pr(pr)
        .into_iter()
        .filter_map(|(run, live)| live.then_some(run))
        .collect()
}

/// The runs for `pr` whose guidance `recipe guidance show <PR URL>` reads:
/// every live run (reachable or not, so an unreachable one can be shown
/// with its reason), then each ended run whose folder still passes the
/// checks. An ended run's folder is read so that an entry a step read
/// before the run ended is still reported as read; nothing is written to
/// it.
pub(crate) fn runs_to_read_for_pr(pr: &PrRef) -> Vec<LiveRun> {
    let (live, ended): (Vec<_>, Vec<_>) = runs_for_pr(pr).into_iter().partition(|(_, l)| *l);
    live.into_iter()
        .map(|(run, _)| run)
        .chain(
            ended
                .into_iter()
                .map(|(run, _)| run)
                .filter(|run| run.reachable.is_ok()),
        )
        .collect()
}

/// Every run that records `pr`, each once, with whether it is live.
fn runs_for_pr(pr: &PrRef) -> Vec<(LiveRun, bool)> {
    let Ok(log_dir) = super::detach::log_dir() else {
        return Vec::new();
    };
    let mut roots = RootChecks::default();
    let mut seen: Vec<String> = Vec::new();
    let mut runs = Vec::new();
    for record in guide_records::scan(&log_dir).records {
        let Some(recorded) = record.run_dir.clone() else {
            continue;
        };
        if seen.contains(&recorded) {
            continue;
        }
        if !pointer_pr(&record).is_some_and(|found| same_repo_ref(&found, pr)) {
            continue;
        }
        seen.push(recorded.clone());
        let run_dir = PathBuf::from(&recorded);
        if !run_dir.is_absolute() {
            continue;
        }
        let live = is_live(&run_dir, record.run_id.as_deref());
        let reachable = accept_recorded(&recorded, record.run_id.as_deref(), &mut roots)
            .and_then(|(dir, _)| guidance_folder(&dir));
        let run = LiveRun {
            session: record
                .session
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            run_dir,
            reachable,
        };
        runs.push((run, live));
    }
    runs
}

/// The run's guidance folder when it is trusted, or missing under a run
/// folder that has just passed the checks.
fn guidance_folder(run_dir: &Path) -> Result<PathBuf, String> {
    let dir = run_dir.join(GUIDANCE_DIR);
    match dir_state(&dir) {
        Ok(DirState::Trusted | DirState::Missing) => Ok(dir),
        Ok(DirState::Untrusted(reason)) => Err(format!("{}: {reason}", dir.display())),
        Err(e) => Err(e.to_string()),
    }
}

/// Whether the run in `run_dir` is running now. See the module docs.
#[cfg(target_os = "linux")]
fn is_live(run_dir: &Path, pointer_run_id: Option<&str>) -> bool {
    use crate::commands::cleanup::run_dir::{RunReport, pid_start_ticks};
    let report = RunReport::read(run_dir).ok();
    if report.as_ref().is_some_and(|r| r.ended_at.is_some()) {
        return false;
    }
    let mut ids: Vec<String> = pointer_run_id.map(str::to_string).into_iter().collect();
    if let Some(report) = &report {
        for attempt in &report.attempts {
            if !ids.contains(&attempt.correlation_id) {
                ids.push(attempt.correlation_id.clone());
            }
        }
    }
    ids.iter().any(|id| {
        let Ok(Some(record)) = super::run_record::read_in(run_dir, id) else {
            return false;
        };
        u32::try_from(record.runner.pid)
            .ok()
            .and_then(pid_start_ticks)
            .is_some_and(|ticks| ticks == record.runner.start_ticks)
    })
}

/// Process records are Linux only, like `recipe stop <run_id>`: elsewhere
/// no run is known to be live, and the pull request folder is written as
/// before.
#[cfg(not(target_os = "linux"))]
fn is_live(_run_dir: &Path, _pointer_run_id: Option<&str>) -> bool {
    false
}
