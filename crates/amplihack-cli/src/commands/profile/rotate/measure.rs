//! Measuring: the live login and every candidate, in parallel, with the
//! `profile usage` probe and its settings (`probe_limits`): the method, the
//! deadline and the bound. The live login is probed with `CLAUDE_CONFIG_DIR`
//! removed, so `claude` uses `~/.claude`. Claude's output is parsed and
//! dropped; only percentages, reset times and states leave this module.

use super::cycle::Ctx;
use super::decide::Probed;
use super::inflight::{InFlight, Job};
use crate::binary_finder::BinaryInfo;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::probe_limits::{self, ProbeMode};
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd::{self, Probe, RowReader};
use crate::commands::profile::usage_parse::UsageState;
use crate::commands::profile::usage_source::{ClaudeGate, LazyBinary, SourceEnv};
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

/// Whose login is in `~/.claude`.
pub enum Live<'a> {
    Profile(&'a ProfileName),
    /// No profile owns it yet.
    Unowned,
    /// An API-key profile (issue #1700): never probed, since a probe costs
    /// tokens or times out; its reading is [`Probed::platform`].
    Platform,
}

impl Live<'_> {
    /// Whether the live login is probed: an API-key one never is.
    pub fn probed(&self) -> bool {
        !matches!(self, Self::Platform)
    }
}

/// The reading for `p`. A failed or timed-out probe counts as "swap needed",
/// so its cause goes to stderr (the daemon's `rotate.err`); the text is fixed
/// and never Claude's output. Both read `Failed`: never a target and never
/// exhausted, and probed again next cycle. Both are also unknown, which
/// blocks a move onto overflow. A profile directory refused just before the
/// spawn is no exception (issue #1698): that login may have capacity. From
/// the next cycle the profile is skipped as `directory refused` before any
/// probe, and that skip holds the move until the directory is fixed.
pub(super) fn reading(who: &str, p: &Probe) -> Probed {
    if p.error.is_some() || p.report.state == UsageState::Error {
        let why = p
            .error
            .map_or_else(|| "unreadable /usage output".to_string(), |e| e.to_string());
        eprintln!("warning: rotate: probe of {who} failed: {why}");
        return Probed::failed();
    }
    if p.report.state == UsageState::Timeout {
        eprintln!("warning: rotate: probe of {who} timed out");
    }
    Probed::from_report(&p.report)
}

/// The probe method, as `profile usage` resolves it. A bad variable falls
/// back to the default order and warns on stderr, once per cycle.
fn mode() -> ProbeMode {
    let (mode, warning) = probe_limits::probe_mode_from_env();
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    mode
}

/// The probe deadline, as `profile usage` resolves it without a flag. A bad
/// variable falls back to the default and warns on stderr, once per cycle,
/// as a bad parallelism variable does.
fn timeout() -> Duration {
    let (secs, warning) = probe_limits::timeout_from_env(None);
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    Duration::from_secs(secs)
}

/// The live probe and each candidate's by `claude` only, in the order of
/// `candidates`, with reset times kept for exhausted readings too: the held
/// cycle's spend-limit probe, which only `/usage` answers. The method,
/// deadline and bound are those `profile usage` uses; the live probe counts
/// against the bound.
pub fn measure(
    store: &ProfileStore,
    binary: &BinaryInfo,
    live: Live<'_>,
    candidates: &[ProfileName],
    now: i64,
) -> (Probed, Vec<(ProfileName, Probed)>) {
    let mode = mode();
    let timeout = timeout();
    let probes = candidates.len() + usize::from(live.probed());
    let (bound, warning) = probe_limits::parallel_from_host(probes);
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    // As `profile usage` warns: with one of these set, every probe may
    // report that credential, so the readings say nothing about the logins.
    if let Some(var) = usage_cmd::env_credential_var(|k| std::env::var_os(k)) {
        eprintln!("{}", usage_cmd::env_credential_warning(var));
    }
    measure_with(store, binary, live, candidates, now, mode, timeout, bound)
}

/// [`measure`] with the method, deadline and bound given. The live probe and
/// the candidates share one pool of `bound` slots, the live probe first, so
/// at most `bound` run at once and a slot freed by any probe, the live one
/// included, goes to the next candidate. Claude's output is never kept.
#[allow(clippy::too_many_arguments)]
pub(super) fn measure_with(
    store: &ProfileStore,
    binary: &BinaryInfo,
    live: Live<'_>,
    candidates: &[ProfileName],
    now: i64,
    mode: ProbeMode,
    timeout: Duration,
    bound: usize,
) -> (Probed, Vec<(ProfileName, Probed)>) {
    let probe_live = || {
        let p = match live {
            Live::Platform => return Probed::platform(),
            Live::Profile(name) => {
                usage_cmd::probe_profile(store, binary, name, true, mode, timeout, now, false)
            }
            Live::Unowned => usage_cmd::probe_home(binary, mode, timeout, now, false),
        };
        reading("~/.claude", &p)
    };
    // Job 0 is the live probe, unless the live login is not probed.
    let skip = usize::from(live.probed());
    let jobs = skip + candidates.len();
    let mut probed = usage_cmd::pooled(jobs, bound, |i| match i.checked_sub(skip) {
        // A panic in the live probe is a bug; it reads as a failed probe.
        None => std::panic::catch_unwind(std::panic::AssertUnwindSafe(&probe_live))
            .unwrap_or_else(|_| Probed::failed()),
        Some(c) => {
            let name = &candidates[c];
            let p = usage_cmd::probe_profile(store, binary, name, false, mode, timeout, now, false);
            reading(name.as_str(), &p)
        }
    });
    let live = if skip == 1 {
        probed.remove(0)
    } else {
        Probed::platform()
    };
    (live, candidates.iter().cloned().zip(probed).collect())
}

// ---------------------------------------------------------------------------
// Issue #1844: reading in order, with reads that may outlast the cycle
// ---------------------------------------------------------------------------

/// Which login a read is for. The live login is its own key, so a read
/// started while a profile was live is never used once it is not.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// The login in `~/.claude`, owned by this profile.
    Live(ProfileName),
    /// The login in `~/.claude`, owned by no profile.
    Unowned,
    /// A candidate in its own directory.
    Candidate(ProfileName),
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Live(_) | Self::Unowned => f.write_str("~/.claude"),
            Self::Candidate(n) => f.write_str(n.as_str()),
        }
    }
}

/// How long a cycle waits for its reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Until this instant; reads still running are used by a later cycle.
    Until(Instant),
    /// Until every read is done: the one-shot command and `--dry-run`.
    Full,
}

/// Two seconds before the next cycle is due, and at least one second after
/// `start`.
pub fn cycle_deadline(start: Instant, interval: Duration) -> Instant {
    let wait = interval
        .saturating_sub(Duration::from_secs(2))
        .max(Duration::from_secs(1));
    start + wait
}

/// One reading per key, in the order of `keys`: collect what earlier reads
/// returned, start a read for each key with no result and none in flight,
/// wait as `wait` says, then read each key. A key still in flight reads
/// failed and unknown, with `still_running` set; a later call uses its
/// result.
pub fn gather<K>(
    fl: &mut InFlight<K, Probe>,
    keys: &[K],
    mut make: impl FnMut(&K) -> Job<Probe>,
    wait: Wait,
) -> Vec<(K, Probed)>
where
    K: Clone + Eq + Hash + Send + fmt::Display + 'static,
{
    for k in fl.drop_stale(Instant::now()) {
        eprintln!("warning: rotate: read of {k} still running after its limit; dropped");
    }
    let mut got: HashMap<K, Probe> = fl.collect().into_iter().collect();
    let jobs: Vec<(K, Job<Probe>)> = keys
        .iter()
        .filter(|k| !got.contains_key(*k) && !fl.busy(k))
        .map(|k| (k.clone(), make(k)))
        .collect();
    let bound = fl.bound();
    fl.start_all(jobs, bound);
    let more = match wait {
        Wait::Until(t) => fl.wait(t),
        Wait::Full => fl.wait_all(),
    };
    got.extend(more);
    keys.iter()
        .map(|k| {
            let probed = match got.remove(k) {
                Some(p) => reading(&k.to_string(), &p),
                None if fl.busy(k) => {
                    let mut p = Probed::failed();
                    p.still_running = true;
                    p
                }
                None => Probed::failed(),
            };
            (k.clone(), probed)
        })
        .collect()
}

/// The live login and each candidate read in order (passive, direct,
/// `claude`), as `profile usage` reads them. A watching rotator keeps reads
/// that outlast the cycle in `ctx.in_flight` and waits only until two
/// seconds before the next cycle; any other cycle waits for every read.
pub fn measure_ordered(
    ctx: &Ctx,
    live: Live<'_>,
    candidates: &[ProfileName],
    now: i64,
) -> (Probed, Vec<(ProfileName, Probed)>) {
    let start = Instant::now();
    let mode = mode();
    let timeout = timeout();
    let n = candidates.len() + usize::from(live.probed());
    let (claude_bound, warning) = probe_limits::parallel_from_host(n);
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    let (direct_bound, warning) = probe_limits::direct_parallel_from_env(n);
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    if let Some(var) = usage_cmd::env_credential_var(|k| std::env::var_os(k)) {
        eprintln!("{}", usage_cmd::env_credential_warning(var));
    }
    let bound = match mode {
        ProbeMode::Auto | ProbeMode::Text => claude_bound,
        ProbeMode::Ordered | ProbeMode::Direct => direct_bound,
    };
    let reader = Arc::new(RowReader {
        store: ctx.store.clone(),
        env: SourceEnv::from_env(mode, timeout),
        binary: LazyBinary::claude(),
        gate: ClaudeGate::new(claude_bound),
        home: Some(ctx.home.clone()),
        no_refresh: usage_cmd::no_refresh_dirs(&ctx.store),
        overflow: crate::commands::profile::overflow::overflow_names(ctx.store.root()),
        now,
        keep_raw: false,
    });
    let mut keys = Vec::with_capacity(n);
    match live {
        Live::Profile(name) => keys.push(Key::Live(name.clone())),
        Live::Unowned => keys.push(Key::Unowned),
        Live::Platform => {}
    }
    keys.extend(candidates.iter().cloned().map(Key::Candidate));
    let make = |k: &Key| -> Job<Probe> {
        let reader = Arc::clone(&reader);
        let k = k.clone();
        Box::new(move || match &k {
            Key::Live(name) => reader.profile(name, true),
            Key::Unowned => reader.home(),
            Key::Candidate(name) => reader.profile(name, false),
        })
    };
    let mut read = match &ctx.in_flight {
        Some(fl) => {
            let mut fl = fl.lock().unwrap_or_else(PoisonError::into_inner);
            fl.set_bound(bound);
            let until = cycle_deadline(start, Duration::from_secs(ctx.settings.interval));
            gather(&mut fl, &keys, make, Wait::Until(until))
        }
        None => {
            let mut fl = InFlight::new(super::inflight::max_age(timeout));
            fl.set_bound(bound);
            gather(&mut fl, &keys, make, Wait::Full)
        }
    };
    let live = if live.probed() {
        read.remove(0).1
    } else {
        Probed::platform()
    };
    let others = candidates
        .iter()
        .cloned()
        .zip(read.into_iter().map(|(_, p)| p))
        .collect();
    (live, others)
}
