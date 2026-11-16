//! The usage-limit wait for recipe agent steps (issue #1618).
//!
//! When a supervised `-p` step fails on a usage limit and no auto-switch
//! rerun is left, the step waits for the reset (or for the login to change),
//! then runs once more with the same inputs. A limit that will not reset soon
//! waits for another login with capacity instead when one could take over
//! (issue #1801), and an organisation spend limit on a `$HOME/.claude` step
//! waits for the rotator to move another login in (issue #2731). A step waits
//! once per login it runs on, inside one `max_wait_secs` budget (`marks`).
//! Each wait's `start` and `end` also reach the run's recipe log, so a
//! waiting step is visible there and not just in its own stderr
//! (`recipe_log`). See docs/reference/profile-command.md#usage-limit-wait.
//!
//! `Waiter` decides what happens after a limit; the one supervisor loop,
//! `auto_switch::rerun::run`, runs the attempts with or without auto-switch.
//! Log lines are built only from static text, integers and parsed instants.

pub(crate) mod claude_args;
mod clock;
mod events;
pub(crate) mod fingerprint;
mod ledger;
mod marks;
pub(crate) mod plan;
mod recipe_log;
pub(crate) mod scratch;
pub(crate) mod settings;
pub(crate) mod wait;

#[cfg(test)]
mod tests_changed;
#[cfg(test)]
mod tests_claude_args;
#[cfg(test)]
mod tests_fingerprint;
#[cfg(test)]
mod tests_fingerprint_ledgers;
#[cfg(test)]
mod tests_log;
#[cfg(test)]
mod tests_log_recipe;
#[cfg(test)]
mod tests_login_key;
#[cfg(test)]
mod tests_marks;
#[cfg(test)]
mod tests_plan;
#[cfg(test)]
mod tests_plan_login_change;
#[cfg(test)]
mod tests_plan_reading;
#[cfg(test)]
mod tests_recipe_log;
#[cfg(test)]
mod tests_retry_move;
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
use events::{Event, FailReason, Shape, Skip, log, recipe_line};
use fingerprint::{Fingerprint, Source, Watch};
use marks::Marks;
use plan::{NoWait, PlanKind, WaitFor, plan, plan_capacity, plan_login_change};
pub(crate) use recipe_log::RECIPE_LOG_ENV;
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

/// Why the retry after a wait may run under another login, and from when a
/// capacity reading counts as fresh enough to choose one.
///
/// The two are separate decisions and are deliberately not one field: *may
/// the retry move* is true after any capacity wait, while *was room actually
/// seen* is true only when a reading ended the wait early. A capacity wait
/// that reached its reset may still move, but finding no room is then the
/// expected outcome, not a loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetryMove {
    /// When the wait began, on the wait's clock: read in `after_limit`
    /// just before `wait` takes its own `start`, so it is that instant or a
    /// hair before it, never after. The freshness bound that decides which
    /// readings may move a step onto another account, and not a tuning
    /// parameter: a later instant would make every reading taken during the
    /// wait look stale, so the step would silently retry on the exhausted
    /// profile, and an earlier one would admit readings taken before the
    /// limit was hit. Where the two differ the retry's window is the wider
    /// one, which can only admit a reading the probe already accepted.
    start: i64,
    /// Whether a reading taken during the wait showed another login had
    /// room, which is what ended the wait (#2694). Only then does finding
    /// no room at the retry mean something was lost: the room was taken in
    /// between, and the step gave up a wait that would have run to its
    /// reset and succeeded. A capacity wait that ended any other way never
    /// saw room at all, so finding none is what the step already expected
    /// and costs it one more attempt and nothing else.
    room_was_seen: bool,
}

impl RetryMove {
    /// The floor for a reading to count towards the retry's choice.
    pub(crate) fn start(self) -> i64 {
        self.start
    }

    /// Whether losing the race for the room costs this step a wait that
    /// would have succeeded, rather than merely an attempt.
    pub(crate) fn room_was_seen(self) -> bool {
        self.room_was_seen
    }
}

/// The wait's state for one step: one wait per login, inside one budget.
pub(crate) struct Waiter {
    /// `None` when `AMPLIHACK_NONINTERACTIVE` is not exactly `1`.
    settings: Option<WaitSettings>,
    /// Which logins this step has waited on, and what it has spent (#2731).
    marks: Marks,
    /// The login the attempt now running was launched on, and which file
    /// named it, read before the attempt started. `None` for any step the
    /// rotator cannot move. The source is kept so the reading after the
    /// limit is compared with this one only when both came from the same
    /// file (`changed`).
    attempt_login: Option<(Source, String)>,
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
    /// Whether another login could take over this `$HOME/.claude` step
    /// (#1801), called only when a limit is seen. Always false for a step
    /// with an explicit login or an inherited profile.
    home_takeover: Box<dyn Fn() -> bool>,
    /// Whether a rotator is running that could move another login into
    /// `$HOME/.claude` (#2731), called only when a spend limit is seen.
    home_rotator: Box<dyn Fn() -> bool>,
    /// Set when the retry after this step's wait may run under another
    /// login. Written once, when the wait returns; `None` leaves the retry
    /// on the inherited profile.
    retry_move: Option<RetryMove>,
}

impl Waiter {
    pub(crate) fn new(
        settings: Option<WaitSettings>,
        watch: Watch,
        prompt_in_args: bool,
        home_takeover: Box<dyn Fn() -> bool>,
        home_rotator: Box<dyn Fn() -> bool>,
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
            marks: Marks::new(),
            attempt_login: None,
            clock,
            step_start,
            step_timeout,
            watch,
            // SAFETY: getppid(2) has no preconditions.
            parent: unsafe { libc::getppid() },
            blocked: None,
            prompt_in_args,
            home_takeover,
            home_rotator,
            retry_move: None,
        }
    }

    /// Whether this step may wait (checks 4 and 5 of the gate).
    pub(crate) fn enabled(&self) -> bool {
        self.settings.is_some_and(|s| s.enabled)
    }

    /// Whether this step has waited at all, which is what auto-switch reads
    /// to stop choosing another candidate. #2731 did not change it.
    pub(crate) fn used(&self) -> bool {
        self.marks.used()
    }

    /// Reads the login the next attempt will run on, immediately before it
    /// is launched (issue #2731). On a shared-login host the rotator reacts
    /// to the same exhausted reading that fails the attempt, so the swap
    /// usually lands while the attempt is still failing; taking the key
    /// before the attempt is what lets `after_limit` see that.
    pub(crate) fn note_attempt_login(&mut self) {
        self.attempt_login = self.watch.login();
    }

    /// Whether this step runs on `$HOME/.claude`, the one directory the
    /// rotator swaps logins into.
    pub(crate) fn is_home(&self) -> bool {
        self.watch.is_home()
    }

    /// Whether a rotator is running that could move another login in.
    pub(crate) fn rotator_running(&self) -> bool {
        (self.home_rotator)()
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

    /// Whether another login could take over this `$HOME/.claude` step.
    /// Called when a limit is seen, so steps that never hit one make no
    /// reads.
    pub(crate) fn takeover_home(&self) -> bool {
        (self.home_takeover)()
    }

    /// Whether the retry may run under another login, and what it costs if
    /// none has room; `None` keeps the inherited profile. Cleared as each
    /// wait is planned, so it only ever describes the wait that just ran.
    pub(crate) fn retry_move(&self) -> Option<RetryMove> {
        self.retry_move
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
    /// is the inherited profile's own limit, and it times the wait: other
    /// profiles' limits neither lengthen nor cancel it.
    ///
    /// `probe(start, now)`, both on the wait's clock, ends a wait of any
    /// kind, reset as well as capacity: a reading taken after `start` shows
    /// that another login has room (#2694). The retry then runs under that
    /// login, so a step pinned to a profile may change account part way
    /// through a run; auto-switch off, or the other profiles in the
    /// auto-switch exclude list, holds it on its own login. Whether the step
    /// may move at all is `probe`'s own decision and belongs nowhere else:
    /// it is false for a step with no auto-switch candidate, which is how a
    /// step on `$HOME/.claude` or under an explicit login stays put.
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
        // The mark is the login the failed attempt ran on, read before it
        // was launched. A login this step has already waited on, or a spent
        // budget after an earlier wait, refuses this one (#2731).
        let mark = self.attempt_login.as_ref().map(|(_, n)| n.clone());
        let max = settings.max_wait_secs;
        if self.marks.refuses(&mark, max) {
            log(&Event::Skipped(Skip::AlreadyWaited));
            log(&Event::Failed(FailReason::AlreadyWaited));
            return AfterLimit::Finish(None);
        }
        let now = self.clock.now();
        let step = self.step_timeout.map(|t| (self.step_start, t));
        // `max_wait_secs` is a budget for the whole step, so a later wait
        // gets only what the earlier ones left.
        let budget = self.marks.remaining(max);
        let (harness, p, shape) = match self.blocked.map_or(inherited, WaitFor::Skip) {
            WaitFor::Reset { harness, reset } => {
                (harness, plan(now, reset, budget, step), Shape::Reset)
            }
            WaitFor::Capacity { harness, reset } => (
                harness,
                plan_capacity(now, reset, budget, step),
                Shape::Capacity,
            ),
            WaitFor::LoginChange { harness } => (
                harness,
                plan_login_change(now, budget, step),
                Shape::LoginChange,
            ),
            WaitFor::Skip(why) => {
                log(&Event::Skipped(Skip::Cannot(why)));
                return AfterLimit::Finish(None);
            }
        };
        self.marks.record(mark.clone());
        let account = mark.as_deref();
        let capacity = p.kind == PlanKind::Capacity;
        // Each wait chooses its own retry login and only its own. The
        // clear is here, above the retry-before-wait return below, so a
        // value an earlier wait left behind can never pick the login a
        // later retry runs on: its freshness floor is older and would admit
        // readings taken before this limit was hit (issue #2731). It is
        // written again only by a wait that actually ran.
        self.retry_move = None;
        emit(
            &Event::Start {
                harness,
                reset: p.reset,
                until: p.until,
                bound: p.bound,
                shape,
            },
            account,
        );
        // The login may already have changed while the attempt was failing,
        // which on a shared-login host is the common case: there is then
        // nothing left to wait for. The comparison fails closed, so a ledger
        // the rotator is mid-replace reads as no change and the step waits
        // as it otherwise would.
        if changed(&self.attempt_login, self.watch.login()) {
            emit(&Event::End(End::LoginChanged), account);
            log(&Event::Resume {
                attempt: n.saturating_add(1),
            });
            // Nothing is recorded in the wait ledger: no time passed.
            return AfterLimit::Retry;
        }
        let before = self.clock.now();
        let mut deps = RealDeps {
            clock: &mut self.clock,
            watch: &self.watch,
            signals,
            parent: self.parent,
            probe,
        };
        let waited_from = std::time::Instant::now();
        let end = wait::wait(&p, &mut deps);
        // Real time, not the test clock: auto-drive compares it with its own
        // wall time (issue #1743).
        ledger::record(waited_from.elapsed());
        // The retry may move to another login after a capacity wait however
        // it ended (#1801), and after a wait of any kind that ended because
        // another login has room (#2694).
        let room_was_seen = end == End::OtherCapacity;
        self.retry_move = (capacity || room_was_seen).then_some(RetryMove {
            start: now,
            room_was_seen,
        });
        // The budget is spent on the wait's own clock, the one that planned
        // the deadline.
        self.marks
            .spend(u64::try_from(self.clock.now().saturating_sub(before)).unwrap_or(0));
        emit(&Event::End(end), account);
        match end {
            End::Signal(sig) => AfterLimit::Finish(Some(128 + sig)),
            End::SessionEnded => AfterLimit::Finish(None),
            End::StepTimeout => {
                // Progress lines may have pushed the limit message out of the
                // runner's evidence tail, so this line keeps it classed.
                log(&Event::Failed(FailReason::StepTimeout));
                AfterLimit::Finish(None)
            }
            End::CapacityExpired => {
                log(&Event::Failed(FailReason::CapacityExpired));
                AfterLimit::Finish(None)
            }
            // A login-change wait that reached its deadline: no reset lifts
            // an organisation spend limit, and no login arrived that might.
            End::MaxReached if shape == Shape::LoginChange => {
                log(&Event::Failed(FailReason::SpendLimit));
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

/// Whether the login changed between `was`, read before the attempt, and
/// `now`, read after its limit (issue #2731).
///
/// Fails closed. A change counts only when both readings name a login, both
/// came from the same [`Source`], and the names differ. The cost of the two
/// answers is not symmetric: a missed change costs the step the wait it was
/// going to make anyway, and the rotator's next swap is still seen, while a
/// false change spends the step's one retry on a login that is still at its
/// limit and ends the step. So the three ways the question cannot be
/// answered all read as no change: either reading failed, or they came from
/// different files and are answers to different questions, or the names
/// agree.
fn changed(was: &Option<(Source, String)>, now: Option<(Source, String)>) -> bool {
    matches!(
        (was, &now),
        (Some((sa, a)), Some((sb, b))) if sa == sb && a != b
    )
}

/// Writes `e` to the step's stderr and, for a wait's `start` and `end`, to
/// the run's recipe log (issue #2731). `account` is the login the wait is
/// for; it reaches the recipe log only, never stderr.
fn emit(e: &Event, account: Option<&str>) {
    log(e);
    if let Some(line) = recipe_line(e, account, &chrono::Local) {
        recipe_log::append(&line);
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
    let home_takeover = super::auto_switch::takeover::home_check(home_store.clone());
    // Issue #2731: a login-change wait needs only a running rotator.
    let home_rotator = super::auto_switch::takeover::home_rotator(home_store);
    super::auto_switch::rerun::run(
        None,
        Waiter::new(
            Some(settings),
            watch,
            prompt_in_args,
            home_takeover,
            home_rotator,
        ),
    )
}
