//! `amplihack workstreams status` (issue #1710): a plain-text table of the
//! workstreams the calling session started — detached recipe runs and the
//! runs they started — with stage and progress, and of Claude login capacity.
//! Built for an agent to paste inline into its reply: at most 100 columns, no
//! colour codes, always finishes, exit 0 unless the arguments are wrong.

pub(crate) mod autodrive;
pub(crate) mod discover;
pub(crate) mod logins;
pub(crate) mod logptr;
pub(crate) mod manifest;
pub(crate) mod meta;
pub(crate) mod render;
pub(crate) mod sources;
pub(crate) mod stage;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};

use crate::commands::profile::usage_cmd::sanitize_display;
use discover::{Run, Scope};
use manifest::ManifestRow;
use render::{Report, WorkRow};
use sources::{Pr, Sources};
use stage::{PrLookup, RunFacts};

pub(crate) const DEADLINE_ENV: &str = "AMPLIHACK_WORKSTREAMS_LOGINS_DEADLINE_SECS";
const DEFAULT_DEADLINE_SECS: u64 = 20;
const MAX_DEADLINE_SECS: u64 = 600;
/// Manifests larger than this are refused.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// PR lists fetched per status at most.
const MAX_REPOS: usize = 8;
const NO_SESSION: &str =
    "no amplihack session id in this environment; run with --all to list every run on the host";

#[derive(Subcommand, Debug)]
pub enum WorkstreamsCommands {
    /// Show this session's workstreams and login capacity as a plain-text table
    Status(WorkstreamsStatusArgs),
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkstreamsStatusArgs {
    /// List every run on this host, not only the runs this session started
    #[arg(long)]
    pub all: bool,
    /// Read labels and queued items from this file instead of
    /// ~/.amplihack/work/workstreams.tsv
    #[arg(long, value_name = "PATH")]
    pub manifest: Option<PathBuf>,
    /// Skip the logins section
    #[arg(long)]
    pub no_logins: bool,
    /// Stop waiting for login readings after this many seconds (1-600,
    /// default 20; or AMPLIHACK_WORKSTREAMS_LOGINS_DEADLINE_SECS)
    #[arg(long, value_name = "SECS", value_parser = clap::value_parser!(u64).range(1..=600))]
    pub logins_deadline: Option<u64>,
}

/// Dispatch a `workstreams` subcommand with the real sources.
pub fn dispatch(command: WorkstreamsCommands) -> anyhow::Result<()> {
    match command {
        WorkstreamsCommands::Status(args) => {
            let code = run(&args);
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
    }
}

fn run(args: &WorkstreamsStatusArgs) -> i32 {
    let home = crate::user_config::home_dir();
    let log_dir = crate::commands::recipe::detach::log_dir();
    if log_dir.is_err() {
        // A fixed message: the error text could name a path.
        eprintln!("warning: recipe log directory cannot be resolved; no runs listed");
    }
    let log_dir = log_dir.ok();
    let state_root = home
        .as_deref()
        .map(|h| autodrive::state_root(std::env::var_os("AMPLIHACK_STATE_DIR").as_deref(), h));
    let default_manifest = home
        .as_deref()
        .map(|h| h.join(".amplihack").join("work").join("workstreams.tsv"));
    let prs = sources::GhPrs::default();
    let src = Sources {
        clock: &sources::SystemClock,
        live: &sources::TmuxSessions,
        prs: &prs,
        git: &sources::GitCli,
        usage: &sources::ProfileUsage,
        log_dir,
        state_root,
        default_manifest,
    };
    let env = |k: &str| std::env::var(k).ok();
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    run_with(args, &src, &env, &mut out, &mut err)
}

/// The whole command over injected sources. Returns the exit code: 0, or 2
/// for an explicit `--manifest` that cannot be read or a table that cannot be
/// written (a closed pipe still gives 0).
pub(crate) fn run_with(
    args: &WorkstreamsStatusArgs,
    src: &Sources<'_>,
    env: &dyn Fn(&str) -> Option<String>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let begun = Instant::now();

    // The manifest first: an unreadable explicit one is a usage error, and
    // then no probe should have started.
    let manifest_rows = match load_manifest(args.manifest.as_deref(), src, err) {
        Ok(rows) => rows,
        Err(()) => return 2,
    };

    // Start the probes before discovery so they run while it does.
    let usage = if args.no_logins {
        None
    } else {
        let secs = deadline_secs(args.logins_deadline, env, err);
        let deadline = begun + Duration::from_secs(secs);
        match src.usage.start(Duration::from_secs(secs)) {
            Ok(stream) => Some((stream, deadline)),
            Err(_) => {
                // A fixed message: an error chain could name a profile path.
                let _ = writeln!(err, "warning: login usage cannot be read");
                None
            }
        }
    };

    let scope = discover::scope_from_env(args.all, env);
    let mut runs = Vec::new();
    if let (Some(dir), false) = (&src.log_dir, scope == Scope::None) {
        let found = discover::discover(dir, &scope);
        for w in &found.warnings {
            let _ = writeln!(err, "{w}");
        }
        runs = found.runs;
    }

    let live = src.live.list();
    let live_created: HashMap<&str, Option<chrono::DateTime<chrono::Utc>>> =
        live.iter().map(|s| (s.name.as_str(), s.created)).collect();
    let repos = repos_of(&runs, src, err);
    let now = src.clock.now().to_utc();

    let entries = manifest::merge(manifest_rows, runs);
    let mut state_truncated = false;
    let rows = entries
        .into_iter()
        .map(|e| {
            let Some(run) = e.run else {
                return WorkRow {
                    label: e.label,
                    issue: e.issue,
                    pr: e.pr,
                    status: stage::status(
                        &RunFacts {
                            queued: true,
                            ..RunFacts::default()
                        },
                        now,
                    ),
                };
            };
            let repo = run.cwd.as_ref().and_then(|c| repos.get(c));
            let lookup = match repo {
                Some(r) => stage::find_pr(
                    r.prs.as_deref(),
                    run.pr.or(e.pr),
                    run.pointer_pr,
                    run.branch.as_deref(),
                    run.issue.or(e.issue),
                ),
                // No directory to ask: unknown only if there is a PR to know.
                None if run.pr.or(run.pointer_pr).or(e.pr).is_some() => PrLookup::Unknown,
                None => PrLookup::None,
            };
            let pr_number = match &lookup {
                PrLookup::Found(p) => Some(p.number),
                _ => e.pr.or(run.pr).or(run.pointer_pr),
            };
            let autodrive = repo.and_then(|r| {
                let root = src.state_root.as_deref()?;
                read_autodrive(root, r.origin.as_deref(), pr_number, run.branch.as_deref())
            });
            state_truncated |= autodrive.as_ref().is_some_and(|a| a.truncated);
            let live_entry = live_created.get(run.session.as_str());
            let facts = RunFacts {
                queued: false,
                live: live_entry.is_some(),
                pr: lookup,
                autodrive,
                exit_code: run.exit_code,
                started: run.started.or_else(|| live_entry.copied().flatten()),
                stopped_on_limit: run.stopped_on_limit,
            };
            WorkRow {
                label: e.label,
                issue: e.issue.or(run.issue),
                pr: pr_number,
                status: stage::status(&facts, now),
            }
        })
        .collect();
    if state_truncated {
        // No path: the state dir name can hold the origin URL (SR-24).
        let _ = writeln!(
            err,
            "warning: more than 4096 entries in the auto-drive state directory; \
             some rounds may be missing"
        );
    }

    let logins = usage.map(|(stream, deadline)| logins::collect(stream, deadline));
    let report = Report {
        now: src.clock.now(),
        note: (scope == Scope::None).then(|| NO_SESSION.to_string()),
        rows,
        logins,
    };
    match out
        .write_all(render::render(&report).as_bytes())
        .and_then(|()| out.flush())
    {
        Ok(()) => 0,
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => 0,
        Err(_) => 2,
    }
}

/// The logins deadline: the flag, else the environment variable (1–600; a
/// bad value warns once), else 20 seconds.
fn deadline_secs(
    flag: Option<u64>,
    env: &dyn Fn(&str) -> Option<String>,
    err: &mut dyn Write,
) -> u64 {
    if let Some(secs) = flag {
        return secs;
    }
    let Some(raw) = env(DEADLINE_ENV).filter(|v| !v.is_empty()) else {
        return DEFAULT_DEADLINE_SECS;
    };
    match raw.trim().parse::<u64>() {
        Ok(secs) if (1..=MAX_DEADLINE_SECS).contains(&secs) => secs,
        _ => {
            let shown: String = sanitize_display(&raw, false).chars().take(32).collect();
            let _ = writeln!(
                err,
                "warning: ignored {DEADLINE_ENV} \"{shown}\" (use 1 to {MAX_DEADLINE_SECS}); using {DEFAULT_DEADLINE_SECS}"
            );
            DEFAULT_DEADLINE_SECS
        }
    }
}

/// The manifest rows. An explicit path must be a readable regular file that
/// is not a symlink, else an error and `Err`. The default file is skipped
/// silently when missing, and with a warning when it is a symlink, not a
/// regular file, not owned by this user, or writable by others.
fn load_manifest(
    explicit: Option<&Path>,
    src: &Sources<'_>,
    err: &mut dyn Write,
) -> Result<Vec<ManifestRow>, ()> {
    let text = if let Some(path) = explicit {
        match read_explicit(path) {
            Ok(text) => text,
            Err(reason) => {
                let _ = writeln!(
                    err,
                    "error: cannot read the manifest {}: {reason}",
                    sanitize_display(&path.display().to_string(), false)
                );
                return Err(());
            }
        }
    } else {
        let Some(path) = src.default_manifest.as_deref() else {
            return Ok(Vec::new());
        };
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            _ => {}
        }
        match meta::read_private(path, MAX_MANIFEST_BYTES) {
            Some(body) => String::from_utf8_lossy(&body).into_owned(),
            None => {
                let _ = writeln!(
                    err,
                    "warning: skipped the default manifest: it is a symlink, not a regular file, \
                     not owned by you, writable by others, or larger than 1 MiB"
                );
                return Ok(Vec::new());
            }
        }
    };
    let (rows, warnings) = manifest::parse(&text);
    for w in warnings {
        let _ = writeln!(err, "warning: {w}");
    }
    Ok(rows)
}

fn read_explicit(path: &Path) -> Result<String, &'static str> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "no such file",
            std::io::ErrorKind::PermissionDenied => "permission denied",
            _ if e.raw_os_error() == Some(libc::ELOOP) => {
                "it is a symlink; pass the path of its target"
            }
            _ => "it cannot be opened",
        })?;
    let m = file.metadata().map_err(|_| "it cannot be read")?;
    if !m.is_file() {
        return Err("it is not a regular file");
    }
    if m.len() > MAX_MANIFEST_BYTES {
        return Err("it is larger than 1 MiB");
    }
    let mut body = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|_| "it cannot be read")?;
    if body.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("it is larger than 1 MiB");
    }
    String::from_utf8(body).map_err(|_| "it is not UTF-8 text")
}

/// One repo's facts: its origin and its PR list (`None` when unreadable).
struct Repo {
    origin: Option<String>,
    prs: Option<Vec<Pr>>,
}

/// The repo of every run directory: origins, then one PR list per distinct
/// repo (origin, else directory), at most [`MAX_REPOS`], fetched in parallel.
fn repos_of(runs: &[Run], src: &Sources<'_>, err: &mut dyn Write) -> HashMap<PathBuf, Repo> {
    let mut dirs: Vec<&PathBuf> = runs.iter().filter_map(|r| r.cwd.as_ref()).collect();
    dirs.sort();
    dirs.dedup();
    // Only the Sync sources cross into the worker threads.
    let (git, prs) = (src.git, src.prs);
    let origins: Vec<(PathBuf, Option<String>)> = std::thread::scope(|s| {
        let handles: Vec<_> = dirs
            .iter()
            .map(|d| s.spawn(move || ((*d).clone(), git.origin(d))))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    // One directory per repo asks for its PR list.
    let mut repo_dir: Vec<(String, PathBuf)> = Vec::new();
    for (dir, origin) in &origins {
        let key = origin.clone().unwrap_or_else(|| dir.display().to_string());
        if !repo_dir.iter().any(|(k, _)| *k == key) {
            repo_dir.push((key, dir.clone()));
        }
    }
    if repo_dir.len() > MAX_REPOS {
        let _ = writeln!(
            err,
            "warning: runs span {} repositories; PR state is read for the first {MAX_REPOS} only",
            repo_dir.len()
        );
        repo_dir.truncate(MAX_REPOS);
    }
    let lists: HashMap<String, Option<Vec<Pr>>> = std::thread::scope(|s| {
        let handles: Vec<_> = repo_dir
            .iter()
            .map(|(key, dir)| s.spawn(move || (key.clone(), prs.list(dir).ok())))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    origins
        .into_iter()
        .map(|(dir, origin)| {
            let key = origin.clone().unwrap_or_else(|| dir.display().to_string());
            let prs = lists.get(&key).cloned().flatten();
            (dir, Repo { origin, prs })
        })
        .collect()
}

/// The run's auto-drive state: the dir keyed by its PR number, else by its
/// branch, as `autodrive_state_key` names them.
fn read_autodrive(
    root: &Path,
    origin: Option<&str>,
    pr: Option<u64>,
    branch: Option<&str>,
) -> Option<autodrive::AutoDriveState> {
    let by_pr =
        pr.and_then(|n| autodrive::read(&root.join(autodrive::state_key(origin, &n.to_string()))));
    by_pr.or_else(|| {
        let b = branch.filter(|b| !b.is_empty())?;
        autodrive::read(&root.join(autodrive::state_key(origin, b)))
    })
}
