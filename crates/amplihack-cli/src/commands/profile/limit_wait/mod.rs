//! The usage-limit wait for recipe agent steps (issue #1618).
//!
//! When a supervised `-p` step fails on a usage limit and no auto-switch
//! rerun is left, the step waits once for the reset (or for the login to
//! change), then runs once more with the same inputs. See
//! docs/reference/profile-command.md#usage-limit-wait.
//!
//! `Waiter` decides what happens after a limit; the auto-switch supervisor
//! (`auto_switch::rerun`) and `supervise_unprofiled` below run the attempts.
//! Log lines are built only from static text, integers and parsed instants.

pub(crate) mod fingerprint;
pub(crate) mod plan;
pub(crate) mod scratch;
pub(crate) mod wait;

#[cfg(test)]
mod tests_fingerprint;
#[cfg(test)]
mod tests_plan;
#[cfg(test)]
mod tests_scratch;
#[cfg(test)]
mod tests_wait;

use super::attempt::{self, Limited, Signals};
use super::auto_switch::sweep_rerun;
use crate::commands::recipe::run::limit_patterns::Harness;
use crate::wait_config::WaitSettings;
use anyhow::{Context, Result};
use chrono::TimeZone;
use fingerprint::{Fingerprint, Watch};
use plan::{Bound, plan};
use scratch::{CopyError, StdinCopy};
use std::fmt::Display;
use std::path::PathBuf;
use std::time::Duration;
use wait::{End, WaitDeps};

const PREFIX: &str = "amplihack.recipe.usage_limit_wait";
const STEP_TIMEOUT_ENV: &str = "AMPLIHACK_STEP_TIMEOUT";

/// One log line's content.
pub(crate) enum Event {
    Start {
        harness: Harness,
        reset: Option<i64>,
        until: i64,
        bound: Bound,
    },
    Progress {
        remaining: u64,
    },
    End(End),
    Resume {
        attempt: u32,
    },
    Skipped(Skip),
}

#[derive(Clone, Copy)]
pub(crate) enum Skip {
    Disabled,
    AlreadyWaited,
    NotSupervised,
}

/// The log line for `e`, times in zone `tz`.
pub(crate) fn line<Z: TimeZone>(e: &Event, tz: &Z) -> String
where
    Z::Offset: Display,
{
    let time = |t: i64| match tz.timestamp_opt(t, 0).single() {
        Some(d) => d.format("%Y-%m-%d %H:%M:%S %:z").to_string(),
        None => format!("@{t}"),
    };
    let rest = match e {
        Event::Start {
            harness,
            reset,
            until,
            bound,
        } => format!(
            "start reason=usage_limit harness={} reset={} until={} bound={}",
            harness.as_str(),
            reset.map_or_else(|| "unparsed".to_string(), time),
            time(*until),
            bound.as_str()
        ),
        Event::Progress { remaining } => format!("progress remaining={remaining}s"),
        Event::End(end) => format!("end reason={}", end.as_str()),
        Event::Resume { attempt } => format!("resume attempt={attempt}"),
        Event::Skipped(skip) => format!(
            "skipped reason={}",
            match skip {
                Skip::Disabled => "disabled",
                Skip::AlreadyWaited => "already_waited",
                Skip::NotSupervised => "not_supervised",
            }
        ),
    };
    format!("{PREFIX} {rest}")
}

fn log(e: &Event) {
    eprintln!("{}", line(e, &chrono::Local));
}

/// What the supervisor does after an attempt hit its usage limit.
pub(crate) enum AfterLimit {
    /// Print the attempt's output and exit with its code, or this one.
    Finish(Option<i32>),
    /// Run the step once more under the inherited profile.
    Retry,
}

/// Wall time, or with the debug-only test seams a virtual clock that starts
/// at `TEST_NOW` and moves only when the wait sleeps.
struct Clock {
    base: Option<i64>,
    scale: f64,
    /// Virtual time slept minus real time slept.
    skew: Duration,
}

impl Clock {
    fn new() -> Self {
        let (base, scale) = seams();
        Self {
            base,
            scale,
            skew: Duration::ZERO,
        }
    }

    fn now(&self) -> i64 {
        let skew = i64::try_from(self.skew.as_secs()).unwrap_or(i64::MAX);
        self.base
            .unwrap_or_else(super::fsx::now)
            .saturating_add(skew)
    }

    fn sleep(&mut self, d: Duration) {
        let real = d.div_f64(self.scale).min(Duration::from_secs(1));
        std::thread::sleep(real);
        self.skew += if self.base.is_some() { d } else { d - real };
    }
}

/// `AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW` and `_TEST_SCALE`, read only in
/// debug builds.
#[cfg(debug_assertions)]
fn seams() -> (Option<i64>, f64) {
    let var = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let base = var("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW")
        .filter(|v| v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse().ok());
    let scale = var("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| s.is_finite() && (1.0..=1_000_000.0).contains(s))
        .unwrap_or(1.0);
    (base, scale)
}

#[cfg(not(debug_assertions))]
fn seams() -> (Option<i64>, f64) {
    (None, 1.0)
}

struct RealDeps<'a> {
    clock: &'a mut Clock,
    watch: &'a Watch,
    signals: &'a Signals,
}

impl WaitDeps for RealDeps<'_> {
    type Print = Fingerprint;

    fn now(&mut self) -> i64 {
        self.clock.now()
    }

    fn sleep(&mut self, d: Duration) {
        self.clock.sleep(d);
    }

    fn fingerprint(&mut self) -> Option<Fingerprint> {
        self.watch.probe()
    }

    fn signal(&mut self) -> Option<i32> {
        self.signals.received()
    }

    fn log(&mut self, line: &str) {
        eprintln!("{line}");
    }
}

/// The wait's state for one step: at most one wait per step.
pub(crate) struct Waiter {
    /// `None` when `AMPLIHACK_NONINTERACTIVE` is not exactly `1`.
    settings: Option<WaitSettings>,
    used: bool,
    clock: Clock,
    step_start: i64,
    step_timeout: Option<u64>,
    watch: Watch,
}

impl Waiter {
    pub(crate) fn new(settings: Option<WaitSettings>, watch: Watch) -> Self {
        let clock = Clock::new();
        let step_start = clock.now();
        let step_timeout = std::env::var(STEP_TIMEOUT_ENV)
            .ok()
            .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|v| v.parse().ok());
        Self {
            settings,
            used: false,
            clock,
            step_start,
            step_timeout,
            watch,
        }
    }

    /// Whether this step may wait (checks 4 and 5 of the gate).
    pub(crate) fn enabled(&self) -> bool {
        self.settings.is_some_and(|s| s.enabled)
    }

    pub(crate) fn used(&self) -> bool {
        self.used
    }

    /// The wait's clock, for reading a reset relative to it.
    pub(crate) fn now(&self) -> i64 {
        self.clock.now()
    }

    /// Writes `skipped reason=not_supervised` when the wait applied but the
    /// step could not be supervised.
    pub(crate) fn not_supervised(&self) {
        not_supervised(self.settings);
    }

    /// After attempt `n` hit its usage limit and no auto-switch rerun is
    /// left: pass the result up, or wait once and ask for a retry.
    pub(crate) fn after_limit(&mut self, lim: &Limited, n: u32, signals: &Signals) -> AfterLimit {
        let Some(settings) = self.settings else {
            return AfterLimit::Finish(None);
        };
        if let Some(sig) = signals.received() {
            return AfterLimit::Finish(Some(128 + sig));
        }
        if !settings.enabled {
            log(&Event::Skipped(Skip::Disabled));
            return AfterLimit::Finish(None);
        }
        if self.used {
            log(&Event::Skipped(Skip::AlreadyWaited));
            return AfterLimit::Finish(None);
        }
        self.used = true;
        let now = self.clock.now();
        let step = self.step_timeout.map(|t| (self.step_start, t));
        let p = plan(now, lim.wait_reset, settings.max_wait_secs, step);
        log(&Event::Start {
            harness: lim.harness,
            reset: p.reset,
            until: p.until,
            bound: p.bound,
        });
        let mut deps = RealDeps {
            clock: &mut self.clock,
            watch: &self.watch,
            signals,
        };
        let end = wait::wait(&p, &mut deps);
        log(&Event::End(end));
        match end {
            End::Signal(sig) => AfterLimit::Finish(Some(128 + sig)),
            End::StepTimeout => AfterLimit::Finish(None),
            End::ResetReached | End::MaxReached | End::LoginChanged => {
                log(&Event::Resume {
                    attempt: n.saturating_add(1),
                });
                AfterLimit::Retry
            }
        }
    }
}

/// `skipped reason=not_supervised`, when `settings` enable the wait.
pub(crate) fn not_supervised(settings: Option<WaitSettings>) {
    if settings.is_some_and(|s| s.enabled) {
        log(&Event::Skipped(Skip::NotSupervised));
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Supervises a `-p` step with no inherited profile and the wait enabled:
/// no profile substitution, no ledger, scratch in `$HOME/.amplihack/rerun`.
/// `Ok(None)` when the step could not be supervised and runs once as before.
pub(crate) fn supervise_unprofiled(settings: WaitSettings) -> Result<Option<i32>> {
    let home = env_path("HOME");
    let config_dir = env_path("CLAUDE_CONFIG_DIR");
    // The error can quote the profile root, so only a fixed reason is shown.
    let store = super::store::ProfileStore::from_env()
        .map_err(|_| eprintln!("amplihack: usage-limit wait: profile store unavailable"))
        .ok();
    let watch = Watch::new(store.as_ref(), None, config_dir.as_deref(), home.as_deref());
    let mut waiter = Waiter::new(Some(settings), watch);
    let unsupervised = |waiter: &Waiter, why: &dyn Display| {
        eprintln!(
            "warning: the usage-limit wait cannot supervise this step ({why}); running it once"
        );
        waiter.not_supervised();
        Ok(None)
    };
    let setup = home
        .context("HOME is not set")
        .and_then(|h| scratch::home_rerun_dir(&h).context("no private scratch directory"))
        .and_then(|dir| {
            Ok((
                dir,
                std::env::current_exe().context("cannot find the amplihack binary")?,
            ))
        });
    let (dir, exe) = match setup {
        Ok(s) => s,
        Err(e) => return unsupervised(&waiter, &e),
    };
    sweep_rerun(&dir);
    let signals = match Signals::register() {
        Ok(s) => s,
        Err(e) => return unsupervised(&waiter, &e),
    };
    let stdin = match StdinCopy::take(&dir) {
        Ok(c) => c,
        Err(CopyError::BeforeRead(e)) => return unsupervised(&waiter, &e),
        Err(e @ CopyError::AfterRead(_)) => {
            eprintln!(
                "error: could not copy this step's input for the usage-limit wait ({e}); not running it"
            );
            return Ok(Some(1));
        }
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut n = 0u32;
    loop {
        n += 1;
        let attempt = match attempt::run_attempt(&exe, &args, &dir, None, &stdin, n, &signals) {
            Ok(a) => a,
            // A terminal stdin was not consumed, so a plain launch still works.
            Err(e) if n == 1 && stdin.path().is_none() => return unsupervised(&waiter, &e),
            Err(e) => return Err(e),
        };
        let Some(lim) = attempt::usage_limited(&attempt, waiter.now()) else {
            return attempt.finish(None);
        };
        match waiter.after_limit(&lim, n, &signals) {
            AfterLimit::Finish(code) => return attempt.finish(code),
            AfterLimit::Retry => attempt.discard(),
        }
    }
}
