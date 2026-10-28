//! `amplihack workstreams status` (issue #1710): a plain-text table of the
//! detached runs the calling session started, with their stage and progress,
//! and the capacity of every Claude login, for an agent to paste into its
//! reply.
//!
//! Runs are found through the `<session>.run.json` records `recipe run
//! --detach` leaves (see `recipe/run_record.rs`) and filtered by the
//! caller's identity (`recipe/run_identity.rs`). PR state, live sessions,
//! origins and login usage come through the traits in `sources.rs`, so
//! [`snapshot`] runs in tests without tmux, gh, git or claude. Rendering is
//! the pure function in `render.rs`. The command only reads.

pub(crate) mod discover;
pub(crate) mod logins;
pub(crate) mod manifest;
pub(crate) mod render;
pub(crate) mod sources;
pub(crate) mod stage;

#[cfg(test)]
mod tests_discover;
#[cfg(test)]
mod tests_logins;
#[cfg(test)]
mod tests_manifest;
#[cfg(test)]
mod tests_render;
#[cfg(test)]
mod tests_stage;
#[cfg(test)]
mod tests_status;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::DateTime;
use clap::Subcommand;

use crate::commands::recipe::autodrive_paths::{
    CRUSTY_ROUND_PREFIX, MERGE_READY_ROUND_PREFIX, concern_count, round_count, state_dir,
    state_key, state_root,
};
use crate::commands::recipe::run_identity::caller_identity_from_env;
use crate::commands::recipe::run_record::{self, RunRecord};
use discover::Discovered;
use manifest::ManifestRow;
use render::{LoginsSection, PrCell, Snapshot, WorkRow};
use sources::{LiveSessions, OriginSource, PrInfo, PrSource, UsageSource};
use stage::{PrState, StageInput};

/// Default `--logins-deadline`, in seconds.
const DEFAULT_DEADLINE_SECS: u64 = 8;
/// How long a probe has to stop after its cancel flag is set.
const CANCEL_GRACE: Duration = Duration::from_secs(1);
/// `recipe.log_dir` is unset and so is HOME.
pub(crate) const LOG_DIR_UNAVAILABLE: &str =
    "warning: recipe log directory unavailable (HOME not set)";

/// `amplihack workstreams <subcommand>`
#[derive(Subcommand, Debug)]
pub enum WorkstreamsCommands {
    /// Print the workstreams this session started and the capacity of every
    /// Claude login, as plain text for pasting into a reply
    Status {
        /// Show every run on this host, not only this session's
        #[arg(long)]
        all: bool,
        /// Leave out the LOGINS section; no usage probe is started
        #[arg(long)]
        no_logins: bool,
        /// Labels and queued items (default ~/.amplihack/work/workstreams.tsv)
        #[arg(long, value_name = "PATH")]
        manifest: Option<PathBuf>,
        /// Seconds the LOGINS section may take, 1 to 60
        #[arg(
            long,
            value_name = "SECS",
            default_value_t = DEFAULT_DEADLINE_SECS,
            value_parser = clap::value_parser!(u64).range(1..=60)
        )]
        logins_deadline: u64,
    },
}

/// Dispatch for `amplihack workstreams`.
pub fn run(command: WorkstreamsCommands) -> Result<()> {
    match command {
        WorkstreamsCommands::Status {
            all,
            no_logins,
            manifest,
            logins_deadline,
        } => status(all, no_logins, manifest, logins_deadline),
    }
}

fn status(all: bool, no_logins: bool, manifest: Option<PathBuf>, deadline: u64) -> Result<()> {
    let deadline = Duration::from_secs(deadline);
    let home = crate::user_config::home_dir();
    let manifest = match manifest {
        Some(path) => Some(ManifestPath::Explicit(path)),
        None => home.as_ref().map(|h| {
            ManifestPath::Default(h.join(".amplihack").join("work").join("workstreams.tsv"))
        }),
    };
    let input = StatusInput {
        // Without a log directory there is nothing to discover; the table
        // still shows manifest rows and logins.
        log_dir: crate::commands::recipe::detach::log_dir().map_err(|_| LOG_DIR_UNAVAILABLE),
        state_root: state_root(),
        manifest,
        identity: caller_identity_from_env(),
        all,
        logins_deadline: deadline,
    };
    let usage: Option<Arc<dyn UsageSource>> = (!no_logins)
        .then(|| Arc::new(sources::ProfileUsage { timeout: deadline }) as Arc<dyn UsageSource>);
    let sources = Sources {
        prs: Box::new(sources::GhPrs),
        live: Box::new(sources::TmuxSessions),
        origin: Box::new(sources::GitOrigin::default()),
        usage,
        now: chrono::Utc::now().timestamp(),
        warn: Box::new(|warning| eprintln!("{warning}")),
    };
    print!("{}", render::render(&snapshot(&input, &sources)?));
    Ok(())
}

/// Where the manifest comes from: a missing default is ignored, a missing
/// explicit one is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManifestPath {
    Default(PathBuf),
    Explicit(PathBuf),
}

/// What [`snapshot`] reads, apart from its outside sources.
#[derive(Debug, Clone)]
pub(crate) struct StatusInput {
    /// The recipe log directory, or the warning to print when there is none.
    pub log_dir: Result<PathBuf, &'static str>,
    /// The auto-drive state root; `None` when HOME and the state directory
    /// are both unset.
    pub state_root: Option<PathBuf>,
    pub manifest: Option<ManifestPath>,
    pub identity: Option<String>,
    pub all: bool,
    pub logins_deadline: Duration,
}

/// The outside sources, injected so tests run none of them.
pub(crate) struct Sources {
    pub prs: Box<dyn PrSource>,
    pub live: Box<dyn LiveSessions>,
    pub origin: Box<dyn OriginSource>,
    /// `None` with `--no-logins`: no section and no probe.
    pub usage: Option<Arc<dyn UsageSource>>,
    /// Unix seconds.
    pub now: i64,
    /// Prints a fixed warning line; it never holds a path or an OS error.
    pub warn: Box<dyn Fn(&'static str)>,
}

/// Everything the table shows. Errors only for an explicit manifest that
/// cannot be used or any malformed manifest line.
pub(crate) fn snapshot(input: &StatusInput, sources: &Sources) -> Result<Snapshot> {
    let manifest = match &input.manifest {
        None => Vec::new(),
        Some(ManifestPath::Default(path)) => manifest::load(path, false)?,
        Some(ManifestPath::Explicit(path)) => manifest::load(path, true)?,
    };
    // Started first so the probe overlaps tmux, gh and git below.
    let logins = sources
        .usage
        .as_ref()
        .map(|usage| PendingLogins::start(usage, input.logins_deadline));
    let (found, warning) = match &input.log_dir {
        Ok(dir) => discover::scan(dir),
        Err(warning) => (Vec::new(), Some(*warning)),
    };
    if let Some(warning) = warning {
        (sources.warn)(warning);
    }
    let runs = discover::select(found, input.identity.as_deref(), input.all);
    let live: HashMap<String, i64> = sources.live.list().into_iter().collect();
    let prs = pr_lists(&runs, sources.prs.as_ref());

    let mut used = vec![false; manifest.len()];
    let mut rows: Vec<(Option<i64>, String, WorkRow)> = runs
        .iter()
        .map(|run| {
            let (start, row) = run_row(run, input, sources, &live, &prs, &manifest, &mut used);
            (start, run.session.clone(), row)
        })
        .collect();
    // Oldest first; an unknown start after every known one.
    rows.sort_by(|a, b| (a.0.is_none(), a.0, &a.1).cmp(&(b.0.is_none(), b.0, &b.1)));
    let mut workstreams: Vec<WorkRow> = rows.into_iter().map(|(_, _, row)| row).collect();
    let only_list = (prs.len() == 1)
        .then(|| prs.values().next().and_then(|l| l.as_deref()))
        .flatten();
    workstreams.extend(
        manifest
            .iter()
            .zip(&used)
            .filter(|(_, used)| !**used)
            .map(|(row, _)| queued_row(row, only_list)),
    );

    Ok(Snapshot {
        no_identity: input.identity.is_none() && !input.all,
        workstreams,
        logins: logins.map(PendingLogins::finish),
    })
}

/// The PR list of every repository the runs name, each asked once; `None`
/// for a repository GitHub could not be asked about.
fn pr_lists(runs: &[Discovered], prs: &dyn PrSource) -> HashMap<String, Option<Vec<PrInfo>>> {
    let repos: BTreeSet<&str> = runs
        .iter()
        .filter_map(|r| r.record.as_ref()?.repo.as_deref())
        .collect();
    repos
        .into_iter()
        .map(|repo| (repo.to_string(), prs.list(repo).ok()))
        .collect()
}

/// A discovered run's row and its start time.
fn run_row(
    run: &Discovered,
    input: &StatusInput,
    sources: &Sources,
    live: &HashMap<String, i64>,
    prs: &HashMap<String, Option<Vec<PrInfo>>>,
    manifest: &[ManifestRow],
    used: &mut [bool],
) -> (Option<i64>, WorkRow) {
    let record = run.record.as_ref();
    let issue = record.and_then(|r| r.issue_number);
    let recorded_pr = record.and_then(|r| r.pr_number);
    let mut matched = manifest_match(manifest, used, issue, recorded_pr);
    let mut pr_number = recorded_pr.or_else(|| matched.and_then(|i| manifest[i].pr));

    // `None`: no repository, or GitHub could not be asked.
    let list = record
        .and_then(|r| r.repo.as_deref())
        .and_then(|repo| prs.get(repo))
        .and_then(Option::as_deref);
    let info = list.and_then(|list| find_pr(list, pr_number, issue));
    pr_number = pr_number.or(info.map(|p| p.number));
    if matched.is_none() {
        matched = manifest_match(manifest, used, issue, pr_number);
    }
    let pr = match pr_number {
        Some(n) => PrCell::Known(n),
        None if list.is_some() || (issue.is_none() && recorded_pr.is_none()) => PrCell::None,
        None => PrCell::Unknown,
    };
    let pr_state = info.and_then(|p| PrState::from_github(&p.state));

    let rounds = pr_number.and_then(|n| {
        let root = input.state_root.as_deref()?;
        let origin = record.and_then(|r| sources.origin.origin(&r.cwd));
        state_dir(root, &state_key(origin.as_deref(), &n.to_string()))
    });
    let (crusty, merge_ready) = rounds.as_deref().map_or((0, 0), |dir| {
        (
            round_count(dir, CRUSTY_ROUND_PREFIX),
            round_count(dir, MERGE_READY_ROUND_PREFIX),
        )
    });
    let stage = stage::classify(&StageInput {
        pr: pr_state,
        merge_ready_rounds: merge_ready,
        crusty_rounds: crusty,
        has_run: true,
    });
    let reviewing = merge_ready == 0
        && crusty > 0
        && !matches!(pr_state, Some(PrState::Merged | PrState::Closed));
    let concerns = rounds
        .as_deref()
        .filter(|_| reviewing)
        .and_then(concern_count);

    let created = live.get(&run.session).copied();
    let start = record.and_then(|r| epoch(&r.started_at)).or(created);
    let finished = record
        .and_then(|r| r.outcome.as_ref())
        .and_then(|o| epoch(&o.finished_at));
    let elapsed_secs = match (start, created, finished) {
        (Some(s), Some(_), _) => Some(sources.now.saturating_sub(s)),
        (Some(s), None, Some(f)) => Some(f.saturating_sub(s)),
        _ => None,
    };
    let running = created.is_some() && !stage.closed;
    let reason = (!running).then(|| {
        let class = match record {
            Some(r) => r.outcome.as_ref().map(|o| o.class.clone()),
            None => run.log.as_deref().and_then(log_class),
        };
        stage::stop_reason(class.as_deref(), pr_state == Some(PrState::Merged))
    });

    let label = matched
        .and_then(|i| manifest[i].label.clone())
        .unwrap_or_else(|| default_label(record, &run.session));
    let row = WorkRow {
        running,
        label,
        issue,
        pr,
        percent: stage.percent,
        elapsed_secs,
        stage: stage.label,
        concerns,
        conflicts: info.is_some_and(|p| p.mergeable == "CONFLICTING"),
        reason: reason.flatten(),
    };
    (start, row)
}

/// The first manifest row the run matches, marked used.
fn manifest_match(
    manifest: &[ManifestRow],
    used: &mut [bool],
    issue: Option<u64>,
    pr: Option<u64>,
) -> Option<usize> {
    let i = manifest
        .iter()
        .position(|row| manifest::matches(row, issue, pr))?;
    used[i] = true;
    Some(i)
}

/// The PR numbered `pr`, else the first whose title names `issue`.
fn find_pr(list: &[PrInfo], pr: Option<u64>, issue: Option<u64>) -> Option<&PrInfo> {
    match (pr, issue) {
        (Some(n), _) => list.iter().find(|p| p.number == n),
        (None, Some(i)) => stage::pr_for_issue(list, i),
        (None, None) => None,
    }
}

/// `<recipe> #<issue>`, or the session name.
fn default_label(record: Option<&RunRecord>, session: &str) -> String {
    match record.and_then(|r| Some((&r.recipe, r.issue_number?))) {
        Some((recipe, issue)) => format!("{recipe} #{issue}"),
        None => session.to_string(),
    }
}

/// The class from a run without a record: only a marker line counts.
fn log_class(log: &Path) -> Option<String> {
    run_record::classify_tail(&run_record::read_tail(log), None).map(str::to_string)
}

fn epoch(rfc3339: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|t| t.timestamp())
}

/// A manifest row no run matched. Its PR state is looked up only when the
/// runs name exactly one repository, so a number is never matched against
/// another repository's PRs.
fn queued_row(row: &ManifestRow, list: Option<&[PrInfo]>) -> WorkRow {
    let info = list.and_then(|list| find_pr(list, row.pr, row.issue));
    let pr_state = info.and_then(|p| PrState::from_github(&p.state));
    let stage = stage::classify(&StageInput {
        pr: pr_state,
        merge_ready_rounds: 0,
        crusty_rounds: 0,
        has_run: false,
    });
    let label = row
        .label
        .clone()
        .unwrap_or_else(|| match (row.issue, row.pr) {
            (Some(issue), _) => format!("issue #{issue}"),
            (None, Some(pr)) => format!("PR #{pr}"),
            (None, None) => "-".to_string(),
        });
    WorkRow {
        running: false,
        label,
        issue: row.issue,
        pr: row
            .pr
            .or(info.map(|p| p.number))
            .map_or(PrCell::None, PrCell::Known),
        percent: stage.percent,
        elapsed_secs: None,
        stage: stage.label,
        concerns: None,
        conflicts: info.is_some_and(|p| p.mergeable == "CONFLICTING"),
        reason: None,
    }
}

/// The logins section: the probe runs on a worker thread from the start of
/// [`snapshot`]; at the deadline the cancel flag is set and the section is
/// built at most [`CANCEL_GRACE`] later, with `no reading` for every
/// profile that has no result. A probe that still has not returned is left
/// to finish on its detached thread.
enum PendingLogins {
    Ready(LoginsSection),
    Probing {
        profiles: Vec<(String, bool)>,
        rx: mpsc::Receiver<Result<Vec<crate::commands::profile::usage_cmd::Row>, String>>,
        cancel: Arc<AtomicBool>,
        due: Instant,
    },
}

impl PendingLogins {
    fn start(usage: &Arc<dyn UsageSource>, deadline: Duration) -> Self {
        let due = Instant::now() + deadline;
        let profiles = match usage.profiles() {
            Ok(profiles) => profiles,
            Err(reason) => return Self::Ready(LoginsSection::Refused(reason)),
        };
        if profiles.iter().all(|(_, api_key)| *api_key) {
            return Self::Ready(LoginsSection::Rows(logins::login_rows(
                &profiles,
                Some(&[]),
            )));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (worker_usage, worker_cancel) = (Arc::clone(usage), Arc::clone(&cancel));
        let worker_profiles = profiles.clone();
        std::thread::spawn(move || {
            let _ = tx.send(worker_usage.probe(&worker_profiles, &worker_cancel));
        });
        Self::Probing {
            profiles,
            rx,
            cancel,
            due,
        }
    }

    fn finish(self) -> LoginsSection {
        let (profiles, rx, cancel, due) = match self {
            Self::Ready(section) => return section,
            Self::Probing {
                profiles,
                rx,
                cancel,
                due,
            } => (profiles, rx, cancel, due),
        };
        let left = due.saturating_duration_since(Instant::now());
        let result = rx.recv_timeout(left).ok().or_else(|| {
            cancel.store(true, Ordering::SeqCst);
            rx.recv_timeout(CANCEL_GRACE).ok()
        });
        match result {
            Some(Err(reason)) => LoginsSection::Refused(reason),
            // A probe cut short by the cancel flag reports it as a timeout.
            Some(Ok(rows)) => LoginsSection::Rows(logins::login_rows(&profiles, Some(&rows))),
            None => LoginsSection::Rows(logins::login_rows(&profiles, None)),
        }
    }
}
