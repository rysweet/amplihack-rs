//! Measuring: the live login and every candidate, in parallel, with the
//! `profile usage` probe and its settings (`probe_limits`): the method, the
//! deadline and the bound. The live login is probed with `CLAUDE_CONFIG_DIR`
//! removed, so `claude` uses `~/.claude`. Claude's output is parsed and
//! dropped; only percentages, reset times and states leave this module.

use super::cycle::Ctx;
use super::decide::Probed;
pub(super) use super::inflight::join_pending;
use super::inflight::{InFlight, Job};
use crate::binary_finder::BinaryInfo;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::probe_limits::{self, ProbeMode};
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd::{self, Probe, RowReader};
use crate::commands::profile::usage_parse::UsageState;
use crate::commands::profile::usage_source::{self, ClaudeGate, LazyBinary, SourceEnv};
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
    /// An excluded profile (issue #2218): never probed; [`Probed::excluded`].
    Excluded,
}

impl Live<'_> {
    /// Whether the live login is probed: API-key and excluded ones are not.
    pub fn probed(&self) -> bool {
        !matches!(self, Self::Platform | Self::Excluded)
    }

    /// The reading of a live login that is not probed.
    fn unprobed(&self) -> Probed {
        match self {
            Self::Excluded => Probed::excluded(),
            _ => Probed::platform(),
        }
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
    // SR-10: logged whether or not the row fell back to `claude`.
    if p.direct_failure.is_some_and(usage_source::not_saved) {
        eprintln!("{}", usage_cmd::not_saved_warning(who));
    }
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
/// back to the default order and warns on stderr. Only [`measure_ordered`]
/// calls this, once per cycle, so the warning is printed once per cycle;
/// anything else in the cycle uses [`mode_quiet`].
pub(super) fn mode() -> ProbeMode {
    let (mode, warning) = probe_limits::probe_mode_from_env();
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    mode
}

/// [`mode`] with no warning: for a read later in a cycle whose
/// [`measure_ordered`] has already warned about the same variable (the
/// freshness gate's re-read, issue #1870 review round 4).
pub(super) fn mode_quiet() -> ProbeMode {
    probe_limits::probe_mode_from_env().0
}

/// The probe deadline, as `profile usage` resolves it without a flag. A bad
/// variable falls back to the default and warns on stderr, as a bad
/// parallelism variable does. Only [`measure_ordered`] calls this, once per
/// cycle, so the warning is printed once per cycle; anything else in the
/// cycle uses [`timeout_quiet`].
pub(super) fn timeout() -> Duration {
    let (secs, warning) = probe_limits::timeout_from_env(None);
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    Duration::from_secs(secs)
}

/// [`timeout`] with no warning: for the freshness gate's time limit, set
/// after [`measure_ordered`] has already warned this cycle.
pub(super) fn timeout_quiet() -> Duration {
    Duration::from_secs(probe_limits::timeout_from_env(None).0)
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
            Live::Platform | Live::Excluded => return live.unprobed(),
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
        live.unprobed()
    };
    (live, candidates.iter().cloned().zip(probed).collect())
}

// ---------------------------------------------------------------------------
// Issue #1844: reading in order, with reads that may outlast the cycle
// ---------------------------------------------------------------------------

/// The reads a watching rotator keeps across cycles (issue #1844).
pub type Reads = super::inflight::InFlight<Key, crate::commands::profile::usage_cmd::Probe>;

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
    /// A structurally eligible login whose identity is not yet readable.
    /// Its usage read may refresh expired credentials, but cannot select it.
    UnboundCandidate(ProfileName),
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Live(_) | Self::Unowned => f.write_str("~/.claude"),
            Self::Candidate(n) | Self::UnboundCandidate(n) => f.write_str(n.as_str()),
        }
    }
}

/// How long a cycle waits for its reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Legacy deadline coverage is test-only; production never schedules cycles.
    #[cfg(test)]
    Until(Instant),
    /// Until every read is done: the one-shot command and `--dry-run`.
    Full,
}

/// Two seconds before the next cycle is due, and at least one second after
/// `start`.
#[cfg(test)]
pub fn cycle_deadline(start: Instant, interval: Duration) -> Instant {
    let wait = interval
        .saturating_sub(Duration::from_secs(2))
        .max(Duration::from_secs(1));
    start + wait
}

/// Collect existing reads and launch missing keys in input order.
/// Production awaits completion; legacy deadline behaviour is test-only.
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
    // Carried results cannot claim this command's measurement time.
    let started: Vec<K> = jobs.iter().map(|(k, _)| k.clone()).collect();
    let bound = fl.bound();
    fl.start_all(jobs, bound);
    #[cfg(test)]
    let more = match wait {
        Wait::Until(t) => fl.wait(t),
        Wait::Full => fl.wait_all(),
    };
    #[cfg(not(test))]
    let more = {
        let Wait::Full = wait;
        fl.wait_all()
    };
    got.extend(more);
    for k in fl.take_panicked() {
        eprintln!("warning: rotate: read of {k} panicked; no result");
    }
    keys.iter()
        .map(|k| {
            let probed = match got.remove(k) {
                Some(p) => {
                    let mut probed = reading(&k.to_string(), &p);
                    probed.carried = !started.contains(k);
                    probed
                }
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

/// Explicit-command reads in passive, direct, then Claude order.
/// Watches consume generation-bound evidence instead of interval deadlines.
pub fn measure_ordered(
    ctx: &Ctx,
    live: Live<'_>,
    candidates: &[ProfileName],
    unbound: &[ProfileName],
    now: i64,
) -> (Probed, Vec<(ProfileName, Probed)>) {
    if ctx.event_inputs.is_some() {
        return super::event_measure::measure(ctx, live, candidates);
    }
    let mode = ctx.read_only_mode().unwrap_or_else(mode);
    let timeout = timeout();
    let n = candidates.len() + unbound.len() + usize::from(live.probed());
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
    let reader = Arc::new(reader(
        ctx,
        SourceEnv::from_env(mode, timeout),
        claude_bound,
        now,
    ));
    let mut keys = Vec::with_capacity(n);
    match live {
        Live::Profile(name) => keys.push(Key::Live(name.clone())),
        Live::Unowned => keys.push(Key::Unowned),
        Live::Platform | Live::Excluded => {}
    }
    keys.extend(candidates.iter().cloned().map(Key::Candidate));
    keys.extend(unbound.iter().cloned().map(Key::UnboundCandidate));
    let make = |k: &Key| -> Job<Probe> {
        let reader = Arc::clone(&reader);
        let k = k.clone();
        Box::new(move || match &k {
            Key::Live(name) => reader.profile(name, true),
            Key::Unowned => reader.home(),
            Key::Candidate(name) | Key::UnboundCandidate(name) => reader.profile(name, false),
        })
    };
    let mut read = match &ctx.in_flight {
        Some(fl) => {
            let mut fl = fl.lock().unwrap_or_else(PoisonError::into_inner);
            // Join outstanding refresh reads without trusting their results.
            join_pending(&fl, &mut keys);
            fl.set_bound(bound);
            gather(&mut fl, &keys, make, Wait::Full)
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
        live.unprobed()
    };
    let others = candidates
        .iter()
        .cloned()
        .zip(read.into_iter().map(|(key, p)| {
            if matches!(key, Key::Candidate(_)) {
                p
            } else {
                Probed::failed()
            }
        }))
        .collect();
    (live, others)
}

/// The row reader of one cycle, with `env` and at most `claude_bound`
/// `claude` reads at once. Shared by [`measure_ordered`] and the freshness
/// gate's re-read (`fresh::reread_as_configured`); [`Ctx`] makes it read-only.
pub(super) fn reader(ctx: &Ctx, env: SourceEnv, claude_bound: usize, now: i64) -> RowReader {
    RowReader {
        store: ctx.store.clone(),
        env: ctx.read_env(env),
        binary: LazyBinary::claude(),
        gate: ClaudeGate::new(claude_bound),
        home: Some(ctx.home.clone()),
        no_refresh: ctx.no_refresh_dirs(),
        overflow: crate::commands::profile::overflow::overflow_names(ctx.store.root()),
        // An unknown list (`None`) excludes every profile (issue #2218).
        excluded: usage_cmd::exclude_list(ctx.store.root(), |_| {}).ok(),
        now,
        keep_raw: false,
    }
}
