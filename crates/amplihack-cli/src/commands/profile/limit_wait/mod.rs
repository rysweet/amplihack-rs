//! The usage-limit wait for recipe agent steps (issues #1618, #1761).
//!
//! When a supervised `-p` step fails on a usage limit and no auto-switch
//! rerun is left, the step waits for the reset (or for the login to
//! change), then runs once more with the same inputs. Without a login
//! rotator a step waits once; while one runs for `$HOME/.claude` it waits
//! again at each limit, up to `recipe.usage_limit_max_waits_per_step`
//! waits. All waits of one recipe run share
//! `recipe.usage_limit_wait_budget_secs` (`budget`). See
//! docs/reference/profile-command.md#usage-limit-wait.
//!
//! `Waiter` decides what happens after a limit (`after_limit`); the one
//! supervisor loop, `auto_switch::rerun::run`, runs the attempts with or
//! without auto-switch. Log lines are built only from static text,
//! integers, parsed instants and checked profile names.

mod after_limit;
mod budget;
pub(crate) mod claude_args;
mod clock;
mod events;
pub(crate) mod fingerprint;
pub(crate) mod plan;
pub(crate) mod scratch;
pub(crate) mod settings;
pub(crate) mod wait;

#[cfg(test)]
mod tests_after_limit;
#[cfg(test)]
mod tests_budget;
#[cfg(test)]
mod tests_budget_refused;
#[cfg(test)]
mod tests_budget_sweep;
#[cfg(test)]
mod tests_claude_args;
#[cfg(test)]
mod tests_fingerprint;
#[cfg(test)]
mod tests_fingerprint_ledgers;
#[cfg(test)]
mod tests_fingerprint_rotator;
#[cfg(test)]
mod tests_log;
#[cfg(test)]
mod tests_notes;
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

use anyhow::Result;
use budget::Budget;
use clock::Clock;
use events::{Event, Skip, log};
use fingerprint::Watch;
use plan::{NoWait, WaitFor};
pub(crate) use settings::WaitSettings;
use std::path::PathBuf;

const STEP_TIMEOUT_ENV: &str = "AMPLIHACK_STEP_TIMEOUT";

/// What the supervisor does after an attempt hit its usage limit.
pub(crate) enum AfterLimit {
    /// Print the attempt's output and exit with its code, or this one.
    Finish(Option<i32>),
    /// Run the step once more under the inherited profile.
    Retry,
}

/// The wait's state for one step.
pub(crate) struct Waiter {
    /// `None` when `AMPLIHACK_NONINTERACTIVE` is not exactly `1`.
    settings: Option<WaitSettings>,
    /// Waits started in this step, whatever ended them.
    waits: u32,
    /// The recipe run's wait budget, as this step sees it.
    budget: Budget,
    /// The step id on log lines: this supervisor's process id.
    step: u32,
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
}

impl Waiter {
    pub(crate) fn new(settings: Option<WaitSettings>, watch: Watch, prompt_in_args: bool) -> Self {
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
            waits: 0,
            budget: Budget::from_env(),
            step: std::process::id(),
            clock,
            step_start,
            step_timeout,
            watch,
            // SAFETY: getppid(2) has no preconditions.
            parent: unsafe { libc::getppid() },
            blocked: None,
            prompt_in_args,
        }
    }

    /// Whether this step may wait (checks 4 and 5 of the gate).
    pub(crate) fn enabled(&self) -> bool {
        self.settings.is_some_and(|s| s.enabled)
    }

    /// This step has waited at least once.
    pub(crate) fn used(&self) -> bool {
        self.waits > 0
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

    /// The wait's clock, for reading a reset relative to it.
    pub(crate) fn now(&self) -> i64 {
        self.clock.now()
    }

    /// Writes `skipped reason=not_supervised` when the wait applied but the
    /// step could not be supervised.
    pub(crate) fn not_supervised(&self) {
        not_supervised(self.settings);
    }
}

/// `skipped reason=not_supervised`, when `settings` enable the wait.
pub(crate) fn not_supervised(settings: Option<WaitSettings>) {
    if settings.is_some_and(|s| s.enabled) {
        log(&Event::Skipped {
            skip: Skip::NotSupervised,
            step: std::process::id(),
        });
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
    prompt_in_args: bool,
) -> Result<Option<i32>> {
    let path = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let (home, config_dir) = (path("HOME"), path("CLAUDE_CONFIG_DIR"));
    let watch = Watch::new(store, default, config_dir.as_deref(), home.as_deref());
    super::auto_switch::rerun::run(None, Waiter::new(Some(settings), watch, prompt_in_args))
}
