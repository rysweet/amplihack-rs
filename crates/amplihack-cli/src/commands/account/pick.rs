//! One pick (issue #2006): read every account live without the lock, then,
//! under the lock, count running jobs, sweep and load the leases, rank, and
//! write this pick's lease. No child process or network read runs while the
//! lock is held, and a lease's home is only ever compared with candidate
//! homes, never chosen.

use super::Cli;
use super::lease::{Lease, LeaseDir};
use super::procscan;
use super::rank::{self, Scored};
use super::reading::{Candidate, checked};
use std::path::PathBuf;

/// Every candidate as read, and the time of the earliest read.
#[derive(Debug, Clone, PartialEq)]
pub struct Readings {
    pub candidates: Vec<Candidate>,
    pub read_at: i64,
}

/// What a pick depends on, injected so tests use a fixed clock, a fake
/// `/proc`, a private lease directory and fixed readings.
pub struct PickEnv<'a> {
    pub now: i64,
    pub proc_root: PathBuf,
    pub lease_root: PathBuf,
    /// Not counted as a job.
    pub self_pid: u32,
    /// Only this real uid's processes are counted.
    pub uid: u32,
    /// The pid recorded in the lease: the launcher's own for a launch,
    /// `None` for a standalone pick.
    pub lease_pid: Option<u32>,
    pub job_cost_pct: f64,
    pub ttl_secs: u64,
    pub read: &'a (dyn Fn(Cli) -> Readings + Sync),
    pub alive: &'a (dyn Fn(u32) -> bool + Sync),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickOpts {
    /// `false` for `--list`: rank only, write nothing.
    pub write_lease: bool,
}

/// One candidate with its jobs, leases and effective headroom.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub candidate: Candidate,
    pub running_jobs: usize,
    pub active_leases: usize,
    pub effective_headroom: Option<f64>,
}

#[derive(Debug)]
pub struct Pick {
    /// Index into `ranked`.
    pub chosen: Option<usize>,
    pub ranked: Vec<Ranked>,
    pub read_at: i64,
    /// The lease written for this pick.
    pub lease: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoUsableReason {
    NoAccounts,
    NoneReadable,
    NoHeadroom,
}

impl NoUsableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoAccounts => "no accounts found",
            Self::NoneReadable => "none signed in or readable",
            Self::NoHeadroom => "none has headroom after running jobs and leases",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PickError {
    #[error("no usable {cli} account ({})", reason.as_str())]
    NoUsable {
        cli: &'static str,
        ranked: Vec<Ranked>,
        read_at: i64,
        reason: NoUsableReason,
    },
    #[error("could not {op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// `c` with `1 job`, `N jobs`, `1 lease` or `N leases` added to its note.
fn with_counts(mut c: Candidate, jobs: usize, leases: usize) -> Candidate {
    let plural = |n: usize, one: &str| match n {
        1 => format!("1 {one}"),
        n => format!("{n} {one}s"),
    };
    for (n, what) in [(jobs, "job"), (leases, "lease")] {
        if n > 0 && c.note != "unsafe path" {
            if !c.note.is_empty() {
                c.note.push_str(", ");
            }
            c.note.push_str(&plural(n, what));
        }
    }
    c
}

/// Picks the `cli` account with the most effective headroom and, unless
/// `opts.write_lease` is `false`, records a lease for it.
pub fn pick(env: &PickEnv<'_>, cli: Cli, opts: PickOpts) -> Result<Pick, PickError> {
    // Read first, without the lock: picks must not queue behind reads.
    let readings = (env.read)(cli);
    let candidates: Vec<Candidate> = readings
        .candidates
        .into_iter()
        .map(|mut c| {
            // Canonicalised before the path check, so a symlink cannot
            // pass the check and then resolve to an unsafe target.
            match std::fs::canonicalize(&c.home) {
                Ok(canon) => {
                    c.home = canon;
                    checked(c)
                }
                Err(_) => {
                    c.usable = false;
                    c.note = "unresolvable path".to_string();
                    c
                }
            }
        })
        .collect();
    let homes: Vec<PathBuf> = candidates.iter().map(|c| c.home.clone()).collect();

    let dir = LeaseDir::open(&env.lease_root, cli)?;
    let guard = dir.lock()?;
    let (jobs, job_warnings) =
        procscan::count_jobs(&env.proc_root, cli.env_key(), &homes, env.self_pid, env.uid);
    let (stored, lease_warnings) = guard.load();
    for w in job_warnings.into_iter().chain(lease_warnings) {
        eprintln!("{w}");
    }
    let leases = guard.sweep(stored, &jobs, env.now, env.ttl_secs, env.alive);

    let scored: Vec<Scored> = candidates
        .iter()
        .map(|c| {
            let n_leases = leases.iter().filter(|s| s.lease.home == c.home).count();
            Scored::new(
                c.home.clone(),
                c.live_headroom,
                c.usable,
                jobs.count(&c.home),
                n_leases,
                env.job_cost_pct,
            )
        })
        .collect();
    let order = rank::order(&scored);
    let ranked: Vec<Ranked> = order
        .iter()
        .map(|&i| Ranked {
            candidate: with_counts(candidates[i].clone(), scored[i].jobs, scored[i].leases),
            running_jobs: scored[i].jobs,
            active_leases: scored[i].leases,
            effective_headroom: scored[i].effective,
        })
        .collect();
    // `order[0]` is `ranked[0]`, so the chosen row is always 0.
    let chosen = rank::choose(&scored, &order).map(|_| 0);

    if !opts.write_lease {
        return Ok(Pick {
            chosen,
            ranked,
            read_at: readings.read_at,
            lease: None,
        });
    }
    let Some(chosen) = chosen else {
        let reason = if ranked.is_empty() {
            NoUsableReason::NoAccounts
        } else if ranked.iter().all(|r| r.candidate.live_headroom.is_none()) {
            NoUsableReason::NoneReadable
        } else {
            NoUsableReason::NoHeadroom
        };
        return Err(PickError::NoUsable {
            cli: cli.as_str(),
            ranked,
            read_at: readings.read_at,
            reason,
        });
    };
    let lease = guard.write(&Lease::new(
        cli,
        ranked[chosen].candidate.home.clone(),
        env.lease_pid,
        env.now,
    ))?;
    drop(guard);
    Ok(Pick {
        chosen: Some(chosen),
        ranked,
        read_at: readings.read_at,
        lease: Some(lease),
    })
}
