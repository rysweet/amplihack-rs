//! Whether a usage limit can be waited out, and the wait's deadline (issue
//! #1618). A reset wait needs the harness's own limit message with a reset
//! at most five hours away. A far or unreadable reset is skipped, because no
//! amount of waiting is known to help, unless another login could take over:
//! [`upgrade`] then makes it a capacity wait (issue #1801), as does a reset
//! already behind the clock met on the retry a wait earned
//! ([`upgrade_passed_reset`], issue #2857). An organisation
//! spend limit on a `$HOME/.claude` step becomes a login-change wait instead
//! ([`upgrade_spend`], issue #2731). The deadline is
//! the earliest row of the deadline table that applies, a tie named by the
//! earlier row. All arithmetic saturates. A reset wait's deadline always lies
//! in `now ..= now + 5 h + 120 s`; a capacity wait has no five-hour ceiling
//! and also ends at the failed login's own reset plus the margin.

use crate::commands::recipe::run::limit_patterns::{Harness, LimitMessage};

/// Added to a reset still in the future.
pub(crate) const MARGIN: i64 = 120;
/// A reset further away than this is not waited for.
pub(crate) const FIVE_HOURS: i64 = 5 * 3600;
/// The wait ends this long before the step timeout.
pub(crate) const STEP_TIMEOUT_LEAD: i64 = 60;
/// A capacity wait's length when nothing sets one: the schema's default.
pub(crate) const CAPACITY_DEFAULT_MAX: u64 = 21_600;

/// Why a usage limit is not waited out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoWait {
    /// The output has no line in the harness's own limit-message shape.
    NoLimitMessage,
    /// An organisation spend limit that states no reset: none lifts it.
    SpendLimit,
    /// The message states no reset that parses.
    NoReset,
    /// The stated reset is more than five hours away.
    ResetTooFar,
    /// The prompt came on a pipe and no copy of it could be kept, so a
    /// rerun cannot read it again (issue #2815). A piped prompt that was
    /// copied is replayed and never reaches this reason; the name is kept
    /// because it is the logged value callers already match on.
    PipedPrompt,
}

impl NoWait {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            NoWait::NoLimitMessage => "no_limit_message",
            NoWait::SpendLimit => "spend_limit",
            NoWait::NoReset => "no_reset",
            NoWait::ResetTooFar => "reset_too_far",
            NoWait::PipedPrompt => "piped_prompt",
        }
    }
}

/// What a usage-limited attempt allows: a wait for `reset`, a wait for
/// another login with capacity (issue #1801), or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitFor {
    Reset {
        harness: Harness,
        reset: i64,
    },
    Capacity {
        harness: Harness,
        reset: Option<i64>,
    },
    /// The login in `$HOME/.claude` changing (issue #2731): an organisation
    /// spend limit that states no reset, on a step the rotator can move.
    LoginChange {
        harness: Harness,
    },
    Skip(NoWait),
}

/// What the attempt's parsed limit message allows at `now`.
pub(crate) fn wait_for_message(message: Option<LimitMessage>, now: i64) -> WaitFor {
    let Some(m) = message else {
        return WaitFor::Skip(NoWait::NoLimitMessage);
    };
    // A spend-limit line that also says when the session or weekly limit
    // resets is waited out like any other: that limit is what stops work.
    match m.reset {
        None if m.spend => WaitFor::Skip(NoWait::SpendLimit),
        None => WaitFor::Skip(NoWait::NoReset),
        Some(r) if r > now.saturating_add(FIVE_HOURS) => WaitFor::Skip(NoWait::ResetTooFar),
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
    if reset > now.saturating_add(FIVE_HOURS) {
        WaitFor::Skip(NoWait::ResetTooFar)
    } else {
        WaitFor::Reset { harness, reset }
    }
}

/// `w`, made a capacity wait when it is a far or unreadable reset and another
/// login could take over. `seen` is the limit as read; `None` for a spend
/// limit, which another login of the same organisation would not lift.
/// `takeover` reads the profile store, so it runs only for a limit that
/// qualifies.
pub(crate) fn upgrade(
    w: WaitFor,
    seen: Option<(Harness, Option<i64>)>,
    takeover: impl Fn() -> bool,
) -> WaitFor {
    match (w, seen) {
        (WaitFor::Skip(NoWait::ResetTooFar | NoWait::NoReset), Some((harness, reset)))
            if takeover() =>
        {
            WaitFor::Capacity { harness, reset }
        }
        _ => w,
    }
}

/// `w`, made a capacity wait when it is a reset the clock has already passed
/// on a re-entry the ordinary one-wait-per-login rule would have refused
/// (issue #2857).
///
/// The retry a finished wait earned met a usage limit stating a reset already
/// behind the clock. [`plan`] gives such a reset a deadline of `now`, which is
/// how the grace a login's *first* limit gets is spelled: retrying at once is
/// the right answer there, because the message may be minutes stale. For a
/// re-entry it is not: the wait would end the moment it began and spend none
/// of the step's budget. A limit after a wait must go back into a wait and
/// must not end the step, so the re-entry is planned as a capacity wait
/// instead. Its deadline is the step's remaining budget, which keeps
/// `max_wait_secs` a bound on the chain, and it ends early only on a capacity
/// reading or a login with room, as every capacity wait does. With no
/// rotator and no second login nothing ends it early, so it runs the budget
/// out and the retry follows.
///
/// The stated reset is carried through for the `start` line, and
/// [`plan_capacity`] ignores one that is already past.
///
/// `reentry_refused` is that narrowing, read from
/// [`Waiter::ordinarily_refused`]: a retry that merely landed on a login this
/// step has never waited on is that login's first wait and keeps its grace.
/// `seen` is the limit as read, and `None` for a spend limit, which no other
/// login of the same organisation would lift (issue #2731): a spend limit that
/// also states a passed session reset must stay the reset wait it is, and the
/// caller refuses it ([`Plan::ends_at_once`]). `now` is the wait's own clock.
/// The reset is compared with `now` and never subtracted from it.
///
/// [`Waiter::ordinarily_refused`]: super::Waiter::ordinarily_refused
pub(crate) fn upgrade_passed_reset(
    w: WaitFor,
    seen: Option<(Harness, Option<i64>)>,
    reentry_refused: bool,
    now: i64,
) -> WaitFor {
    match (w, seen) {
        (WaitFor::Reset { harness, reset }, Some(_)) if reentry_refused && reset <= now => {
            WaitFor::Capacity {
                harness,
                reset: Some(reset),
            }
        }
        _ => w,
    }
}

/// `w`, made a login-change wait when it is an organisation spend limit
/// that states no reset on a `$HOME/.claude` step and a rotator is running
/// (issue #2731). `spend` is the harness of such a limit, and `None` for
/// every other limit and every step whose login the rotator never changes,
/// so the promotion cannot reach one. No reset lifts a spend limit and no
/// login of the same organisation would either, so this waits for the login
/// to change rather than for a reading to show capacity: [`upgrade`]'s
/// capacity wait stays exactly as issue #1801 left it.
///
/// `rotator` reads files, so it runs only for a limit that qualifies. Only
/// the rotator moves a login into `~/.claude`, so with none running there is
/// nothing to wait for and the skip stands.
pub(crate) fn upgrade_spend(
    w: WaitFor,
    spend: Option<Harness>,
    rotator: impl Fn() -> bool,
) -> WaitFor {
    match (w, spend) {
        (WaitFor::Skip(NoWait::SpendLimit), Some(harness)) if rotator() => {
            WaitFor::LoginChange { harness }
        }
        _ => w,
    }
}

/// Which row set the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bound {
    ResetMargin,
    MaxWait,
    StepTimeout,
}

impl Bound {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Bound::ResetMargin => "reset+margin",
            Bound::MaxWait => "max_wait",
            Bound::StepTimeout => "step_timeout",
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

    /// Whether this wait would end the moment it began, because the reset it
    /// waits for has already passed at `now`.
    ///
    /// [`plan`] gives such a wait a deadline of `now`, which is how the grace
    /// a step's first limit on a login gets is spelled: the reset in the
    /// message may be minutes behind the clock the retry reads, and retrying
    /// at once is the right answer. The wait sleeps for nothing, so it spends
    /// none of the step's budget, and a chain of them is bounded by nothing.
    /// The caller asks this where that matters (issue #2857) and nowhere else.
    ///
    /// A capacity wait is never this: it waits for a reading, not for the
    /// reset, and its deadline is its own. Neither is a login-change wait,
    /// whose limit stated no reset at all. That is why
    /// [`upgrade_passed_reset`] answers the same case for a usage limit: the
    /// plan it builds is a capacity wait, so it reaches the caller's refusal
    /// as a wait that will cost something, and the budget binds it. What is
    /// still a reset wait here, and is refused, is a spend limit stating a
    /// passed session reset, and a reset read from the auto-switch readings
    /// ([`from_reading`]).
    pub(crate) fn ends_at_once(&self, now: i64) -> bool {
        self.kind == PlanKind::Reset && self.reset.is_some_and(|r| r <= now)
    }

    /// Whether this is the capacity wait a re-entry on a reset the clock had
    /// already passed was planned as (issue #2857), at `now`.
    ///
    /// Such a wait has no reset to end on, so the budget is its only
    /// deadline. When that deadline is reached the step retries once, as a
    /// reset wait does at its own deadline: the retry is the check the wait
    /// was for, and a limit on it is refused by the budget that is now spent.
    /// Any other capacity wait still ends `capacity expired` at its deadline.
    pub(crate) fn is_passed_reset_capacity(&self, now: i64) -> bool {
        self.kind == PlanKind::Capacity && self.reset.is_some_and(|r| r <= now)
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

/// The deadline for a wait starting at `now` for `reset`, which
/// [`wait_for_message`] has kept within five hours. `step` is the step's start and
/// its `AMPLIHACK_STEP_TIMEOUT` in seconds; a timeout of 0 does not apply.
pub(crate) fn plan(now: i64, reset: i64, max: Option<u64>, step: Option<(i64, u64)>) -> Plan {
    let ceiling = now.saturating_add(FIVE_HOURS);
    let mut until = if reset <= now {
        now
    } else {
        reset.min(ceiling).saturating_add(MARGIN)
    };
    let mut bound = Bound::ResetMargin;
    let mut consider = |candidate: i64, b: Bound| {
        if candidate < until {
            until = candidate;
            bound = b;
        }
    };
    if let Some(max) = max {
        consider(now.saturating_add(secs(max)), Bound::MaxWait);
    }
    if let Some(end) = step_deadline(now, step) {
        consider(end, Bound::StepTimeout);
    }
    Plan {
        until,
        bound,
        kind: PlanKind::Reset,
        reset: Some(reset),
        login_change: false,
    }
}

/// When `step` requires the wait to end, never before `now`; a timeout of 0
/// does not apply.
fn step_deadline(now: i64, step: Option<(i64, u64)>) -> Option<i64> {
    let (start, timeout) = step.filter(|(_, t)| *t > 0)?;
    Some(
        start
            .saturating_add(secs(timeout))
            .saturating_sub(STEP_TIMEOUT_LEAD)
            .max(now),
    )
}

/// The deadline for a capacity wait starting at `now`: the earliest of the
/// failed login's own `reset` plus the margin (when still in the future), `max`
/// from now and the step timeout less its lead, never before `now`. A tie
/// between the reset and the maximum is named `ResetMargin`, and one between
/// the maximum and the step timeout `MaxWait`. There is no five-hour ceiling.
/// Reaching the reset retries the step, because that login has recovered.
pub(crate) fn plan_capacity(
    now: i64,
    reset: Option<i64>,
    max: Option<u64>,
    step: Option<(i64, u64)>,
) -> Plan {
    let mut until = now.saturating_add(secs(max.unwrap_or(CAPACITY_DEFAULT_MAX)));
    let mut bound = Bound::MaxWait;
    if let Some(r) = reset.filter(|r| *r > now) {
        let end = r.saturating_add(MARGIN);
        if end <= until {
            until = end;
            bound = Bound::ResetMargin;
        }
    }
    if let Some(end) = step_deadline(now, step)
        && end < until
    {
        until = end;
        bound = Bound::StepTimeout;
    }
    Plan {
        until,
        bound,
        kind: PlanKind::Capacity,
        reset,
        login_change: false,
    }
}

/// The deadline for a login-change wait starting at `now`: `max` from now,
/// or the step timeout less its lead when that is earlier, never before
/// `now`. A tie is named `MaxWait`. There is no reset row, because the limit
/// stated none, and no five-hour ceiling.
///
/// The plan's kind is [`PlanKind::Reset`] on purpose: the kind says what
/// the deadline means, and reaching this one is `End::MaxReached`, which the
/// caller turns into `failed reason=spend_limit`. What this wait does *not*
/// consult is carried by [`Plan::capacity_may_end`] instead: no reading says
/// whether the next login's organisation has credit, and the rotator picks
/// what goes into `~/.claude` in any case.
pub(crate) fn plan_login_change(now: i64, max: Option<u64>, step: Option<(i64, u64)>) -> Plan {
    let mut until = now.saturating_add(secs(max.unwrap_or(CAPACITY_DEFAULT_MAX)));
    let mut bound = Bound::MaxWait;
    if let Some(end) = step_deadline(now, step)
        && end < until
    {
        until = end;
        bound = Bound::StepTimeout;
    }
    Plan {
        until: until.max(now),
        bound,
        kind: PlanKind::Reset,
        reset: None,
        login_change: true,
    }
}

fn secs(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}
