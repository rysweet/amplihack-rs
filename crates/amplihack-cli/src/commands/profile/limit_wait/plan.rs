//! What a usage limit allows, and the wait's deadline (issues #1618, #2911).
//!
//! No limit this module reads is refused a wait, and no count or budget here
//! ends a step that keeps meeting limits. A reset the step can use within
//! five hours is waited for directly. Every other limit becomes a wait for
//! *capacity* (a login with room, the rotator's next cycle, or the login in
//! `$HOME/.claude` changing) and says why it has no reset to end on
//! ([`Waiting`]). Two readings are left that no wait can help, and neither is
//! a limit: output with no limit message in it at all, and a prompt that
//! could not be read to its end ([`NoWait`]).
//!
//! # What ends a wait, and the one timed bound left
//!
//! #2911 asks for a wait that is woken by limit events: the recorded reset
//! passing, a login with room, the login changing. How close a wait comes
//! to that turns on whether its limit stated a reset the step can sleep to.
//!
//! - **A reset wait** ([`plan`]) has no timed bound. It ends at its reset
//!   plus [`MARGIN`], or sooner on room or a login change, and on nothing
//!   else. The configured length does not shorten it (round 6 of the review
//!   of PR #2992).
//! - **A capacity wait** ([`plan_capacity`]) and **a login-change wait**
//!   ([`plan_login_change`]) end on room or a login change too, and a
//!   capacity wait at its failed login's own reset when that comes first.
//!   Failing those, they end at an **outer bound**
//!   ([`Bound::MaxWait`]): `recipe.usage_limit_max_wait_secs`, or
//!   [`CAPACITY_DEFAULT_MAX`] when unset, never less than
//!   [`MIN_WAIT_SECS`]. Reaching it runs the step once more, and a limit on
//!   that run starts another wait. It never ends the step.
//!
//! The outer bound is a timer, not a limit event. It is kept as the interim
//! outer bound that #2909 allows "while this lands", for two reasons. A session, weekly or credits
//! limit that states no reset records no block (`rotate::step_block`), so
//! nothing records when it lifts, and on a host with no rotator and no other
//! login nothing but this bound would ever run such a step again without a
//! person. And a capacity wait whose limit stated a reset more than five
//! hours away is still reached in steps of the bound rather than waited
//! for, because waiting for that reset alone needs it held to its kind's
//! window, as the ledger holds it, which this module does not yet do.
//! Removing the bound is #3098, together with the 30-second pass of the
//! wait loop (`wait`).
//!
//! The deadline is the earlier of the rows a wait has, a tie named by the
//! reset row. All arithmetic saturates.
//!
//! **Every plan a limit produces ends strictly after `now`.** That is the
//! whole of the pacing rule, and it replaces the per-step budget #2731
//! used: a chain of limits is paced by each wait costing time, not by a
//! counter that eventually refuses one. It holds because a reset row is
//! only built from a reset in the future, and because [`MIN_WAIT_SECS`]
//! floors the configured outer bound where it is read (`settings`), so no
//! outer bound can land on `now` either.
//!
//! There is no step-timeout row (issue #3037, review of PR #2992). One used
//! to pull the deadline back to `$AMPLIHACK_STEP_TIMEOUT` less a minute and
//! then end the step, on the reading that the recipe runner kills a step at
//! that time. It does not: recipe-runner-rs enforces only a step's own YAML
//! `timeout:` field and reads the variable for nothing else, and nothing in
//! amplihack enforces it either (`--step-timeout` says as much). So that
//! row was the only code anywhere that turned the variable into an ending,
//! and the only steps it ended were steps waiting on a limit: a wall-clock
//! budget for limits, which #2911 removes.

use crate::commands::recipe::run::limit_patterns::{Harness, LimitKind, LimitMessage};

/// Added to a reset still in the future.
pub(crate) const MARGIN: i64 = 120;
/// A reset further away than this is not waited for directly: the router
/// ([`wait_for_message`], [`from_reading`]) makes it a capacity wait.
pub(crate) const FIVE_HOURS: i64 = 5 * 3600;
/// The outer bound of a capacity or login-change wait when nothing sets
/// one: the schema's default, six hours.
///
/// A timer, kept as the interim outer bound until the wait wakes on events
/// alone (#3098; see the module docs). A reset wait never reaches it.
pub(crate) const CAPACITY_DEFAULT_MAX: u64 = 21_600;
/// The shortest outer bound a configured length may ask for: two passes of
/// the wait loop (issue #2911).
///
/// The loop reads its files every 30 seconds (`wait`), so an outer bound
/// shorter than one pass could expire before any reading had a chance to
/// end the wait first, and every expiry runs the step's agent again: a chain
/// of those would be a busy loop of agent launches. It floors the outer
/// bound and adds no timer of its own, and it goes when the outer bound
/// does (#3098).
pub(crate) const MIN_WAIT_SECS: u64 = 60;

/// Why no wait can help, whatever the output said.
///
/// Both are readings of the *step*, not of a limit: neither says anything
/// about an account's windows, so no amount of waiting changes either. Every
/// limit this module can name waits (issue #2911).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoWait {
    /// The output has no line in any harness's own limit-message shape.
    /// Logged as `skipped reason=no_limit_message`.
    NoLimitMessage,
    /// The step's prompt came on a pipe and could not be read to its end:
    /// the source failed, or the copier panicked part way through it
    /// (issues #2816, #3036). The attempt in hand was given part of its
    /// task, so the step fails whether or not it met a limit, and no retry
    /// can be given more. It is a lost prompt, not a limit ending, and is
    /// logged as `failed reason=prompt_cut_short`, never as a skipped wait
    /// (review of PR #2992). The supervisor says so in words beside the
    /// event, with the limit's reset (`ending::prompt_lost_note`).
    ///
    /// It replaces `piped_prompt`, which also covered a copy that failed or
    /// could not be set up. Neither can end a step on a limit any more: the
    /// copy is kept in memory, so every byte the copier reads is a byte a
    /// retry is given, and a step whose copy cannot be set up is not run.
    PromptCutShort,
}

impl NoWait {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            NoWait::NoLimitMessage => "no_limit_message",
            NoWait::PromptCutShort => "prompt_cut_short",
        }
    }
}

/// Why a wait has no reset of its own to end on, and so waits for capacity
/// (issue #2911).
///
/// A closed set, carrying no text from the harness's output. Three of the
/// names are the ones that used to be logged as a terminal `skipped
/// reason=...`, kept so that an operator grepping for `reset_too_far`,
/// `no_reset` or `spend_limit` still finds the step's lines, now attached to
/// a wait rather than to a step that ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Waiting {
    /// The stated reset is more than five hours away, so the step waits for
    /// a login with room instead of sleeping to it.
    ResetTooFar,
    /// The message states no reset that parses.
    NoReset,
    /// A spend limit: no usage window's reset lifts one, so the step waits
    /// for the login itself to change.
    SpendLimit,
    /// Codex workspace credits, which state no reset at all.
    Credits,
    /// The stated reset is already behind the clock. The message may be
    /// minutes stale, so the step waits for a reading rather than for a time
    /// that has been and gone (issue #2857).
    PassedReset,
}

impl Waiting {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Waiting::ResetTooFar => "reset_too_far",
            Waiting::NoReset => "no_reset",
            Waiting::SpendLimit => "spend_limit",
            Waiting::Credits => "credits",
            Waiting::PassedReset => "passed_reset",
        }
    }
}

/// What a usage-limited attempt allows: a wait for `reset`, a wait for
/// another login with capacity (issue #1801), a wait for the login in
/// `$HOME/.claude` to change (issue #2731), or, for the two readings that
/// are not limits at all, none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitFor {
    Reset {
        harness: Harness,
        reset: i64,
    },
    Capacity {
        harness: Harness,
        reset: Option<i64>,
        /// Why this limit has no reset the step can wait for.
        why: Waiting,
    },
    /// The login in `$HOME/.claude` changing (issue #2731): a spend limit
    /// that states no reset. No reading reports an organisation's credit,
    /// so such a wait is never ended by one ([`Plan::capacity_may_end`]).
    LoginChange {
        harness: Harness,
    },
    Skip(NoWait),
}

impl WaitFor {
    /// The reset the limit stated, as read, whatever the wait does with it;
    /// `None` when it stated none and for a skip (review of PR #2992).
    ///
    /// Read by the one ending a step that has met a limit can still reach,
    /// a prompt that could not be read to its end, so it can say when the
    /// limit lifts rather than only that it has not. A `Capacity` wait keeps
    /// its limit's reset even when it is too far away to sleep to, and that
    /// is still the right answer to *when does this clear*.
    pub(crate) fn reset(self) -> Option<i64> {
        match self {
            WaitFor::Reset { reset, .. } => Some(reset),
            WaitFor::Capacity { reset, .. } => reset,
            WaitFor::LoginChange { .. } | WaitFor::Skip(_) => None,
        }
    }

    /// Why this wait has no reset to end on, for the `waiting_for_capacity`
    /// line; `None` for a wait that has one and for a skip.
    pub(crate) fn waiting_for_capacity(self) -> Option<Waiting> {
        match self {
            WaitFor::Capacity { why, .. } => Some(why),
            WaitFor::LoginChange { .. } => Some(Waiting::SpendLimit),
            WaitFor::Reset { .. } | WaitFor::Skip(_) => None,
        }
    }
}

/// What the attempt's parsed limit message allows at `now`.
///
/// Every limit waits. Which wait it is turns on one question: is there a
/// reset the step can usefully sleep to? A spend limit has none by
/// definition, Codex's credits state none, a message may state none that
/// parses, and a reset days away or already behind the clock is not one the
/// step can use. Each of those becomes a wait for capacity, named by its
/// [`Waiting`]; the rest is a reset wait, exactly as before.
pub(crate) fn wait_for_message(message: Option<LimitMessage>, now: i64) -> WaitFor {
    let Some(m) = message else {
        return WaitFor::Skip(NoWait::NoLimitMessage);
    };
    let capacity = |why| WaitFor::Capacity {
        harness: m.harness,
        reset: m.reset,
        why,
    };
    match m.reset {
        // No usage window's reset lifts a spend limit, and no login of the
        // same organisation would either, so this waits for the login
        // itself to change (issue #2731).
        None if m.spend() => WaitFor::LoginChange { harness: m.harness },
        None if m.kind == LimitKind::Credits => capacity(Waiting::Credits),
        None => capacity(Waiting::NoReset),
        // A spend-limit line that also says when the session or weekly
        // window resets is waited out like any other: that window is what
        // stops work first.
        Some(r) if r > now.saturating_add(FIVE_HOURS) => capacity(Waiting::ResetTooFar),
        Some(r) if r <= now => capacity(Waiting::PassedReset),
        Some(reset) => WaitFor::Reset {
            harness: m.harness,
            reset,
        },
    }
}

/// The wait for a limit read from the auto-switch readings rather than an
/// attempt's output: the inherited account was already exhausted, so the
/// step never ran under it. `reset` is when its exhausted window resets.
pub(crate) fn from_reading(harness: Harness, reset: i64, now: i64) -> WaitFor {
    let capacity = |why| WaitFor::Capacity {
        harness,
        reset: Some(reset),
        why,
    };
    if reset > now.saturating_add(FIVE_HOURS) {
        capacity(Waiting::ResetTooFar)
    } else if reset <= now {
        capacity(Waiting::PassedReset)
    } else {
        WaitFor::Reset { harness, reset }
    }
}

/// The capacity wait a bare 429 asks for (issue #2857), or `None` when
/// nothing could end one.
///
/// This is the one reading left that is not a limit message. A 429 states
/// no reset and says nothing about an account's windows, so unlike a limit
/// it is not by itself a reason to hold the step: with no login that could
/// take over, it goes back to the recipe runner as the transient transport
/// fault the classifier calls it everywhere else, and no line is written.
///
/// `takeover` reads the profile store, so it is called only here, for the
/// one reading that needs the answer.
pub(crate) fn rate_limit_capacity(
    harness: Harness,
    takeover: impl Fn() -> bool,
) -> Option<WaitFor> {
    takeover().then_some(WaitFor::Capacity {
        harness,
        reset: None,
        why: Waiting::NoReset,
    })
}

/// Which row set the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bound {
    /// The limit's own reset plus [`MARGIN`]: the recorded reset passing,
    /// which is a limit event.
    ResetMargin,
    /// The outer bound of a capacity or login-change wait: a timer, kept as
    /// the interim outer bound until the wait wakes on events alone (#3098).
    /// A reset wait never has it.
    MaxWait,
}

impl Bound {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Bound::ResetMargin => "reset+margin",
            Bound::MaxWait => "max_wait",
        }
    }
}

/// What a wait waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanKind {
    /// The limit's own reset, or a login change.
    Reset,
    /// Any login with capacity (issue #1801).
    Capacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) until: i64,
    pub(crate) bound: Bound,
    pub(crate) kind: PlanKind,
    /// The reset as read, for the log line; a capacity wait may have none.
    pub(crate) reset: Option<i64>,
    /// This wait is for the login in `$HOME/.claude` changing, and for
    /// nothing else (issue #2731). Set by [`plan_login_change`] alone, which
    /// is why it is private: no other constructor, and no test, may raise
    /// it. It is not a [`PlanKind`], because the kind says what the deadline
    /// means and this plan's deadline means what [`PlanKind::Reset`]'s does.
    login_change: bool,
}

impl Plan {
    /// Whether this wait is for the login in `$HOME/.claude` changing, and
    /// for nothing else (issue #2731). The reader of the private field.
    pub(crate) fn is_login_change(&self) -> bool {
        self.login_change
    }

    /// Whether a capacity reading may end this wait.
    ///
    /// True for a reset wait (issue #2694: a login with room ends it rather
    /// than the step sleeping to a reset hours away) and for a capacity wait
    /// (issue #1801: that is what it waits for). False for a login-change
    /// wait, whose limit is on an organisation's credit: no reading reports
    /// credit, and an exhausted credit leaves the session and weekly windows
    /// looking healthy, so a reading would end the wait and send the step
    /// back onto the same spent login.
    pub(crate) fn capacity_may_end(&self) -> bool {
        !self.is_login_change()
    }

    /// A plan with a deadline the caller chooses, for the wait's own tests.
    ///
    /// There is deliberately no way to raise `login_change` here:
    /// [`plan_login_change`] is its one source, so a test that wants such a
    /// plan must call that and is then testing what the code does.
    #[cfg(test)]
    pub(super) fn for_test(until: i64, bound: Bound, kind: PlanKind, reset: Option<i64>) -> Self {
        Self {
            until,
            bound,
            kind,
            reset,
            login_change: false,
        }
    }
}

/// The deadline for a reset wait starting at `now` for `reset`: the reset
/// plus the margin, and nothing else.
///
/// A reset wait has no timed bound (round 6 of the review of PR #2992). The
/// router builds one only from a reset strictly ahead of the clock and
/// within five hours, so its reset is the limit event it waits for: the wait
/// ends there, or sooner on a login with room or a login change. The
/// configured length used to shorten it, which turned a reset wait into a
/// retry on a timer whenever the length was below the reset's distance; it
/// no longer does.
///
/// There is no five-hour ceiling here either. The five-hour rule decides
/// which wait a limit earns, and it is applied once, by the router: a reset
/// beyond it never reaches this function but becomes a capacity wait, whose
/// outer bound is the timer that reaches a far reset in steps
/// ([`plan_capacity`]). A ceiling repeated here could only have cut short a
/// wait the router had already judged.
///
/// The row is floored at `now + MARGIN`, so the deadline is after `now`
/// even for a reset no caller should have passed in.
pub(crate) fn plan(now: i64, reset: i64) -> Plan {
    Plan {
        until: reset.max(now).saturating_add(MARGIN),
        bound: Bound::ResetMargin,
        kind: PlanKind::Reset,
        reset: Some(reset),
        login_change: false,
    }
}

/// The deadline for a capacity wait starting at `now`: the earlier of the
/// failed login's own `reset` plus the margin (when still in the future)
/// and the outer bound `max` from now. A tie is named `ResetMargin`. There
/// is no five-hour ceiling. Reaching the reset retries the step, because
/// that login has recovered; reaching the outer bound retries it too (issue
/// #2911), and that is the timed re-entry declared in the module docs and
/// removed by #3098.
///
/// A weekly limit's reset, days away, is waited for here and not in
/// [`plan`], so it is this function's outer bound, not a ceiling on a reset
/// wait, that reaches such a reset in steps.
///
/// `max` has already been floored at [`MIN_WAIT_SECS`] where it was read, and
/// the reset row is only applied for a reset still ahead of `now`, so the
/// deadline is always after `now`.
pub(crate) fn plan_capacity(now: i64, reset: Option<i64>, max: Option<u64>) -> Plan {
    let mut until = now.saturating_add(secs(max.unwrap_or(CAPACITY_DEFAULT_MAX)));
    let mut bound = Bound::MaxWait;
    if let Some(r) = reset.filter(|r| *r > now) {
        let end = r.saturating_add(MARGIN);
        if end <= until {
            until = end;
            bound = Bound::ResetMargin;
        }
    }
    Plan {
        until,
        bound,
        kind: PlanKind::Capacity,
        reset,
        login_change: false,
    }
}

/// The deadline for a login-change wait starting at `now`: the outer bound
/// `max` from now. There is no reset row, because the limit stated none,
/// and no five-hour ceiling. The login changing is the event this wait is
/// for; the outer bound is the declared interim timer (#3098) that runs the
/// step again when no change comes, which on a host with no rotator is the
/// only thing that would.
///
/// The plan's kind is [`PlanKind::Reset`] on purpose: the kind says what
/// the deadline means, and reaching this one retries the step, as reaching a
/// reset wait's own deadline does. What this wait does *not* consult is
/// carried by [`Plan::capacity_may_end`] instead: no reading says whether
/// the next login's organisation has credit, and the rotator picks what goes
/// into `~/.claude` in any case.
pub(crate) fn plan_login_change(now: i64, max: Option<u64>) -> Plan {
    let until = now.saturating_add(secs(max.unwrap_or(CAPACITY_DEFAULT_MAX)));
    Plan {
        until: until.max(now),
        bound: Bound::MaxWait,
        kind: PlanKind::Reset,
        reset: None,
        login_change: true,
    }
}

fn secs(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}
