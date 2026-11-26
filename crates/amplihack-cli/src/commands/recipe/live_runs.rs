//! Issue #3161: the live recipe runs for one pull request, whatever their
//! work root and state folder.
//!
//! `recipe guide <PR URL>` writes the entry into each live run's own
//! `runs/<id>/guidance/` folder, which every run lists first, and
//! `recipe guidance show <PR URL>` reads the same folders. Both find them
//! here, so the read and the write resolve one set of folders.
//!
//! An ended run's folder is not listed for a pull request (PR #3190
//! review). The goal loop exports every folder `show` lists to each turn,
//! and a launch puts every entry of every listed folder in its prompt, so a
//! new drive would take an ended drive's session-name guidance, such as
//! "stop", and mark it read in that run's log. An entry sent by pull
//! request URL is also in the pull request folder, so it is still listed
//! from there. [`runs_for_pr_by_state`] returns the ended runs' folders
//! apart: `show` reads their logs, never their entries, so an entry a step
//! read before its run ended is still reported as read. An ended run's own
//! guidance is read by its session name.
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
//! A pointer that records no pull request (a drive that opened the pull
//! request itself) falls back to the drive's goal record: the
//! `guidance_target` of the newest `goal.json` under the run's own
//! `autodrive-state/turns/` (`goal_pr`). It is read only once the run
//! folder has passed `accept_recorded`, and it is validated and matched as a
//! pointer's `pr_url` is.
//!
//! Known limits: a drive given its own `autodrive_state_dir` outside the run
//! folder, a run whose session log was removed, and a run started in the
//! foreground (its start record goes to its own stderr, not to the log
//! folder) are not found. The pull request folder is still written for
//! those, as before, and a run with the pull request in its context reads
//! it at its next agent step when its state folder is the sender's.

use super::guide::{RootChecks, accept_recorded};
use super::guide_records::{self, Pointer};
use crate::commands::cleanup::run_dir::RunReport;
use crate::run_guidance::env::pr_ref_from_context;
use crate::run_guidance::fs_safety::{DirState, dir_state, untrusted_reason};
use crate::run_guidance::pr_key::{PrRef, same_repo_ref};
use crate::run_guidance::sanitize::sanitize_line;
use crate::run_guidance::{GUIDANCE_DIR, Source};
use std::collections::{BTreeMap, HashSet};
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

/// The pull request a pointer records, through the shared resolver, when it
/// can be pull request number `want`.
///
/// A number-only pointer is resolved by running `git remote get-url origin`
/// in its folder. The log folder holds a pointer for every past run, so the
/// recorded number is compared with `want` first, and git runs only for a
/// pointer that can match. A pointer with another number could not pass
/// [`same_repo_ref`], so the result is the same.
pub(super) fn pointer_pr(record: &Pointer, want: u64) -> Option<PrRef> {
    let mut context = BTreeMap::new();
    if let Some(url) = &record.pr_url {
        context.insert("pr_url".to_string(), url.clone());
    }
    let number = record
        .pr_number
        .as_ref()
        .filter(|n| n.trim().parse::<u64>().ok() == Some(want));
    let working_dir = number.and_then(|_| {
        record
            .worktree
            .as_deref()
            .or(record.cwd.as_deref())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_dir())
    });
    // A number with no folder to resolve it against names no repository.
    if let (Some(number), Some(_)) = (number, &working_dir) {
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

/// For `recipe guidance show <PR URL>`: every live run for `pr` (reachable
/// or not, so an unreachable one can be shown with its reason), and the
/// guidance folder of each ended run whose folder still passes the checks.
/// The live runs' folders are listed; the ended runs' folders are not, and
/// only their logs are read, for read state. Nothing is written to either.
pub(crate) fn runs_for_pr_by_state(pr: &PrRef) -> (Vec<LiveRun>, Vec<PathBuf>) {
    let (live, ended): (Vec<_>, Vec<_>) = runs_for_pr(pr).into_iter().partition(|(_, l)| *l);
    (
        live.into_iter().map(|(run, _)| run).collect(),
        ended
            .into_iter()
            .filter_map(|(run, _)| run.reachable.ok())
            .collect(),
    )
}

/// Every run that records `pr`, each once, with whether it is live.
fn runs_for_pr(pr: &PrRef) -> Vec<(LiveRun, bool)> {
    let Ok(log_dir) = super::detach::log_dir() else {
        return Vec::new();
    };
    let mut roots = RootChecks::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut runs = Vec::new();
    let scan = guide_records::scan(&log_dir);
    // A log folder that exists but could not be read hides every run, so the
    // sender is told rather than shown "no live run". A missing folder just
    // means no run has been started.
    if scan.records.is_empty()
        && scan.skipped == 0
        && let Some(note) = scan.note.as_deref().filter(|n| *n != "it does not exist")
    {
        eprintln!(
            "recipe guide: warning: live runs could not be looked up, so none was found: {}",
            crate::run_guidance::sanitize::sanitize_line(note)
        );
    }
    for record in scan.records {
        let Some(recorded) = record.run_dir.clone() else {
            continue;
        };
        let run_dir = PathBuf::from(&recorded);
        if !run_dir.is_absolute() || seen.contains(&recorded) {
            continue;
        }
        let records_pr = [&record.pr_url, &record.pr_number]
            .iter()
            .any(|v| v.as_deref().is_some_and(|v| !v.trim().is_empty()));
        let accepted = if records_pr {
            if !pointer_pr(&record, pr.number).is_some_and(|found| same_repo_ref(&found, pr)) {
                continue;
            }
            accept_recorded(&recorded, record.run_id.as_deref(), &mut roots).map(|(dir, _)| dir)
        } else {
            // No pull request in the pointer: only a run folder that passes
            // the checks is read for a goal record.
            let Ok((dir, _)) = accept_recorded(&recorded, record.run_id.as_deref(), &mut roots)
            else {
                continue;
            };
            if !goal_pr(&dir).is_some_and(|found| same_repo_ref(&found, pr)) {
                continue;
            }
            Ok(dir)
        };
        seen.insert(recorded.clone());
        let live = is_live(&run_dir, record.run_id.as_deref());
        let reachable = accepted.and_then(|dir| guidance_folder(&dir));
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

/// The largest `goal.json` read. The goal loop itself refuses one over 1 MiB.
const MAX_GOAL_BYTES: u64 = 1024 * 1024;

/// How many of the newest trusted turn folders are tried for a `goal.json`.
/// A turn folder exists before its goal record is written, so the newest one
/// may not hold it yet.
const GOAL_TURNS_TRIED: usize = 3;

/// The pull request a drive's goal record names: the `guidance_target` of
/// the newest readable `goal.json` under `<run_dir>/autodrive-state/turns/<n>/`,
/// through the resolver a pointer's `pr_url` goes through. Turn folders that
/// fail the trust rule are passed over, and of the rest the newest
/// [`GOAL_TURNS_TRIED`] are tried. The first record that parses decides: one
/// with no valid target names nothing, so an older turn's target is not used.
/// `None` when the folders are missing or untrusted, or the record names no
/// valid pull request. `run_dir` must already have passed `accept_recorded`.
pub(super) fn goal_pr(run_dir: &Path) -> Option<PrRef> {
    let state = run_dir.join("autodrive-state");
    let turns = state.join("turns");
    for dir in [&state, &turns] {
        if !matches!(dir_state(dir), Ok(DirState::Trusted)) {
            return None;
        }
    }
    let mut numbers: Vec<(u64, PathBuf)> = std::fs::read_dir(&turns)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some((name.parse().ok()?, e.path()))
        })
        .collect();
    numbers.sort_by_key(|n| std::cmp::Reverse(n.0));
    numbers
        .into_iter()
        .filter(|(_, dir)| matches!(dir_state(dir), Ok(DirState::Trusted)))
        .take(GOAL_TURNS_TRIED)
        .find_map(|(_, dir)| read_goal(&dir.join("goal.json")))
        .and_then(|goal| goal.get("guidance_target")?.as_str().map(str::to_string))
        .and_then(|url| {
            let context = BTreeMap::from([("pr_url".to_string(), url)]);
            pr_ref_from_context(&context, Path::new("/"))
        })
}

/// One `goal.json`, parsed, read through one handle opened without
/// following a link and checked by its own metadata. `None` when it is
/// missing, untrusted, too large or not JSON.
fn read_goal(path: &Path) -> Option<serde_json::Value> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        // O_NONBLOCK: a FIFO planted here must not hang the reader; the
        // regular-file check below then refuses it.
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if untrusted_reason(&meta, false).is_some() || meta.len() > MAX_GOAL_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_GOAL_BYTES).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
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

/// The end a run recorded in its `run.json`, ready for a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunEnd {
    /// `ended_at`, sanitised and at most [`MAX_END_CHARS`] characters.
    pub at: String,
    /// `outcome` (`success`, `failure`, `interrupted`), sanitised and
    /// bounded the same way; `not recorded` when empty.
    pub outcome: String,
}

/// The most characters of a recorded `ended_at` or `outcome` shown.
const MAX_END_CHARS: usize = 64;

fn end_field(value: &str) -> String {
    sanitize_line(&value.trim().chars().take(MAX_END_CHARS).collect::<String>())
}

/// The end recorded by the run whose guidance folder is `guidance_dir`
/// (PR #3190 review): an entry left unread when its run ended is reported
/// as such, not only as "not yet read".
///
/// `None` when the folder is not a run's (a pull request's state folder,
/// which a later run reads), or its `run.json` cannot be read or records no
/// end. Only an end the run wrote is reported, never one inferred from a
/// process that has gone. `run.json` is read without following a link and
/// up to its size cap, as everywhere else.
pub(crate) fn run_end(guidance_dir: &Path) -> Option<RunEnd> {
    if Source::of(guidance_dir) != Source::Run {
        return None;
    }
    let report = RunReport::read(guidance_dir.parent()?).ok()?;
    let at = report.ended_at.filter(|at| !at.trim().is_empty())?;
    let outcome = if report.outcome.trim().is_empty() {
        "not recorded".to_string()
    } else {
        end_field(&report.outcome)
    };
    Some(RunEnd {
        at: end_field(&at),
        outcome,
    })
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
