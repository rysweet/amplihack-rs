//! The usage-limit wait for recipe agent steps (issue #1618).
//!
//! When a supervised `-p` step fails on a usage limit and no auto-switch
//! rerun is left, the step waits once for the reset (or for the login to
//! change), then runs once more with the same inputs. A limit that will not
//! reset soon waits for another login with capacity instead when one could
//! take over (issue #1801). See
//! docs/reference/profile-command.md#usage-limit-wait.
//!
//! `Waiter` decides what happens after a limit; the one supervisor loop,
//! `auto_switch::rerun::run`, runs the attempts with or without auto-switch.
//! Log lines are built only from static text, integers and parsed instants.

pub(crate) mod claude_args;
mod clock;
mod events;
pub(crate) mod fingerprint;
pub(crate) mod plan;
pub(crate) mod scratch;
pub(crate) mod settings;
pub(crate) mod wait;

#[cfg(test)]
mod tests_claude_args;
#[cfg(test)]
mod tests_fingerprint;
#[cfg(test)]
mod tests_fingerprint_ledgers;
#[cfg(test)]
mod tests_log;
#[cfg(test)]
mod tests_plan;
#[cfg(test)]
mod tests_plan_reading;
#[cfg(test)]
mod tests_scratch;
#[cfg(test)]
mod tests_settings;
#[cfg(test)]
mod tests_wait;
#[cfg(test)]
mod tests_wait_ends;

use super::attempt::Signals;
use super::store::ProfileStore;
use anyhow::Result;
use clock::Clock;
use events::{Event, FailReason, Skip, log};
use fingerprint::{Fingerprint, Watch};
use plan::{NoWait, PlanKind, WaitFor, plan, plan_capacity};
pub(crate) use settings::WaitSettings;
use std::path::PathBuf;
use std::time::Duration;
use wait::{End, WaitDeps};

const STEP_TIMEOUT_ENV: &str = "AMPLIHACK_STEP_TIMEOUT";

/// What the supervisor does after an attempt hit its usage limit.
pub(crate) enum AfterLimit {
    /// Print the attempt's output and exit with its code, or this one.
    Finish(Option<i32>),
    /// Run the step once more: under the inherited profile after a reset
    /// wait; the supervisor chooses the login after a capacity wait.
    Retry,
}

struct RealDeps<'a> {
    clock: &'a mut Clock,
    watch: &'a Watch,
    signals: &'a Signals,
    parent: libc::pid_t,
    probe: &'a dyn Fn(i64, i64) -> bool,
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

    fn other_capacity(&mut self, start: i64) -> bool {
        (self.probe)(start, self.clock.now())
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
    /// Claude's arguments carry the prompt (else it may be on stdin).
    prompt_in_args: bool,
    /// The store of a `$HOME/.claude` step, whose login another stored
    /// profile could take over (#1801). Read only when a limit is seen.
    home_store: Option<ProfileStore>,
    /// When this step's wait began, on the wait's clock, if a capacity wait.
    capacity_start: Option<i64>,
}

impl Waiter {
    pub(crate) fn new(
        settings: Option<WaitSettings>,
        watch: Watch,
        prompt_in_args: bool,
        home_store: Option<ProfileStore>,
    ) -> Self {
        let clock = Clock::new();
        let step_start = clock.now();
        // Trimmed, as the config loader reads it: a stray space must not
        // drop the bound and let the wait outlive the step.
        let step_timeout = std::env::var(STEP_TIMEOUT_ENV)
            .ok()
            .map(|v| v.trim().to_string())
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
            prompt_in_args,
            home_store,
            capacity_start: None,
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

    /// `WaitFor::Skip` for the reason set by `block`, if any. Passed to
    /// `after_limit` when no limit of the inherited profile is known, so the
    /// refusal is still logged.
    pub(crate) fn blocked_skip(&self) -> Option<WaitFor> {
        self.blocked.map(WaitFor::Skip)
    }

    /// Whether Claude's arguments carry the prompt.
    pub(crate) fn prompt_in_args(&self) -> bool {
        self.prompt_in_args
    }

    /// Whether another login could take over this `$HOME/.claude` step from
    /// the profile live there. Called when a limit is seen, so steps that
    /// never hit one make no reads.
    pub(crate) fn takeover_home(&self) -> bool {
        use super::auto_switch::takeover::{could_take_over, live_profile};
        let store = self.home_store.as_ref();
        could_take_over(store, live_profile(store).as_deref())
    }

    /// When this step's capacity wait began, on the wait's clock.
    pub(crate) fn capacity_start(&self) -> Option<i64> {
        self.capacity_start
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
    /// left: pass the result up, or wait once and ask for a retry. `inherited`
    /// is the inherited profile's own limit: the retry runs under that
    /// profile, so other profiles' limits do not time or cancel the wait.
    /// `probe(start, now)`, both on the wait's clock, ends a capacity wait:
    /// a reading taken after `start` shows another login has capacity.
    pub(crate) fn after_limit(
        &mut self,
        inherited: WaitFor,
        n: u32,
        signals: &Signals,
        probe: &dyn Fn(i64, i64) -> bool,
    ) -> AfterLimit {
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
            log(&Event::Failed(FailReason::AlreadyWaited));
            return AfterLimit::Finish(None);
        }
        let now = self.clock.now();
        let step = self.step_timeout.map(|t| (self.step_start, t));
        let max = settings.max_wait_secs;
        let (harness, p) = match self.blocked.map_or(inherited, WaitFor::Skip) {
            WaitFor::Reset { harness, reset } => (harness, plan(now, reset, max, step)),
            WaitFor::Capacity { harness, reset } => (harness, plan_capacity(now, reset, max, step)),
            WaitFor::Skip(why) => {
                log(&Event::Skipped(Skip::Cannot(why)));
                return AfterLimit::Finish(None);
            }
        };
        self.used = true;
        let capacity = p.kind == PlanKind::Capacity;
        self.capacity_start = capacity.then_some(now);
        log(&Event::Start {
            harness,
            reset: p.reset,
            until: p.until,
            bound: p.bound,
            capacity,
        });
        let mut deps = RealDeps {
            clock: &mut self.clock,
            watch: &self.watch,
            signals,
            parent: self.parent,
            probe,
        };
        let end = wait::wait(&p, &mut deps);
        log(&Event::End(end));
        match end {
            End::Signal(sig) => AfterLimit::Finish(Some(128 + sig)),
            End::SessionEnded => AfterLimit::Finish(None),
            End::StepTimeout => {
                if capacity {
                    log(&Event::Failed(FailReason::StepTimeout));
                }
                AfterLimit::Finish(None)
            }
            End::CapacityExpired => {
                log(&Event::Failed(FailReason::CapacityExpired));
                AfterLimit::Finish(None)
            }
            End::ResetReached
            | End::MaxReached
            | End::LoginChanged
            | End::RotatorCapacity
            | End::OtherCapacity => {
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
    store: Option<&ProfileStore>,
    default: Option<&super::name::ProfileName>,
    prompt_in_args: bool,
) -> Result<Option<i32>> {
    let path = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let (home, config_dir) = (path("HOME"), path("CLAUDE_CONFIG_DIR"));
    let watch = Watch::new(store, default, config_dir.as_deref(), home.as_deref());
    // Only `$HOME/.claude` can get another login: an explicit directory or a
    // `.default` profile is the login the launch chose (#1801).
    let home_store = store.filter(|_| watch.is_home()).cloned();
    super::auto_switch::rerun::run(
        None,
        Waiter::new(Some(settings), watch, prompt_in_args, home_store),
    )
}
