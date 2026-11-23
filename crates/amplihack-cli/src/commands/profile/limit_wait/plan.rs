//! What a usage limit allows, and the wait's deadline (issues #1618, #2911).
//!
//! No limit this module reads is refused a wait, and no count or budget here
//! ends a step that keeps meeting limits. Two readings are left that no wait
//! can help, and neither is a limit: output with no limit message in it at
//! all, and a prompt that could not be read to its end ([`NoWait`]).
//!
//! # What ends a wait
//!
//! A wait ends on a limit event and on nothing else: its reset passing, a
//! login with room, or the login changing (and, for the step rather than
//! the limit, a stop signal or the end of its session). No timer ends a
//! wait. Which of those events a wait has turns on the reset its limit
//! stated.
//!
//! - **A limit that states a reset still ahead** waits for that reset.
//!   Within five hours it is a reset wait ([`plan`]). Further away it is a
//!   capacity wait ([`plan_capacity`]), which may also move the retry to
//!   another login with room, and still ends at its reset plus [`MARGIN`].
//!   A weekly limit days away is waited for once, to its reset. The reset is
//!   first held to its kind's window ([`held`]), as the rotate ledger holds
//!   a block, so a forged or misread message cannot hold a step longer than
//!   the window it names allows.
//! - **A limit with no usable reset** (none stated, Codex credits, a reset
//!   already behind the clock, a bare 429 another login could take over, or
//!   a spend limit stating none) has no deadline ([`Plan::until`] is
//!   `None`). Nothing records when such a limit lifts, so the wait waits for
//!   the events that can be seen: a login with room or a login change. It
//!   says so plainly as it starts (`waiting_for_capacity ... until=none`),
//!   so an operator can see that it is waiting and why (issue #2911: "when
//!   nothing can be known about when room returns, it still waits, and says
//!   plainly ... that it is waiting for capacity and why").
//!
//! The second kind has no timed outer bound, and none may be added. Each
//! time such a bound passed it would run the step's agent again on a login
//! that may still be spent: a poll of the harness on a clock, which #2911
//! rules out ("no wall-clock budget ... remove the paths above rather than
//! raising their numbers"). A person who wants such a step to run again
//! makes an event: `amplihack profile switch` for a step on `$HOME/.claude`
//! (its `switched` record ends the wait as `rotator capacity`), logging the
//! step's own config directory in to another account, or a stop signal.
//!
//! **Every plan with a deadline ends strictly after `now`**, and a plan
//! with none ends only on an event. That is the pacing rule that replaced
//! the per-step budget of #2731: a chain of limits is paced by each wait
//! lasting until something changed, not by a counter that eventually
//! refuses one, and no plan can make a busy loop of agent launches.
//!
//! There is no step-timeout row (issue #3037). `$AMPLIHACK_STEP_TIMEOUT`
//! is not enforced on a step by the recipe runner, which enforces only a
//! step's own YAML `timeout:`, so a row built from it ended only steps that
//! were waiting on a limit.

use super::super::rotate::ledger::MAX_BLOCK_SECS;
use crate::commands::recipe::run::limit_patterns::{Harness, LimitKind, LimitMessage};

/// Added to a reset still in the future.
pub(crate) const MARGIN: i64 = 120;
/// A reset further away than this is not waited for directly: the router
/// ([`wait_for_message`], [`from_reading`]) makes it a capacity wait. It is
/// also the session window's length ([`window`]).
pub(crate) const FIVE_HOURS: i64 = 5 * 3600;
/// The weekly window's length.
pub(crate) const SEVEN_DAYS: i64 = 7 * 86_400;

/// The longest the usage window of a limit of `kind` lasts, or `None` for a
/// kind with no fixed window.
///
/// A session window is five hours and a weekly window seven days, so no
/// genuine reset of either lies further away. A spend or credit limit lifts
/// on a billing date or when someone adds funds, which has no fixed
/// distance. The rotate ledger holds a block to the same windows
/// (`rotate::step_block`).
pub(crate) fn window(kind: LimitKind) -> Option<i64> {
    match kind {
        LimitKind::Session => Some(FIVE_HOURS),
        LimitKind::Weekly => Some(SEVEN_DAYS),
        LimitKind::Spend | LimitKind::Credits => None,
    }
}

/// `reset`, held to `kind`'s window from `now`: the reset a wait may end on.
///
/// The step cannot tell the harness's own message from a model that printed
/// one as its last line, so a stated reset is never trusted further than
/// the window its kind allows. A kind with no window is held to the rotate
/// ledger's span rule ([`MAX_BLOCK_SECS`]), so no message can hold a step
/// for longer than a block may last. A genuine reset always falls inside, so
/// a genuine message is never shortened. `kind` is `None` for a reset read
/// from the auto-switch readings, which say only that a window is
/// exhausted; it is held to the longest window, the weekly one.
pub(crate) fn held(reset: i64, now: i64, kind: Option<LimitKind>) -> i64 {
    let longest = match kind {
        Some(k) => window(k).unwrap_or(MAX_BLOCK_SECS),
        None => SEVEN_DAYS,
    };
    reset.min(now.saturating_add(longest))
}

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
    /// logged as `failed reason=prompt_cut_short`, never as a skipped wait.
    ///
    /// It is narrower than the `piped_prompt` ending it replaced, which also
    /// covered a copy that failed or could not be set up. Neither can end a
    /// step on a limit any more: the copy is kept in memory, so every byte
    /// the copier reads is a byte a retry is given, and a step whose copy
    /// cannot be set up is not run. The run resumes the step through the
    /// recipe runner's recorded `usage_limit` cause (`ending`).
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

/// Why a wait is for capacity rather than for a reset within five hours
/// (issue #2911).
///
/// A closed set, carrying no text from the harness's output. Three of the
/// names are the ones that used to be logged as a terminal `skipped
/// reason=...`, kept so that an operator grepping for `reset_too_far`,
/// `no_reset` or `spend_limit` still finds the step's lines, now attached to
/// a wait rather than to a step that ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Waiting {
    /// The stated reset is more than five hours away. The step moves to a
    /// login with room if one appears, and otherwise waits for that reset,
    /// held to its kind's window ([`held`]).
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
    /// The message states its reset as a clock time with no zone, as in
    /// `your 5-hour limit resets 11:50pm` (issue #2967). No zone is guessed,
    /// so the reset is not read; the line names the cause instead of
    /// reading as a limit that stated no reset.
    ResetWithoutZone,
}

impl Waiting {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Waiting::ResetTooFar => "reset_too_far",
            Waiting::NoReset => "no_reset",
            Waiting::SpendLimit => "spend_limit",
            Waiting::Credits => "credits",
            Waiting::PassedReset => "passed_reset",
            Waiting::ResetWithoutZone => "reset_without_zone",
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
    /// The reset the limit stated, held to its kind's window, whatever the
    /// wait does with it; `None` when it stated none and for a skip.
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

    /// Why this wait is for capacity, for the `waiting_for_capacity` line;
    /// `None` for a reset wait and for a skip.
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
/// Every limit waits. A reset within five hours is a reset wait. A reset
/// further away is a capacity wait that still ends at that reset, held to
/// the limit's kind's window ([`held`]). A spend limit stating no reset
/// waits for the login to change; Codex's credits, a message stating no
/// reset that parses and a reset already behind the clock each wait for
/// capacity with no reset to end on. Each is named by its [`Waiting`].
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
        // A reset stated with no zone is not read (issue #2967). The message
        // still names a usage window that resets, so even a spend-limit line
        // is waited out like the one below that states a readable reset: as
        // capacity, which a login with room, a login change or another
        // login's reset ends. A login-change wait would end on the first
        // alone.
        None if m.reset_without_zone => capacity(Waiting::ResetWithoutZone),
        // No usage window's reset lifts a spend limit, and no login of the
        // same organisation would either, so this waits for the login
        // itself to change (issue #2731).
        None if m.spend() => WaitFor::LoginChange { harness: m.harness },
        None if m.kind == LimitKind::Credits => capacity(Waiting::Credits),
        None => capacity(Waiting::NoReset),
        // A spend-limit line that also says when the session or weekly
        // window resets is waited out like any other: that window is what
        // stops work first.
        Some(r) if r > now.saturating_add(FIVE_HOURS) => WaitFor::Capacity {
            harness: m.harness,
            reset: Some(held(r, now, Some(m.kind))),
            why: Waiting::ResetTooFar,
        },
        Some(r) if r <= now => capacity(Waiting::PassedReset),
        Some(reset) => WaitFor::Reset {
            harness: m.harness,
            reset,
        },
    }
}

/// The wait for a limit read from the auto-switch readings rather than an
/// attempt's output: the inherited account was already exhausted, so the
/// step never ran under it. `reset` is when its exhausted window resets; a
/// reset beyond five hours is held to the weekly window ([`held`]).
pub(crate) fn from_reading(harness: Harness, reset: i64, now: i64) -> WaitFor {
    let capacity = |reset, why| WaitFor::Capacity {
        harness,
        reset: Some(reset),
        why,
    };
    if reset > now.saturating_add(FIVE_HOURS) {
        capacity(held(reset, now, None), Waiting::ResetTooFar)
    } else if reset <= now {
        capacity(reset, Waiting::PassedReset)
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

/// What sets the deadline, for the `start` line's `bound=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bound {
    /// The limit's own reset plus [`MARGIN`]: the recorded reset passing,
    /// which is a limit event.
    ResetMargin,
    /// The earliest end of a limit block recorded on another login the step
    /// has run on, plus [`MARGIN`] (issue #2967): that login recovering is a
    /// limit event as much as the failed login's own reset is. Set by
    /// [`earliest`] alone.
    EarliestReset,
    /// No deadline: the limit stated no usable reset, so the wait ends only
    /// on a login with room or a login change (see the module docs). Written
    /// `bound=none`, beside `until=none`.
    Unbounded,
}

impl Bound {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Bound::ResetMargin => "reset+margin",
            Bound::EarliestReset => "earliest_reset+margin",
            Bound::Unbounded => "none",
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
    /// When the wait ends if no event ends it first: the reset plus the
    /// margin. `None` for a wait whose limit stated no usable reset, which
    /// ends only on an event (see the module docs).
    pub(crate) until: Option<i64>,
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
            until: Some(until),
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
/// A reset wait has no timed bound. The router builds one only from a reset
/// strictly ahead of the clock and within five hours, so its reset is the
/// limit event it waits for: the wait ends there, or sooner on a login with
/// room or a login change.
///
/// There is no five-hour ceiling here either. The five-hour rule decides
/// which wait a limit earns, and the router applies it once: a reset beyond
/// it becomes a capacity wait ([`plan_capacity`]), which still ends at that
/// reset.
///
/// The row is floored at `now + MARGIN`, so the deadline is after `now`
/// even for a reset no caller should have passed in.
pub(crate) fn plan(now: i64, reset: i64) -> Plan {
    Plan {
        until: Some(reset.max(now).saturating_add(MARGIN)),
        bound: Bound::ResetMargin,
        kind: PlanKind::Reset,
        reset: Some(reset),
        login_change: false,
    }
}

/// The deadline for a capacity wait starting at `now`.
///
/// A `reset` still ahead of `now` is the deadline, plus the margin, however
/// far away it is: the failed login has recovered there, and nothing else
/// on a timer runs the step before it. A weekly limit days away is waited
/// for once. The router has already held the reset to its kind's window
/// ([`held`]).
///
/// With no reset ahead (none stated, credits, a reset already passed, a
/// bare 429) there is no deadline: the wait ends on a login with room or a
/// login change, and on nothing timed (issue #2911).
pub(crate) fn plan_capacity(now: i64, reset: Option<i64>) -> Plan {
    let (until, bound) = match reset.filter(|r| *r > now) {
        Some(r) => (Some(r.saturating_add(MARGIN)), Bound::ResetMargin),
        None => (None, Bound::Unbounded),
    };
    Plan {
        until,
        bound,
        kind: PlanKind::Capacity,
        reset,
        login_change: false,
    }
}

/// The plan for a login-change wait: no deadline. The limit stated no
/// reset, and the login changing is the event this wait is for, so it ends
/// on that and on nothing timed (issue #2911).
///
/// The plan's kind is [`PlanKind::Reset`] on purpose: the kind says what
/// ending the wait means, and a login change retries the step under the
/// login now live, as a reset wait's end does. What this wait does *not*
/// consult is carried by [`Plan::capacity_may_end`] instead: no reading
/// says whether the next login's organisation has credit, and the rotator
/// picks what goes into `~/.claude` in any case.
pub(crate) fn plan_login_change() -> Plan {
    Plan {
        until: None,
        bound: Bound::Unbounded,
        kind: PlanKind::Reset,
        reset: None,
        login_change: true,
    }
}

/// `p`, ending at `other` plus [`MARGIN`] when that comes before `p` would
/// end (issue #2967).
///
/// On 21 November 2026 a step waited two hours for its own login's reset
/// while other stored logins had room or were about to. Another login's
/// window resetting writes no file this wait watches, so no file event ends
/// the wait there. This adds no timer: it only moves the one deadline a
/// wait already blocks on earlier, and never gives a deadline to a wait that
/// `p` says may not have one for its own reasons.
///
/// `other` is the earliest end, on the wait's clock, of a limit block the
/// rotate ledger records on another login the step has run on
/// (`auto_switch::rerun::earliest_block`). It is held to the weekly window
/// ([`held`]), as any reset not read from the message is. It counts when it
/// is strictly after `now` and its deadline comes strictly before `p.until`,
/// or `p` has none. Then `bound=` reads `earliest_reset+margin`; the plan's
/// kind and its `reset=`, the limit's own reset, are kept.
///
/// A login-change wait is returned as it was: it waits for one
/// organisation's spend limit to be replaced, and no other login's window
/// restores an organisation's credit (issue #2731). The comparison never
/// subtracts, so no input overflows.
pub(crate) fn earliest(p: Plan, other: Option<i64>, now: i64) -> Plan {
    if p.is_login_change() {
        return p;
    }
    let Some(other) = other.filter(|o| *o > now).map(|o| held(o, now, None)) else {
        return p;
    };
    let until = other.saturating_add(MARGIN);
    if p.until.is_some_and(|own| own <= until) {
        return p;
    }
    Plan {
        until: Some(until),
        bound: Bound::EarliestReset,
        ..p
    }
}
