//! The usage-limit wait for recipe agent steps (issue #1618).
//!
//! When a supervised `-p` step fails on a usage limit and no auto-switch
//! rerun is left, the step waits for the reset (or for the login to change),
//! then runs once more with the same inputs. A limit that will not reset soon
//! waits for another login with capacity instead (issue #1801), and a spend
//! limit that states no reset waits for the rotator to move another login in
//! (issue #2731).
//!
//! Issue #2911: no limit is refused a wait, and nothing here counts or
//! totals a step's waits. Every limit this module can read becomes a wait, a
//! step may wait as many times as it meets one, and the chain is paced by
//! the waits costing time rather than by a count or a budget. A wait with no
//! reset it can end on says so plainly, as `waiting_for_capacity`, naming
//! the login and why. What still ends a step that met a limit is a stop
//! signal, the end of its session, the wait turned off, output with no
//! limit message in it, and a prompt that could not be read to its end,
//! which is a lost prompt and not a limit ending.
//!
//! What ends a *wait* is a limit event: its reset, a login with room, or a
//! login change. A limit that states a reset still ahead is waited for to
//! that reset, however far away, held to its kind's window. A limit with no
//! usable reset has no deadline at all: nothing records when it lifts, so
//! the wait ends on a login with room or a login change, and says so
//! plainly as it starts (`plan`). No timer ends a wait. The wait blocks on
//! writes to the files that record those events, on the stop signals and on
//! the exit of the step's parent (`wake`), and reads its files only after a
//! write (`wait`).
//!
//! No clock ends a step. `$AMPLIHACK_STEP_TIMEOUT` used to: the wait
//! ended a minute before it and the step failed, on the reading that the
//! recipe runner kills a step at that time. It does not - recipe-runner-rs
//! enforces only a step's own YAML `timeout:` field - so this module was the
//! only code that turned the variable into an ending, and only for steps
//! waiting on a limit (issue #3037). It no longer reads it.
//!
//! A prompt that could not be read to its end fails its step whether or not
//! a limit is met, because the attempt was given less than its task, and no
//! retry can be given more (issue #3036). A step that meets a limit then
//! ends with `failed reason=prompt_cut_short`, never a skipped wait. A copy
//! that failed or could not be set up used to end a step on a limit too.
//! Neither can now: the copy is kept in memory, so every byte the copier
//! reads is a byte a retry is given, and a step whose copy cannot be set up
//! (a process out of descriptors or threads) is not run at all (`scratch`).
//!
//! Such a step's limit was reported to the recipe runner over the recovery
//! channel (`Outcome::UsageLimit`, issue #2334) the moment the attempt met
//! it, so the run's result carries `usage_limit` as its `failure_cause` and a
//! relaunch resumes at the step, whose whole prompt the runner sends again.
//! The integration test `issue_2911_prompt_loss_resumes` runs that path.
//! `ending` writes one sentence beside the event line for the person
//! reading the log.
//!
//! A prompt that is merely late is not lost, and nothing waits for it. A
//! retry, or an auto-switch rerun, starts at once and is fed the copy
//! already held and then the rest as the caller writes it (`scratch`). The
//! supervisor never waits on the caller's pipe.
//!
//! Each wait's `start`, `waiting_for_capacity` and `end` also reach the run's
//! recipe log, so a waiting step is visible there and not just in its own
//! stderr (`recipe_log`). See docs/reference/usage-limit-waits.md.
//!
//! Issue #2967 adds no timer. Whenever an attempt meets a limit, waited or
//! not, an auto-switch step first moves to another login the candidate rule
//! allows (`auto_switch::rerun`), and waits only when none can run it. That
//! wait's one deadline moves earlier when a limit block recorded on another
//! login the step could move to ends sooner (`plan::earliest`), and the
//! retry then runs on that login. Every wait names its login
//! (`Waiter::note_attempt_login`).
//!
//! `Waiter` decides what happens after a limit; the one supervisor loop,
//! `auto_switch::rerun::run`, runs the attempts with or without auto-switch.
//! Log lines are built only from static text, integers and parsed instants.

pub(crate) mod claude_args;
mod clock;
mod ending;
mod events;
pub(crate) mod fingerprint;
mod ledger;
mod marks;
pub(crate) mod plan;
mod recipe_log;
pub(crate) mod scratch;
pub(crate) mod settings;
pub(crate) mod wait;
mod wake;

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
use ending::the_prompt_was_lost;
pub(crate) use events::Met;
use events::{Event, Shape, Skip, log, recipe_line};
use fingerprint::{Fingerprint, Source, Watch};
use marks::Marks;
use plan::{Bound, NoWait, PlanKind, WaitFor, plan, plan_capacity, plan_login_change};
pub(crate) use recipe_log::RECIPE_LOG_ENV;
pub(crate) use settings::WaitSettings;
use std::path::PathBuf;
use std::time::Duration;
use wait::{End, WaitDeps};
use wake::Wake;

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
    /// the wait that follows is reported once, on a `reentered` line.
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
    wake: &'a mut Wake<'a>,
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

    fn block(&mut self, until: i64) -> bool {
        let left = until.saturating_sub(self.clock.now()).max(0);
        let d = Duration::from_secs(u64::try_from(left).unwrap_or(0));
        let spacing = self.clock.real(wake::READ_SPACING);
        let wake = &mut *self.wake;
        let deadline = self.plan.until.is_some();
        self.clock
            .block(d, deadline, |real| wake.block(real, spacing))
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
        // re-parents this process, so the id changes; the wake also sees
        // the exit itself.
        self.wake.parent_gone() || unsafe { libc::getppid() != self.parent }
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

/// The wait's state for one step: the logins it has waited on, and the
/// settings and clock its waits use. Nothing here counts or caps the waits.
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
    /// The name of the login the attempt now running was launched on (issue
    /// #2967): the profile the supervisor launched it under, else the
    /// inherited profile, else the rotator's live login for a
    /// `$HOME/.claude` step. It is the wait's mark and the `account=` of
    /// its recipe-log lines. `None` only where no name can be read: a host
    /// with no profile store, or a `$CLAUDE_CONFIG_DIR` step.
    attempt_name: Option<String>,
    clock: Clock,
    watch: Watch,
    /// The process that started this step, read before any attempt runs.
    parent: libc::pid_t,
    /// Why no wait can help this step, whatever the limit says, and the
    /// static words that say which case it is. The words are printed when
    /// the step that met a limit ends this way, never when the block is set,
    /// so a step that meets no limit says nothing. Only
    /// [`NoWait::PromptCutShort`] is ever set here.
    blocked: Option<(NoWait, &'static str)>,
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
    /// The earliest end, on the wait's clock, of a limit block recorded on
    /// another login this step could move to (issue #2967). Offered by the
    /// supervisor for the one wait it is about to plan and taken by
    /// [`Waiter::after_limit`] on every call, so no later wait inherits it.
    earliest_reset: Option<i64>,
    /// Whether the wait that last ran ended at that instant (issue #2967).
    /// Written and cleared with `retry_move`. It is not a second record of
    /// the deadline: it is computed in one expression where the wait ends,
    /// from that wait's own plan (`p.bound == Bound::EarliestReset`) and how
    /// it ended, so it cannot disagree with the bound the start line wrote.
    reached_earliest_reset: bool,
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
        Self {
            settings,
            marks: Marks::new(),
            attempt_login: None,
            attempt_name: None,
            clock: Clock::new(),
            watch,
            // SAFETY: getppid(2) has no preconditions.
            parent: unsafe { libc::getppid() },
            blocked: None,
            args,
            home_takeover,
            retry_move: None,
            earliest_reset: None,
            reached_earliest_reset: false,
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

    /// Reads the login the next attempt will run on, immediately before it
    /// is launched (issue #2731). On a shared-login host the rotator reacts
    /// to the same exhausted reading that fails the attempt, so the swap
    /// usually lands while the attempt is still failing; taking the key
    /// before the attempt is what lets `after_limit` see that.
    ///
    /// `profile` is the profile the supervisor is about to launch the
    /// attempt under, `None` for the inherited environment. The login's name
    /// is resolved here as well (issue #2967), by the resolution
    /// [`Waiter::attempt_profile`] already makes, so a step under a stored
    /// profile is marked with that profile and its wait says `account=<name>`.
    /// Before #2967 only a `$HOME/.claude` step had a name: every profile
    /// step shared the one unnamed mark and logged `account=unknown`.
    pub(crate) fn note_attempt_login(&mut self, profile: Option<&ProfileName>) {
        self.attempt_login = self.watch.login();
        self.attempt_name = self
            .attempt_profile(profile)
            .map(|p| p.to_string())
            // A live login whose name the profile rules refuse still marks
            // a `$HOME/.claude` step, as it did before #2967.
            .or_else(|| self.attempt_login.as_ref().map(|(_, n)| n.clone()));
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

    /// Never wait for this step, for `why`. `cause` is the static sentence
    /// [`Waiter::after_limit`] prints when the step ends this way: `why` is
    /// the logged reason an operator greps for, and a reason alone does not
    /// say what happened here, nor what has to be done about it.
    pub(crate) fn block(&mut self, why: NoWait, cause: &'static str) {
        self.blocked = Some((why, cause));
    }

    /// `WaitFor::Skip` for the reason set by `block`, if any. Passed to
    /// `after_limit` when no limit of the inherited profile is known, so the
    /// ending is still logged.
    pub(crate) fn blocked_skip(&self) -> Option<WaitFor> {
        self.blocked.map(|(why, _)| WaitFor::Skip(why))
    }

    /// Whether Claude's arguments carry the prompt, so stdin holds nothing
    /// this step needs.
    pub(crate) fn prompt_in_args(&self) -> bool {
        self.args.prompt
    }

    /// Whether another login could take over this `$HOME/.claude` step.
    /// Called when a limit is seen, so steps that never hit one make no
    /// reads.
    pub(crate) fn takeover_home(&self) -> bool {
        (self.home_takeover)()
    }

    /// The mark for the login the attempt now running was launched on: what
    /// [`Marks`] records for a wait, and the account the wait's lines name.
    ///
    /// The name [`Waiter::note_attempt_login`] resolved (issue #2967), so a
    /// profile step and a `$HOME/.claude` step are both marked with their
    /// own login.
    fn mark(&self) -> Option<String> {
        self.attempt_name.clone()
    }

    /// Whether the retry may run under another login, and what it costs if
    /// none has room; `None` keeps the inherited profile. Cleared as each
    /// wait is planned, so it only ever describes the wait that just ran.
    pub(crate) fn retry_move(&self) -> Option<RetryMove> {
        self.retry_move
    }

    /// Offers the wait about to be planned the earliest end, on the wait's
    /// clock, of a limit block recorded on another login this step could
    /// move to (issue #2967); `None` when there is none. The next call to
    /// [`Waiter::after_limit`] takes it, whatever that call does, and moves
    /// the wait's deadline there when it is sooner (`plan::earliest`).
    pub(crate) fn offer_earliest_reset(&mut self, at: Option<i64>) {
        self.earliest_reset = at;
    }

    /// Whether the wait that last ran ended at the instant
    /// [`Waiter::offer_earliest_reset`] gave it: `end reason=reset reached`
    /// with `bound=earliest_reset+margin` (issue #2967). The retry then runs
    /// on the login whose block ended there. Cleared as each wait is
    /// planned, so it only ever describes the wait that just ran.
    pub(crate) fn reached_earliest_reset(&self) -> bool {
        self.reached_earliest_reset
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
    /// whether the step's input can be given to one (issues #2816, #3036).
    /// It never waits: a prompt still arriving is fed to the retry as it
    /// arrives. Only an input no retry can ever be
    /// given ([`scratch::Replay::Never`], a prompt that could not be read to
    /// its end) ends the step, with the attempt's result and `failed
    /// reason=prompt_cut_short` (see `ending`): a lost prompt, not a limit
    /// ending.
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
    /// waits lasting until something changed rather than by a count or a
    /// budget: a plan with a deadline ends strictly after `now`, and one
    /// without ends only on an event (see `plan`). A wait whose limit stated
    /// a reset still ahead ends at that reset; one with no usable reset ends
    /// on a login with room or a login change. No timer ends a wait.
    /// `$AMPLIHACK_STEP_TIMEOUT` plays no part (issue #3037), and nor does
    /// `CI` (`settings`). What still ends a step is a stop signal, the end of
    /// its session, the wait turned off explicitly by the operator, output
    /// with no limit message in it, and a prompt that could not be read to
    /// its end, which fails the step whether or not it met a limit.
    pub(crate) fn after_limit(
        &mut self,
        inherited: WaitFor,
        n: u32,
        signals: &Signals,
        probe: &dyn Fn(i64, i64) -> bool,
        replay: &mut dyn FnMut() -> scratch::Replay,
        resumed: Resumed,
    ) -> AfterLimit {
        // Cleared here, so only a `Retry` below can leave it set and no
        // later attempt can inherit what an earlier wait claimed.
        self.said_try_now = false;
        // Issue #2967: taken on every call, so an instant offered for this
        // wait can never time a later one.
        let other_reset = self.earliest_reset.take();
        let Some(settings) = self.settings else {
            return AfterLimit::Finish(None);
        };
        if let Some(sig) = signals.received() {
            return AfterLimit::Finish(Some(128 + sig));
        }
        // The one configured ending: the operator turned the wait off, so the
        // limit result is passed up. `CI` does not turn it off (`settings`).
        if !settings.enabled {
            log(&Event::Skipped(Skip::Disabled));
            return AfterLimit::Finish(None);
        }
        // The mark is the login the failed attempt ran on, read before it
        // was launched. It refuses nothing (#2911); it is what the
        // `reentered` line reports.
        let mark = self.mark();
        // Issue #2857: a wait had just ended claiming this login was usable
        // and the limit now being judged contradicts it. Reported once,
        // below, and only when this step has waited on this login before: a
        // retry that merely landed on a login this step has never waited on
        // is that login's first wait and reads as one. It refuses nothing.
        let reentry = match resumed {
            Resumed::AfterASignal(met) if self.marks.waited_on(&mark) => Some(met),
            Resumed::AfterASignal(_) | Resumed::Ordinarily => None,
        };
        let now = self.clock.now();
        let waiting = self
            .blocked
            .map_or(inherited, |(why, _)| WaitFor::Skip(why));
        let (harness, p, shape) = match waiting {
            WaitFor::Reset { harness, reset } => (harness, plan(now, reset), Shape::Reset),
            WaitFor::Capacity { harness, reset, .. } => {
                (harness, plan_capacity(now, reset), Shape::Capacity)
            }
            WaitFor::LoginChange { harness } => (harness, plan_login_change(), Shape::LoginChange),
            WaitFor::Skip(NoWait::PromptCutShort) => {
                // The step ends because its prompt was lost, not because of
                // the limit. Its attempt was given part of its task, so it
                // fails whether or not it met one, and a retry would have no
                // whole prompt to read. The sentence beside the event names
                // the reset of the limit this step actually met (read from
                // `inherited`, since `waiting` is the block).
                let cause = self.blocked.map_or(NO_WHOLE_PROMPT, |(_, cause)| cause);
                the_prompt_was_lost(cause, inherited.reset(), now);
                log(&Event::PromptCutShort);
                return AfterLimit::Finish(None);
            }
            WaitFor::Skip(why) => {
                log(&Event::Skipped(Skip::Cannot(why)));
                return AfterLimit::Finish(None);
            }
        };
        // Issue #2967: a block recorded on another login ending is a limit
        // event too, and one this login's own files never record. Its end
        // moves the deadline earlier when it is sooner; the kind of wait,
        // and so what may end it, is unchanged.
        let p = plan::earliest(p, other_reset, now);
        self.marks.record(mark.clone());
        let account = mark.as_deref();
        // Each wait chooses its own retry login and only its own. The
        // clear is here, above the retry-before-wait return below, so a
        // value an earlier wait left behind can never pick the login a
        // later retry runs on: its freshness floor is older and would admit
        // readings taken before this limit was hit (issue #2731). It is
        // written again only by a wait that actually ran. The same holds
        // for the earliest-reset flag (issue #2967), which names the login a
        // retry runs on in the same way.
        self.retry_move = None;
        self.reached_earliest_reset = false;
        // Issue #2857: one line per re-entry, before the wait's own `start`,
        // so a re-entry whose limit the planner skipped writes nothing.
        if let Some(met) = reentry {
            emit(&Event::Reentered { met }, account);
        }
        // Issue #2911: a capacity or login-change wait says so plainly, on
        // stderr and in the run's recipe log, naming the login and why. Such
        // a step may wait a long time, and an operator must be able to see
        // that it is waiting rather than hung.
        if let Some(why) = waiting.waiting_for_capacity() {
            emit(
                &Event::WaitingForCapacity {
                    harness,
                    why,
                    until: p.until,
                },
                account,
            );
            // A wait with no deadline lasts until an event, which on a host
            // with no rotator and no other login may be a person's action;
            // the line above says so to a program, this says it to them.
            if p.until.is_none() {
                eprintln!("{NO_DEADLINE}");
            }
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
        // The watches are made before the login is read below, so a change
        // that lands after that reading wakes the wait.
        let mut wake = Wake::new(&self.watch, self.parent, signals.interrupt());
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
            emit(&Event::End(End::LoginChanged), account);
            if let Some(finish) = self.prompt_for_the_retry(replay, p.reset) {
                return finish;
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
            wake: &mut wake,
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
        // Issue #2967: the other login's block ended, so the retry runs on it.
        self.reached_earliest_reset = end == End::ResetReached && p.bound == Bound::EarliestReset;
        emit(&Event::End(end), account);
        match end {
            End::Signal(sig) => AfterLimit::Finish(Some(128 + sig)),
            End::SessionEnded => AfterLimit::Finish(None),
            // Every other end is a limit event, and earns a retry. A limit on
            // that retry is another wait, not a failure (issue #2911).
            End::ResetReached | End::LoginChanged | End::RotatorCapacity | End::OtherCapacity => {
                if let Some(finish) = self.prompt_for_the_retry(replay, p.reset) {
                    return finish;
                }
                log(&Event::Resume {
                    attempt: n.saturating_add(1),
                });
                // Issue #2857: each of these ends claimed the retry would
                // work, so a limit on it contradicts that claim.
                self.said_try_now = end.said_try_now();
                AfterLimit::Retry
            }
        }
    }

    /// The input for the retry this wait earned, asked of `replay` the
    /// moment before the retry would be announced: `None` when the retry
    /// can run, or how the step ends instead (issues #2816, #3036).
    ///
    /// A step whose prompt arrives on a pipe is replayed from the copy the
    /// supervisor kept (issue #2815), and a copy whose caller has not
    /// finished writing is replayed as it grows, so the retry runs at once
    /// either way. Nothing here waits on the caller's pipe; a wait for a
    /// late prompt held a step for ever on a caller that never closed its
    /// pipe (issue #3036).
    ///
    /// The one input that ends the step is one no retry can ever be given:
    /// a prompt that could not be read to its end. Its attempt was given
    /// less than its task, so the step fails whether or not it met a limit:
    /// a lost prompt, logged as `failed reason=prompt_cut_short`, with the
    /// reason printed on the line before. `reset` is the limit's own reset,
    /// so the person reading the log is told when a relaunch will find
    /// room, or that nothing here can say (`ending`).
    fn prompt_for_the_retry(
        &mut self,
        replay: &mut dyn FnMut() -> scratch::Replay,
        reset: Option<i64>,
    ) -> Option<AfterLimit> {
        match replay() {
            scratch::Replay::Ready => None,
            scratch::Replay::Never(why) => {
                eprintln!("amplihack: no retry after the wait: {why}");
                the_prompt_was_lost(NO_WHOLE_PROMPT, reset, self.clock.now());
                log(&Event::PromptCutShort);
                Some(AfterLimit::Finish(None))
            }
        }
    }
}

/// What a step whose wait has no deadline prints beside its
/// `waiting_for_capacity ... until=none` line (issue #2911): static text, so
/// no word of the harness's output reaches it.
const NO_DEADLINE: &str = "amplihack: nothing records when this limit lifts, so this step waits \
                           with no deadline until a login has room or the login changes. \
                           `amplihack profile switch` ends such a wait for a step on \
                           ~/.claude; a stop signal ends the step. See \
                           docs/reference/usage-limit-waits.md";

/// The cause `ending` gives when a retry's input can never be whole, which
/// means the prompt was cut short. The specific reason is printed on the
/// line before it.
const NO_WHOLE_PROMPT: &str = "its piped prompt could not be read to its end, so no retry can be \
                               given the whole of it, for the reason on the line above";

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
