//! What a usage limit allows, and the wait's deadline (issues #1618, #2911).
//!
//! A limit never ends a step. A reset the step can use within five hours is
//! waited for directly. Every other limit becomes a wait for *capacity* (a
//! login with room, the rotator's next cycle, or the login in
//! `$HOME/.claude` changing) and says why it has no reset to end on
//! ([`Waiting`]). Only two readings are left that no wait can help, and
//! neither is a limit: output with no limit message in it at all, and a
//! prompt that arrived on a pipe and cannot be replayed ([`NoWait`]).
//!
//! The deadline is the earliest row of the deadline table that applies, a
//! tie named by the earlier row. All arithmetic saturates. A reset wait's
//! deadline lies in `now + 120 s ..= now + 5 h + 120 s`; a capacity or
//! login-change wait has no five-hour ceiling and ends at `max` from now, or
//! at the failed login's own reset plus the margin when that is sooner.
//!
//! **Every plan a limit produces ends strictly after `now`**, unless the
//! step's own timeout forces otherwise. That is the whole of the pacing
//! rule, and it replaces the per-step budget #2731 used: a chain of limits
//! is paced by each wait costing time, not by a counter that eventually
//! refuses one. The invariant holds because a reset row is only built from a
//! reset in the future, and because [`MIN_WAIT_SECS`] floors the configured
//! wait length where it is read (`settings`), so no `max` row can land on
//! `now` either.

use crate::commands::recipe::run::limit_patterns::{Harness, LimitKind, LimitMessage};

/// Added to a reset still in the future.
pub(crate) const MARGIN: i64 = 120;
/// A reset further away than this is not waited for directly.
pub(crate) const FIVE_HOURS: i64 = 5 * 3600;
/// The wait ends this long before the step timeout.
pub(crate) const STEP_TIMEOUT_LEAD: i64 = 60;
/// A capacity wait's length when nothing sets one: the schema's default.
pub(crate) const CAPACITY_DEFAULT_MAX: u64 = 21_600;
/// The shortest wait a configured length may ask for: two poll intervals of
/// the wait loop (issue #2911).
///
/// The loop takes its fingerprint and its capacity readings every 30
/// seconds, so a wait shorter than one interval has no wake condition that
/// could end it and would simply return. A chain of those is the busy loop
/// the pacing rule exists to prevent, wearing different clothes. It is a
/// floor on a length a person configured, not a timer: nothing here sleeps
/// or counts, and the wait loop is the one that already existed.
pub(crate) const MIN_WAIT_SECS: u64 = 60;

/// Why no wait can help, whatever the output said.
///
/// Both are readings of the *step*, not of a limit: neither says anything
/// about an account's windows, so no amount of waiting changes either. Every
/// limit this module can name waits (issue #2911).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoWait {
    /// The output has no line in any harness's own limit-message shape.
    NoLimitMessage,
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
            NoWait::PipedPrompt => "piped_prompt",
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
/// [`wait_for_message`] has kept within five hours and strictly ahead of the
/// clock. `step` is the step's start and its `AMPLIHACK_STEP_TIMEOUT` in
/// seconds; a timeout of 0 does not apply.
///
/// The reset row is floored at `now + MARGIN`, so the deadline is after
/// `now` even for a reset no caller should have passed in; only the step
/// timeout may bring it back to `now`.
pub(crate) fn plan(now: i64, reset: i64, max: Option<u64>, step: Option<(i64, u64)>) -> Plan {
    let ceiling = now.saturating_add(FIVE_HOURS);
    let mut until = reset
        .min(ceiling)
        .saturating_add(MARGIN)
        .max(now.saturating_add(MARGIN));
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
///
/// `max` has already been floored at [`MIN_WAIT_SECS`] where it was read, and
/// the reset row is only applied for a reset still ahead of `now`, so the
/// deadline is after `now` unless the step timeout brings it back.
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
/// the deadline means, and reaching this one retries the step, as reaching a
/// reset wait's own deadline does. What this wait does *not* consult is
/// carried by [`Plan::capacity_may_end`] instead: no reading says whether
/// the next login's organisation has credit, and the rotator picks what goes
/// into `~/.claude` in any case.
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
