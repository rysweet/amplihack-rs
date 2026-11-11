//! The order in which one row is read (issue #1844): a fresh passive
//! reading, then the direct read, then the `claude` fallback, as
//! `AMPLIHACK_PROFILE_USAGE_PROBE` and the overflow marks decide. Shared by
//! `profile usage` and the rotator.
//!
//! | Mode / profile | Steps | Fallback mode |
//! |---|---|---|
//! | `auto` / `text` | fallback only | as named |
//! | overflow, not `direct` | fallback only | `text` |
//! | overflow, `direct` | none: error `direct read: overflow needs /usage` | - |
//! | unset | passive, direct, fallback; no fallback when not signed in | `auto`; `text` when credits are on and no window is read |
//! | `direct` | passive, direct; a failure is an error row, or a not-signed-in row | - |
//!
//! Issue #2271: in `unset` and `direct`, a direct read that finds no login
//! to use is final ([`not_signed_in`]): `no oauth token`, `no refresh token`,
//! and on Linux `no credentials file`. The row is `not_logged_in` from the
//! direct read, shown as `not signed in (<reason>)`, and no `claude` is
//! looked up or started; `claude` would find the same empty login. Every
//! other failure, `credentials refused` included, falls back as before.
//!
//! Issue #1856: a direct read with usage credits on and a readable window
//! is a `direct` row with the direct windows, the redacted `extra_usage`
//! shape and spend [`Spend::NotProbed`]. `profile usage --spend` then runs
//! a spend-only `/usage` probe for it under the normal per-probe timeout;
//! the default run and the rotator do not. The spend column:
//!
//! | Row | Spend |
//! |---|---|
//! | passive, direct with credits off, accepted `get_usage` answer, API key | `n/a` |
//! | `/usage` text with a usage reading | `ok`, or `limit_reached` on a spend limit |
//! | `/usage` text with no usage reading, a failed probe, an error row | `unknown` |
//! | direct with credits on, spend probe asked for but no `claude` found | `unknown` |
//! | direct with credits on, no spend probe asked for | `not_probed` |
//!
//! `claude` is looked up only when a fallback is about to run
//! ([`LazyBinary`]), and at most a fixed number run at once ([`ClaudeGate`]).

use super::name::ProfileName;
use super::probe_limits::{self, ProbeMode};
use super::usage_cmd::{ErrorReason, Probe};
#[cfg(unix)]
use super::usage_direct::{self, Endpoints};
use super::usage_parse::UsageState;
use crate::binary_finder::BinaryInfo;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::Duration;

/// The direct read's failure in `direct` mode for a profile marked as
/// overflow: only `/usage` reports its spend limit.
const OVERFLOW_NEEDS_TEXT: &str = "overflow needs /usage";

/// Which step gave a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Source {
    /// A fresh reading from `usage.json`.
    Passive,
    /// The OAuth usage endpoint.
    Direct,
    /// `claude`, by the `get_usage` request or `/usage`.
    Fallback,
}

impl Source {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Passive => "passive",
            Self::Direct => "direct",
            Self::Fallback => "fallback",
        }
    }
}

/// What is known of a row's usage-credit spend limit (issue #1856). It is
/// shown only: it never changes a row's windows, state or order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Spend {
    /// The source has no spend limit to report: credits are off, or the
    /// row is passive or uses an API key.
    NotApplicable,
    /// `/usage` text gave a usage reading and no used-up spend limit.
    Ok,
    /// `/usage` text reported the spend limit as reached.
    LimitReached,
    /// A probe ran, or was asked for and could not start, and gave no
    /// usage reading.
    Unknown,
    /// Credits are on and no spend probe was asked for: `profile usage`
    /// without `--spend`, `AMPLIHACK_PROFILE_USAGE_PROBE=direct`, or the
    /// rotator.
    NotProbed,
}

impl Spend {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "n/a",
            Self::Ok => "ok",
            Self::LimitReached => "limit_reached",
            Self::Unknown => "unknown",
            Self::NotProbed => "not_probed",
        }
    }

    /// The spend that `/usage` text gave. `ok` only for a usage reading:
    /// an error, a timeout, a login prompt or text that did not parse is
    /// `unknown`.
    pub(super) fn of_text(p: &Probe) -> Self {
        if p.error.is_some() {
            return Self::Unknown;
        }
        match p.report.state {
            UsageState::SpendLimit => Self::LimitReached,
            UsageState::Ok | UsageState::SessionExhausted | UsageState::WeeklyExhausted => Self::Ok,
            UsageState::NotLoggedIn
            | UsageState::Timeout
            | UsageState::Unparsed
            | UsageState::Error => Self::Unknown,
        }
    }
}

/// The settings a read uses, resolved once per command or cycle.
#[derive(Debug, Clone)]
pub(super) struct SourceEnv {
    pub(super) mode: ProbeMode,
    pub(super) passive_max_age: u64,
    #[cfg(unix)]
    pub(super) endpoints: Endpoints,
    /// The per-request and per-probe deadline.
    pub(super) timeout: Duration,
}

impl SourceEnv {
    /// The settings from the environment, printing each warning once on
    /// stderr.
    pub(super) fn from_env(mode: ProbeMode, timeout: Duration) -> Self {
        let (passive_max_age, warning) = probe_limits::passive_max_age_from_env();
        if let Some(w) = warning {
            eprintln!("{w}");
        }
        #[cfg(unix)]
        let endpoints = {
            let (endpoints, warnings) = Endpoints::from_env();
            for w in warnings {
                eprintln!("{w}");
            }
            endpoints
        };
        Self {
            mode,
            passive_max_age,
            #[cfg(unix)]
            endpoints,
            timeout,
        }
    }
}

/// How one row is read: the directory whose `.credentials.json` the direct
/// read uses, the profile whose stored reading may stand in (`None` for the
/// `~/.claude` row), whether the direct read may refresh the token, and
/// whether the profile is marked as overflow.
#[derive(Debug, Clone)]
pub(super) struct Plan {
    pub(super) creds_dir: PathBuf,
    pub(super) passive: Option<ProfileName>,
    pub(super) allow_refresh: bool,
    pub(super) overflow: bool,
}

/// The swap journals under `root`: a refresh never runs while one exists.
#[cfg(unix)]
fn journals(root: &Path) -> [PathBuf; 2] {
    let state = root.join(".auto-switch.d");
    [
        state.join("journal.json"),
        state.join("rotate").join("journal.json"),
    ]
}

/// Reads one row in the order the module documents. `fallback(mode)` runs
/// the `claude` probe; it is called at most once, and only when the order
/// needs it.
pub(super) fn read(
    env: &SourceEnv,
    plan: &Plan,
    root: &Path,
    now: i64,
    fallback: impl FnOnce(ProbeMode) -> Probe,
) -> Probe {
    let fall = |mode: ProbeMode, failure: Option<&'static str>| {
        let mut p = fallback(mode);
        p.source = Some(Source::Fallback);
        p.direct_failure = failure;
        p
    };
    match env.mode {
        ProbeMode::Auto | ProbeMode::Text => return fall(env.mode, None),
        ProbeMode::Ordered | ProbeMode::Direct => {}
    }
    let direct_only = env.mode == ProbeMode::Direct;
    if plan.overflow {
        if direct_only {
            return Probe::direct_error(OVERFLOW_NEEDS_TEXT);
        }
        return fall(ProbeMode::Text, None);
    }
    match read_without_claude(env, plan, root, now) {
        Ok(p) => p,
        Err(why) if not_signed_in(why, plan) => Probe::not_signed_in(why),
        Err(why) if direct_only => Probe::direct_error(why),
        Err(why) => {
            let mode = if credits_on(why) {
                ProbeMode::Text
            } else {
                ProbeMode::Auto
            };
            fall(mode, Some(why))
        }
    }
}

/// SR-10: the direct read refreshed the token but could not save it; the
/// refresh token on disk may no longer work.
#[cfg(unix)]
pub(super) fn not_saved(why: &str) -> bool {
    why == usage_direct::reason::REFRESH_NOT_SAVED
}

#[cfg(not(unix))]
pub(super) fn not_saved(_why: &str) -> bool {
    false
}

#[cfg(unix)]
fn credits_on(why: &str) -> bool {
    why == usage_direct::reason::CREDITS_ON
}

#[cfg(not(unix))]
fn credits_on(_why: &str) -> bool {
    false
}

/// Issue #2271: a direct-read failure that `claude` could not do better on,
/// because the login has nothing to use: no token (`no oauth token`), or an
/// expired token and no refresh token (`no refresh token`). On Linux a
/// missing credentials file (`no credentials file`) counts too, but only in
/// a directory that exists as one (not a symlink, and not the stand-in path
/// for a missing `$HOME`); elsewhere a keychain may hold the login, so it
/// still falls back. `credentials refused` never counts: it would hide a
/// foreign-owned, writable or symlinked file. The one check on disk reads the
/// directory's own metadata, never the credentials file or its listing.
#[cfg(unix)]
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
fn not_signed_in(why: &'static str, plan: &Plan) -> bool {
    use usage_direct::reason;
    match why {
        reason::NO_TOKEN | reason::NO_REFRESH_TOKEN => true,
        #[cfg(target_os = "linux")]
        reason::NO_FILE => std::fs::symlink_metadata(&plan.creds_dir).is_ok_and(|m| m.is_dir()),
        _ => false,
    }
}

#[cfg(not(unix))]
fn not_signed_in(_why: &'static str, _plan: &Plan) -> bool {
    false
}

/// The passive step, then the direct read; the direct read's reason when
/// neither gives a row.
#[cfg(unix)]
fn read_without_claude(
    env: &SourceEnv,
    plan: &Plan,
    root: &Path,
    now: i64,
) -> Result<Probe, &'static str> {
    if let Some(name) = &plan.passive
        && let Some(report) = super::usage_passive::fresh(root, name, env.passive_max_age, now)
    {
        return Ok(Probe::read(report, Source::Passive));
    }
    let journals = journals(root);
    let req = usage_direct::Request {
        creds_dir: &plan.creds_dir,
        journals: &journals,
        allow_refresh: plan.allow_refresh,
        cancel: &super::usage_cmd::PROBE_CANCEL,
        timeout: env.timeout,
        now_ms: chrono::Utc::now().timestamp_millis(),
    };
    usage_direct::read(&env.endpoints, &req).map(Probe::direct)
}

/// No direct read off Unix: the credentials checks it needs are Unix-only.
#[cfg(not(unix))]
fn read_without_claude(
    _env: &SourceEnv,
    _plan: &Plan,
    _root: &Path,
    _now: i64,
) -> Result<Probe, &'static str> {
    Err("unsupported platform")
}

// ---------------------------------------------------------------------------
// The lazy `claude` lookup
// ---------------------------------------------------------------------------

type Resolve = dyn Fn() -> Option<BinaryInfo> + Send + Sync;

/// The `claude` binary, looked up on the first [`LazyBinary::get`] and kept
/// for the rest of the command or cycle. Clones share one lookup.
#[derive(Clone)]
pub(super) struct LazyBinary {
    cell: Arc<OnceLock<Option<BinaryInfo>>>,
    resolve: Arc<Resolve>,
}

impl LazyBinary {
    pub(super) fn new(resolve: impl Fn() -> Option<BinaryInfo> + Send + Sync + 'static) -> Self {
        Self {
            cell: Arc::default(),
            resolve: Arc::new(resolve),
        }
    }

    /// The real lookup, `run::resolve_claude_binary`. A failure is `None`
    /// and its cause, a path or an I/O message, is logged once: the lookup
    /// runs at most once per reader. The row text stays `claude not found`.
    pub(super) fn claude() -> Self {
        Self::new(|| match super::run::resolve_claude_binary() {
            Ok(bin) => Some(bin),
            Err(e) => {
                tracing::warn!("profile usage: claude lookup failed: {e:#}");
                None
            }
        })
    }

    pub(super) fn get(&self) -> Option<&BinaryInfo> {
        self.cell.get_or_init(|| (self.resolve)()).as_ref()
    }
}

/// Runs `run` with the binary, or gives the row `claude not found` when
/// there is none. Only that row fails.
pub(super) fn fallback_with(lazy: &LazyBinary, run: impl FnOnce(&BinaryInfo) -> Probe) -> Probe {
    match lazy.get() {
        Some(binary) => run(binary),
        None => {
            let mut p = Probe::failed(ErrorReason::NoClaude, None);
            p.source = Some(Source::Fallback);
            p
        }
    }
}

// ---------------------------------------------------------------------------
// The `claude` bound
// ---------------------------------------------------------------------------

/// At most `limit` `claude` fallbacks at once, however many reads run: the
/// direct reads have their own, larger bound.
#[derive(Clone)]
pub(super) struct ClaudeGate {
    inner: Arc<(Mutex<usize>, Condvar)>,
    limit: usize,
}

impl ClaudeGate {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            inner: Arc::new((Mutex::new(0), Condvar::new())),
            limit: limit.max(1),
        }
    }

    /// Runs `job` once a slot is free. The time spent waiting for the slot
    /// does not count towards the read's age ([`job_clock`]).
    pub(super) fn run<T>(&self, job: impl FnOnce() -> T) -> T {
        let (count, freed) = &*self.inner;
        job_clock::waiting();
        {
            let mut running = count.lock().unwrap_or_else(PoisonError::into_inner);
            while *running >= self.limit {
                running = freed.wait(running).unwrap_or_else(PoisonError::into_inner);
            }
            *running += 1;
        }
        job_clock::restart();
        struct Slot<'a>(&'a (Mutex<usize>, Condvar));
        impl Drop for Slot<'_> {
            fn drop(&mut self) {
                let mut running = self.0.0.lock().unwrap_or_else(PoisonError::into_inner);
                *running -= 1;
                self.0.1.notify_one();
            }
        }
        let _slot = Slot(&self.inner);
        job()
    }
}

// ---------------------------------------------------------------------------
// The age of a read in the rotator's pool
// ---------------------------------------------------------------------------

/// How long a read on this thread has been working, for the rotator's pool
/// of reads that outlast a cycle (issue #1844). The pool gives each job a
/// clock while it runs; [`ClaudeGate::run`] stops it while the read waits
/// for a `claude` slot and starts it again once the slot is taken. Outside
/// the pool no clock is set and these calls do nothing.
pub(super) mod job_clock {
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Instant;

    /// Where a read is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(in crate::commands::profile) enum Phase {
        /// Not yet started: never too old.
        Queued,
        /// Waiting for a `claude` slot: never too old.
        Waiting,
        /// Working since this instant.
        Running(Instant),
    }

    pub(in crate::commands::profile) type Clock = Arc<Mutex<Phase>>;

    thread_local! {
        static CLOCK: RefCell<Option<Clock>> = const { RefCell::new(None) };
    }

    pub(in crate::commands::profile) fn new() -> Clock {
        Arc::new(Mutex::new(Phase::Queued))
    }

    pub(in crate::commands::profile) fn read(clock: &Clock) -> Phase {
        *clock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(phase: Phase) {
        CLOCK.with(|c| {
            if let Some(clock) = c.borrow().as_ref() {
                *clock.lock().unwrap_or_else(PoisonError::into_inner) = phase;
            }
        });
    }

    /// Runs `job` on this thread with `clock` started now.
    pub(in crate::commands::profile) fn run<T>(clock: &Clock, job: impl FnOnce() -> T) -> T {
        *clock.lock().unwrap_or_else(PoisonError::into_inner) = Phase::Running(Instant::now());
        CLOCK.with(|c| *c.borrow_mut() = Some(Arc::clone(clock)));
        struct Unset;
        impl Drop for Unset {
            fn drop(&mut self) {
                CLOCK.with(|c| *c.borrow_mut() = None);
            }
        }
        let _unset = Unset;
        job()
    }

    pub(super) fn waiting() {
        set(Phase::Waiting);
    }

    pub(super) fn restart() {
        set(Phase::Running(Instant::now()));
    }
}
