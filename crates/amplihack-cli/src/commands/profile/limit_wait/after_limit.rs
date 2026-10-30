//! What happens after an attempt hit its usage limit (issues #1618, #1761).
//!
//! Without settings the result is passed up. Otherwise [`check`] decides,
//! in order: a stop signal, the wait disabled (or off in CI), the rotator
//! check (made once, and only for a step still in the running), the
//! per-step rule, and a limit no wait can outlast. Without a rotator a step
//! waits once (`already_waited`); with one it waits up to
//! `max_waits_per_step` times (`step_waits_spent`).
//! Then [`Waiter::after_limit`] waits and asks for a retry. With a rotator
//! it first reads the run budget and waits for at most what is left of it;
//! without one the run budget neither refuses nor shortens the single wait.
//! Either way the wait's seconds are added to the run's budget record.
//! The budget's lock is never held across a wait.

use super::super::attempt::Signals;
use super::clock::Clock;
use super::events::{Event, Skip, budget_spent_note, log, step_cap_note};
use super::fingerprint::{Fingerprint, Watch};
use super::plan::{WaitFor, plan};
use super::settings::WaitSettings;
use super::wait::{self, End, WaitDeps};
use super::{AfterLimit, Waiter};
use crate::commands::recipe::run::limit_patterns::Harness;
use std::time::Duration;

/// What [`check`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Check {
    /// Wait for `reset`, if the run budget allows; `rotator` as seen.
    Wait {
        harness: Harness,
        reset: i64,
        rotator: bool,
    },
    /// Pass the result up with `code`, after a `skipped` line for `skip`.
    Finish {
        code: Option<i32>,
        skip: Option<Skip>,
    },
}

/// The checks before the budget, for a step that has waited `waits` times.
/// `rotator` is asked at most once, and only once the wait is enabled.
pub(super) fn check(
    settings: WaitSettings,
    signal: Option<i32>,
    waits: u32,
    wait: WaitFor,
    rotator: impl FnOnce() -> bool,
) -> Check {
    let finish = |code, skip| Check::Finish { code, skip };
    if let Some(sig) = signal {
        return finish(Some(128i32.saturating_add(sig)), None);
    }
    if !settings.enabled {
        let skip = if settings.off_in_ci {
            Skip::Ci
        } else {
            Skip::Disabled
        };
        return finish(None, Some(skip));
    }
    let rotator = rotator();
    if !rotator && waits >= 1 {
        return finish(None, Some(Skip::AlreadyWaited));
    }
    if rotator && waits >= settings.max_waits_per_step {
        return finish(None, Some(Skip::StepWaitsSpent));
    }
    match wait {
        WaitFor::Reset { harness, reset } => Check::Wait {
            harness,
            reset,
            rotator,
        },
        // A reset too far away or unreadable ends the step even with a
        // rotator running: #1761 kept the #1618 five-hour rule. Waiting for
        // rotator capacity here is issue #1796. This arm is reached only
        // after the rotator check and the per-step rule above, so a step
        // that has used its waits logs that reason instead.
        WaitFor::Skip(why) => finish(None, Some(Skip::Cannot(why))),
    }
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

/// Writes a budget warning on stderr.
fn warn(line: &str) {
    eprintln!("{line}");
}

impl Waiter {
    /// After attempt `n` hit its usage limit and no auto-switch rerun is
    /// left: pass the result up, or wait and ask for a retry. `inherited`
    /// is the inherited profile's own limit: the retry runs under that
    /// profile, so other profiles' limits do not time or cancel the wait.
    pub(crate) fn after_limit(
        &mut self,
        inherited: WaitFor,
        n: u32,
        signals: &Signals,
    ) -> AfterLimit {
        let Some(settings) = self.settings else {
            return AfterLimit::Finish(None);
        };
        let step = self.step;
        let wait = self.blocked.map_or(inherited, WaitFor::Skip);
        let watch = &self.watch;
        let decided = check(settings, signals.received(), self.waits, wait, || {
            watch.rotator_running()
        });
        let (harness, reset, rotator) = match decided {
            Check::Wait {
                harness,
                reset,
                rotator,
            } => (harness, reset, rotator),
            Check::Finish { code, skip } => {
                if let Some(skip) = skip {
                    log(&Event::Skipped { skip, step });
                    if skip == Skip::StepWaitsSpent {
                        eprintln!("{}", step_cap_note(settings.max_waits_per_step));
                    }
                }
                return AfterLimit::Finish(code);
            }
        };
        // Only a wait with a rotator draws on the run budget. Without one
        // the step keeps the #1618 single wait to the reset, whatever other
        // steps of the run have waited (#1761: "keep today's behaviour").
        // Its seconds are still added below, so a later wait with a
        // rotator sees time the run already spent.
        let left = if rotator {
            let left = self.budget.left(settings.budget_secs, &mut warn);
            if left == 0 {
                log(&Event::Skipped {
                    skip: Skip::BudgetSpent,
                    step,
                });
                let per_step = self.budget.per_step();
                eprintln!("{}", budget_spent_note(settings.budget_secs, per_step));
                return AfterLimit::Finish(None);
            }
            Some(left)
        } else {
            None
        };
        let per_step = self.budget.per_step();
        // Every wait started counts, whatever ends it, so the cap holds.
        self.waits = self.waits.saturating_add(1);
        let now = self.clock.now();
        let step_bound = self.step_timeout.map(|t| (self.step_start, t));
        let p = plan(now, reset, settings.max_wait_secs, step_bound, left);
        log(&Event::Start {
            harness,
            reset: p.reset,
            until: p.until,
            bound: p.bound,
            step,
            attempt: n,
            rotator,
        });
        let mut deps = RealDeps {
            clock: &mut self.clock,
            watch: &self.watch,
            signals,
            parent: self.parent,
        };
        let end = wait::wait(&p, &mut deps);
        let waited = u64::try_from(self.clock.now().saturating_sub(now)).unwrap_or(0);
        self.budget.add(waited, &mut warn);
        log(&Event::End { end, waited });
        match end {
            End::Signal(sig) => AfterLimit::Finish(Some(128i32.saturating_add(sig))),
            End::BudgetSpent => {
                eprintln!("{}", budget_spent_note(settings.budget_secs, per_step));
                AfterLimit::Finish(None)
            }
            End::StepTimeout | End::SessionEnded => AfterLimit::Finish(None),
            End::ResetReached | End::MaxReached | End::LoginChanged | End::RotatorCapacity => {
                log(&Event::Resume {
                    attempt: n.saturating_add(1),
                    step,
                    waited,
                    login: self.watch.login(),
                });
                AfterLimit::Retry
            }
        }
    }
}
