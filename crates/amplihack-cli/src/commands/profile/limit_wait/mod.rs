//! The usage-limit wait for recipe agent steps (issue #1618).
//!
//! When a supervised `-p` step fails on a usage limit and no auto-switch
//! rerun is left, the step waits for the reset (or for the login to change),
//! then runs once more with the same inputs. A limit that will not reset soon
//! waits for another login with capacity instead (issue #1801), and a spend
//! limit that states no reset waits for the rotator to move another login in
//! (issue #2731).
//!
//! Issue #2911: a limit never ends a step. Every limit this module can read
//! becomes a wait, a step may wait as many times as it meets one, and the
//! chain is paced by the waits costing time rather than by a count or a
//! budget. A wait with no reset it can end on says so plainly, as
//! `waiting_for_capacity`, naming the login and why. What still ends a step
//! is its own timeout, a stop signal, the end of its session, output with no
//! limit message in it, and a piped prompt that cannot be replayed.
//!
//! Each wait's `start`, `waiting_for_capacity` and `end` also reach the run's
//! recipe log, so a waiting step is visible there and not just in its own
//! stderr (`recipe_log`). See docs/reference/usage-limit-waits.md.
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
mod tests_passed_reset;
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
mod tests_room_gate;
#[cfg(test)]
mod tests_scratch;
#[cfg(test)]
mod tests_settings;
#[cfg(test)]
mod tests_spend_reset;
#[cfg(test)]
mod tests_wait;
#[cfg(test)]
mod tests_wait_ends;
#[cfg(test)]
mod tests_wait_room;

use super::attempt::Signals;
use super::name::ProfileName;
use super::store::ProfileStore;
use amplihack_utils::rotator_status::Ledger;
use anyhow::Result;
use clock::Clock;
pub(crate) use events::Met;
use events::{Event, FailReason, Shape, Skip, log, recipe_line};
use fingerprint::{Fingerprint, Source, Watch};
use marks::Marks;
use plan::{Bound, NoWait, PlanKind, WaitFor, plan, plan_capacity, plan_login_change};
pub(crate) use recipe_log::RECIPE_LOG_ENV;
pub(crate) use settings::WaitSettings;
use std::path::PathBuf;
use std::time::Duration;
use wait::{End, WaitDeps};

const STEP_TIMEOUT_ENV: &str = "AMPLIHACK_STEP_TIMEOUT";

/// How the attempt whose limit is being judged came to run (issue #2857).
///
/// There is deliberately no `Default`: the open case must be written out by
/// the caller that knows it, never fallen into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resumed {
    /// The step's first attempt, or an auto-switch rerun: nothing had
    /// claimed this login was usable.
    Ordinarily,
    /// The retry a wait that ended on a positive signal earned
    /// ([`wait::End::said_try_now`]), met by `.0`. The signal was wrong, so
    /// the same-login refusal does not apply to the wait that follows.
    AfterASignal(Met),
}

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
    /// The login key read before the attempt was launched, the anchor for
    /// [`WaitDeps::login_changed`] (issue #2731).
    attempt_login: &'a Option<(Source, String)>,
    /// The wait being made, for the room gate (issue #2857): a login-change
    /// wait is exempt, and [`change_may_end`] reads that from the plan.
    plan: &'a plan::Plan,
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

    fn login_changed(&mut self) -> bool {
        // The same comparison the pre-wait check makes, on the same
        // anchor: see `changed`.
        changed(self.attempt_login, self.watch.login())
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

    fn change_may_end(&mut self) -> bool {
        change_may_end(self.plan, self.watch)
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
    /// Which logins this step has waited on (#2731, #2911).
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
    /// What this step's Claude arguments ask for, which is how the
    /// supervisor knows whether stdin carries anything it must keep and
    /// whether that thing is a prompt at all (issues #2815, #2816).
    args: claude_args::ClaudeArgs,
    /// Whether another login could take over this `$HOME/.claude` step
    /// (#1801), called only when a limit is seen. Always false for a step
    /// with an explicit login or an inherited profile.
    home_takeover: Box<dyn Fn() -> bool>,
    /// Set when the retry after this step's wait may run under another
    /// login. Written once, when the wait returns; `None` leaves the retry
    /// on the inherited profile.
    retry_move: Option<RetryMove>,
    /// Whether the retry [`Waiter::after_limit`] last asked for was earned
    /// by a wait that ended on a positive signal (issue #2857). Read and
    /// consumed by the supervisor for the attempt that retry launches.
    said_try_now: bool,
}

impl Waiter {
    pub(crate) fn new(
        settings: Option<WaitSettings>,
        watch: Watch,
        args: claude_args::ClaudeArgs,
        home_takeover: Box<dyn Fn() -> bool>,
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
            args,
            home_takeover,
            retry_move: None,
            said_try_now: false,
        }
    }

    /// Whether the attempt about to be judged is the retry a wait that said
    /// *try now* earned (issue #2857), and clears the answer.
    ///
    /// Set at the two returns of [`Waiter::after_limit`] that ask for a
    /// retry, and only when the wait ended on a positive signal; cleared on
    /// every other return. Consumed on read, so exactly one attempt can be
    /// judged as that retry: an auto-switch rerun under another profile, or
    /// a later step, inherits nothing.
    pub(crate) fn resumed_on_a_signal(&mut self) -> bool {
        std::mem::take(&mut self.said_try_now)
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

    /// The profile root whose rotate ledger this step's login is recorded
    /// in (issue #2890); `None` when the step has no store to record in.
    pub(crate) fn store_root(&self) -> Option<&std::path::Path> {
        self.watch.store_root()
    }

    /// Which login the attempt now running ran under (issue #2890), as a
    /// profile name: `profile` when the supervisor launched it under one,
    /// else the inherited profile this step's watch names, else the login
    /// key read before the attempt started ([`Waiter::note_attempt_login`]),
    /// which for a `$HOME/.claude` step is the rotator's live profile.
    ///
    /// Every name goes through [`ProfileName::parse`], so a value from a
    /// file can never become a path segment; `None` when none parses, and
    /// then nothing is recorded.
    pub(crate) fn attempt_profile(&self, profile: Option<&ProfileName>) -> Option<ProfileName> {
        if let Some(p) = profile {
            return Some(p.clone());
        }
        if let Some(p) = self.watch.inherited_profile() {
            return Some(p);
        }
        self.attempt_login
            .as_ref()
            .and_then(|(_, name)| ProfileName::parse(name).ok())
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

    /// Whether Claude's arguments carry the prompt, so stdin holds nothing
    /// this step needs.
    pub(crate) fn prompt_in_args(&self) -> bool {
        self.args.prompt
    }

    /// Whether stdin is a control channel rather than a prompt that ends:
    /// `--input-format` with anything but `text` (issue #2816).
    ///
    /// Such a channel is read for as long as the session and its caller
    /// holds it open on purpose, so there is no end of it to copy and no
    /// replaying it to a retry. A step reading one keeps the conduct it had
    /// before #2815: nothing is copied, and a usage limit ends it with
    /// `skipped reason=piped_prompt` rather than waiting for a reset it
    /// could not use.
    pub(crate) fn stdin_is_a_channel(&self) -> bool {
        self.args.stream_stdin
    }

    /// Whether another login could take over this `$HOME/.claude` step.
    /// Called when a limit is seen, so steps that never hit one make no
    /// reads.
    pub(crate) fn takeover_home(&self) -> bool {
        (self.home_takeover)()
    }

    /// The mark for the login the attempt now running was launched on: what
    /// [`Marks`] records for a wait, and the account the wait's lines name.
    fn mark(&self) -> Option<String> {
        self.attempt_login.as_ref().map(|(_, n)| n.clone())
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
    ///
    /// `replay()` is asked, at the moment a retry is about to be announced,
    /// why the step's prompt cannot be given to one; `None` means it can
    /// (issue #2816). It is asked here, and not before the wait, because a
    /// prompt still arriving on a pipe when the limit is seen has normally
    /// arrived by the time a wait of hours ends: a refusal before the wait
    /// would throw away the rerun that waiting earns. A refusal here ends
    /// the step with the limit result and `skipped reason=piped_prompt`,
    /// rather than announcing a retry that cannot run.
    ///
    /// `resumed` says how the attempt that hit this limit came to run
    /// (issue #2857). [`Resumed::AfterASignal`] means a wait had just ended
    /// claiming the login was usable and this limit contradicts it; the step
    /// says so once, on a `reentered` line before the wait's `start`, so a
    /// reader can tell a second wait that follows a contradicted signal from
    /// one that follows a new login.
    ///
    /// Issue #2911: nothing here refuses a wait for a limit. A second and a
    /// third limit in one step each wait, and the chain is paced by the
    /// waits costing time rather than by a count or a budget: every plan a
    /// limit produces ends strictly after `now` unless the step timeout
    /// brings it back (see `plan`). What still ends a step is the step's own
    /// timeout, a stop signal, the end of its session, output with no limit
    /// message in it, and a piped prompt that cannot be replayed.
    pub(crate) fn after_limit(
        &mut self,
        inherited: WaitFor,
        n: u32,
        signals: &Signals,
        probe: &dyn Fn(i64, i64) -> bool,
        replay: &mut dyn FnMut() -> Option<String>,
        resumed: Resumed,
    ) -> AfterLimit {
        // Cleared here, so only a `Retry` below can leave it set and no
        // later attempt can inherit what an earlier wait claimed.
        self.said_try_now = false;
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
        // was launched. It refuses nothing (#2911); it is what the
        // `reentered` line reports and what `used` reads.
        let mark = self.mark();
        // `max_wait_secs` is the length of one wait, not a budget for the
        // step: a later wait gets the same length as the first, and no total
        // ends the chain (#2911). The floor that keeps a configured value
        // from planning a wait of no length is applied where the value is
        // read (`settings`).
        let max = settings.max_wait_secs;
        // Issue #2857: a wait had just ended claiming this login was usable
        // and the limit now being judged contradicts it. Reported once,
        // below; it refuses nothing.
        // Reported only when this step has waited on this login before, so
        // the line says what it has always said: the step is going *back*
        // into a wait on a login a signal had just claimed was usable. A
        // retry that merely landed on a login this step has never waited on
        // is that login's first wait and reads as one.
        let reentry = match resumed {
            Resumed::AfterASignal(met) if self.marks.waited_on(&mark) => Some(met),
            Resumed::AfterASignal(_) | Resumed::Ordinarily => None,
        };
        let now = self.clock.now();
        let step = self.step_timeout.map(|t| (self.step_start, t));
        let waiting = self.blocked.map_or(inherited, WaitFor::Skip);
        let (harness, p, shape) = match waiting {
            WaitFor::Reset { harness, reset } => {
                (harness, plan(now, reset, max, step), Shape::Reset)
            }
            WaitFor::Capacity { harness, reset, .. } => (
                harness,
                plan_capacity(now, reset, max, step),
                Shape::Capacity,
            ),
            WaitFor::LoginChange { harness } => (
                harness,
                plan_login_change(now, max, step),
                Shape::LoginChange,
            ),
            WaitFor::Skip(why) => {
                log(&Event::Skipped(Skip::Cannot(why)));
                return AfterLimit::Finish(None);
            }
        };
        self.marks.record(mark.clone());
        let account = mark.as_deref();
        // Each wait chooses its own retry login and only its own. The
        // clear is here, above the retry-before-wait return below, so a
        // value an earlier wait left behind can never pick the login a
        // later retry runs on: its freshness floor is older and would admit
        // readings taken before this limit was hit (issue #2731). It is
        // written again only by a wait that actually ran.
        self.retry_move = None;
        // Issue #2857: one line per re-entry, before the wait's own `start`,
        // so a re-entry whose limit the planner skipped writes nothing.
        if let Some(met) = reentry {
            emit(&Event::Reentered { met }, account);
        }
        // Issue #2911: a wait with no reset it can end on says so plainly,
        // on stderr and in the run's recipe log, naming the login and why.
        // Such a step may wait a long time, and an operator must be able to
        // see that it is waiting for capacity rather than hung.
        if let Some(why) = waiting.waiting_for_capacity() {
            emit(
                &Event::WaitingForCapacity {
                    harness,
                    why,
                    until: p.until,
                },
                account,
            );
        }
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
        //
        // Issue #2857: a login that moved is not by itself a login that can
        // be used. This is the one early exit that never sleeps, so a swap
        // onto a login already at its limit sends the retry straight at it;
        // the same room gate the wait's own exits use holds it back, and the
        // step then waits as it would have had nothing moved.
        if changed(&self.attempt_login, self.watch.login()) && change_may_end(&p, &self.watch) {
            // The step's own deadline still binds. A wait may not outlive
            // the step, and neither may a retry started in its place: with
            // the deadline already behind us `wait` would return
            // `StepTimeout` at once, so this zero-sleep path must finish
            // the same way rather than launch a fresh attempt the runner
            // is about to kill (issue #2731).
            if p.bound == Bound::StepTimeout && self.clock.now() >= p.until {
                emit(&Event::End(End::StepTimeout), account);
                log(&Event::Failed(FailReason::StepTimeout));
                return AfterLimit::Finish(None);
            }
            emit(&Event::End(End::LoginChanged), account);
            if no_prompt_for_a_retry(replay) {
                return AfterLimit::Finish(None);
            }
            log(&Event::Resume {
                attempt: n.saturating_add(1),
            });
            // A login change claims the retry will work, so a limit on that
            // retry contradicts it (issue #2857).
            self.said_try_now = End::LoginChanged.said_try_now();
            // Nothing is recorded in the wait ledger: no time passed.
            return AfterLimit::Retry;
        }
        let mut deps = RealDeps {
            clock: &mut self.clock,
            watch: &self.watch,
            signals,
            parent: self.parent,
            probe,
            attempt_login: &self.attempt_login,
            plan: &p,
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
        let capacity = p.kind == PlanKind::Capacity;
        self.retry_move = (capacity || room_was_seen).then_some(RetryMove {
            start: now,
            room_was_seen,
        });
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
            // Issue #2911: a capacity wait that reached its deadline, and a
            // login-change wait that reached its, each retry rather than end
            // the step. Nothing has lifted the limit, and the retry may well
            // meet it again; that is another wait, not a failure. The two
            // used to write `failed reason=capacity_expired` and `failed
            // reason=spend_limit`, which between them ended most of the runs
            // #2911 reports.
            End::CapacityExpired
            | End::ResetReached
            | End::MaxReached
            | End::LoginChanged
            | End::RotatorCapacity
            | End::OtherCapacity => {
                if no_prompt_for_a_retry(replay) {
                    return AfterLimit::Finish(None);
                }
                log(&Event::Resume {
                    attempt: n.saturating_add(1),
                });
                // Issue #2857: only an end that claimed the retry would work
                // can be contradicted by a limit on it. `max reached` is the
                // wait's own ceiling and claims nothing.
                self.said_try_now = end.said_try_now();
                AfterLimit::Retry
            }
        }
    }
}

/// Whether the retry this wait earned has no prompt to read, asked of
/// `replay` the moment before the retry would be announced (issue #2816).
///
/// A step whose prompt arrives on a pipe is replayed from the copy the
/// supervisor kept (issue #2815), and there are two ways that copy can be
/// missing when the wait ends: it was lost, or the caller never finished
/// writing the prompt and so there is no whole copy of it to give. Either
/// way the retry cannot run, and the wait's own vocabulary already has the
/// word for it. The reason is printed because `piped_prompt` alone does not
/// say which of the two it was.
fn no_prompt_for_a_retry(replay: &mut dyn FnMut() -> Option<String>) -> bool {
    let Some(why) = replay() else {
        return false;
    };
    eprintln!("amplihack: no retry after the wait: this step's prompt {why}");
    log(&Event::Skipped(Skip::Cannot(NoWait::PipedPrompt)));
    true
}

/// Whether a *login change* may end this wait: the login now live has room
/// (issue #2857).
///
/// A change says the login moved; it does not say the login it moved to can
/// be used. On 19 November 2026 a rotator swapped between logins at 98-100%
/// of their session window, and every swap woke every waiting run onto a
/// login that answered the first call with a 429. The rotator is the one
/// that chose the login and the one that measured it, so its own record of
/// the choice is what this reads: the last cycle had capacity and claims
/// room, which is `CycleStatus::says_room`, the same rule `fresh_room` uses
/// for the rotator-capacity exit, so the two cannot disagree.
///
/// A person's own `profile switch` is the one record that claims room
/// without a reading (outcome `switched`, issue #1691). The rotator measured
/// nothing about the login that person chose, and the wake #1691 gave
/// someone unsticking a waiting run stays unless the record positively says
/// `live_room: false`. The incident was the rotator's own `swapped`
/// throughout, and `swapped` and `stayed` still need a recorded `true`.
///
/// Three exemptions open the gate, each because the ledger says nothing
/// about the question:
///
/// - a login-change wait (issue #2731). It waits for one organisation's
///   spend limit to be replaced by another organisation's credit, and no
///   capacity reading reports credit, so requiring room would mean such a
///   wait could never end early again. Part 1 is the backstop: a 429 on the
///   retry it wakes re-enters the wait rather than ending the run.
/// - a step that is not on `$HOME/.claude`. The rotator swaps logins into
///   that directory and no other, so an inherited profile's or an explicit
///   `CLAUDE_CONFIG_DIR`'s login is not in its ledger at all and their own
///   fingerprint exits must go on working.
/// - a `$HOME/.claude` step whose root holds no rotate ledger. A host that
///   never ran a rotator has no record to be wrong about.
///
/// Every other answer is closed. A ledger that cannot be read or trusted, an
/// unknown version, a cycle that is not capacity, and a rotator cycle from
/// before #2857 that records no bit at all are all "no room", so the wait
/// sleeps to its reset instead of retrying onto a login that is full.
/// That costs an early wake during a rotator upgrade and nothing else; the
/// opposite direction is the bug.
fn change_may_end(plan: &plan::Plan, watch: &Watch) -> bool {
    if plan.is_login_change() || !watch.is_home() {
        return true;
    }
    match watch.rotator_state() {
        Ledger::Missing => true,
        Ledger::Unreadable => false,
        Ledger::Read(c) => c.has_capacity() && c.says_room(),
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
    args: claude_args::ClaudeArgs,
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
    let home_takeover = super::auto_switch::takeover::home_check(home_store);
    // Issue #2911: a login-change wait no longer asks whether a rotator is
    // running. With none running nothing ends the wait early, so it runs its
    // length and the step retries; before, it ended the step instead.
    super::auto_switch::rerun::run(
        None,
        Waiter::new(Some(settings), watch, args, home_takeover),
    )
}
