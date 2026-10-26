//! The usage-limit wait for recipe agent steps (issue #1618).
//!
//! When a supervised `-p` step fails on a usage limit and no auto-switch
//! rerun is left, the step waits once for the reset (or for the login to
//! change), then runs once more with the same inputs. See
//! docs/reference/profile-command.md#usage-limit-wait.
//!
//! `Waiter` decides what happens after a limit; the one supervisor loop,
//! `auto_switch::rerun::run`, runs the attempts with or without auto-switch.
//! Log lines are built only from static text, integers and parsed instants.

mod clock;
pub(crate) mod fingerprint;
pub(crate) mod plan;
pub(crate) mod scratch;
pub(crate) mod settings;
pub(crate) mod wait;

#[cfg(test)]
mod tests_fingerprint;
#[cfg(test)]
mod tests_log;
#[cfg(test)]
mod tests_plan;
#[cfg(test)]
mod tests_scratch;
#[cfg(test)]
mod tests_settings;
#[cfg(test)]
mod tests_wait;

use super::attempt::{Limited, Signals};
use crate::commands::recipe::run::limit_patterns::Harness;
use anyhow::Result;
use chrono::TimeZone;
use clock::Clock;
use fingerprint::{Fingerprint, Watch};
use plan::{Bound, NoWait, WaitFor, plan};
pub(crate) use settings::WaitSettings;
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
        reset: i64,
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
    /// Off because this is CI and nothing turned it on.
    Ci,
    /// The limit is not one a wait can outlast.
    Cannot(NoWait),
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
            time(*reset),
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
                Skip::Ci => "ci",
                Skip::Cannot(why) => why.as_str(),
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

struct RealDeps<'a> {
    clock: &'a mut Clock,
    watch: &'a Watch,
    signals: &'a Signals,
    parent: libc::pid_t,
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

    fn session_ended(&mut self) -> bool {
        // SAFETY: getppid(2) has no preconditions. A parent that exits
        // re-parents this process, so the id changes.
        unsafe { libc::getppid() != self.parent }
    }

    fn rotator_capacity(&mut self, start: i64) -> bool {
        self.watch.rotator_capacity(start)
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
    /// The process that started this step, read before any attempt runs.
    parent: libc::pid_t,
    /// Why no wait can help this step, whatever the limit says.
    blocked: Option<NoWait>,
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
            // SAFETY: getppid(2) has no preconditions.
            parent: unsafe { libc::getppid() },
            blocked: None,
        }
    }

    /// Whether this step may wait (checks 4 and 5 of the gate).
    pub(crate) fn enabled(&self) -> bool {
        self.settings.is_some_and(|s| s.enabled)
    }

    pub(crate) fn used(&self) -> bool {
        self.used
    }

    /// Never wait for this step, for `why`.
    pub(crate) fn block(&mut self, why: NoWait) {
        self.blocked = Some(why);
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
            log(&Event::Skipped(if settings.off_in_ci {
                Skip::Ci
            } else {
                Skip::Disabled
            }));
            return AfterLimit::Finish(None);
        }
        if self.used {
            log(&Event::Skipped(Skip::AlreadyWaited));
            return AfterLimit::Finish(None);
        }
        let wait = self.blocked.map_or(lim.wait, WaitFor::Skip);
        let (harness, reset) = match wait {
            WaitFor::Reset { harness, reset } => (harness, reset),
            WaitFor::Skip(why) => {
                log(&Event::Skipped(Skip::Cannot(why)));
                return AfterLimit::Finish(None);
            }
        };
        self.used = true;
        let now = self.clock.now();
        let step = self.step_timeout.map(|t| (self.step_start, t));
        let p = plan(now, reset, settings.max_wait_secs, step);
        log(&Event::Start {
            harness,
            reset: p.reset,
            until: p.until,
            bound: p.bound,
        });
        let mut deps = RealDeps {
            clock: &mut self.clock,
            watch: &self.watch,
            signals,
            parent: self.parent,
        };
        let end = wait::wait(&p, &mut deps);
        log(&Event::End(end));
        match end {
            End::Signal(sig) => AfterLimit::Finish(Some(128 + sig)),
            End::StepTimeout | End::SessionEnded => AfterLimit::Finish(None),
            End::ResetReached | End::MaxReached | End::LoginChanged | End::RotatorCapacity => {
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

/// Supervises a `-p` step with no inherited profile and the wait enabled:
/// no profile substitution, no ledger, scratch in `$HOME/.amplihack/rerun`.
/// `default` is the profile the launch will apply from the `.default`
/// pointer, if any; the wait then watches that profile's login.
/// `Ok(None)` when the step could not be supervised and runs once as before.
pub(crate) fn supervise_unprofiled(
    settings: WaitSettings,
    store: Option<&super::store::ProfileStore>,
    default: Option<&super::name::ProfileName>,
) -> Result<Option<i32>> {
    let path = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let (home, config_dir) = (path("HOME"), path("CLAUDE_CONFIG_DIR"));
    let watch = Watch::new(store, default, config_dir.as_deref(), home.as_deref());
    super::auto_switch::rerun::run(None, Waiter::new(Some(settings), watch))
}
