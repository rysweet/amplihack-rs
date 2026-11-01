//! Whether a usage limit can be waited out, and the wait's deadline (issue
//! #1618). A reset wait needs the harness's own limit message with a reset
//! at most five hours away. A far or unreadable reset is skipped, because no
//! amount of waiting is known to help, unless another login could take over:
//! [`upgrade`] then makes it a capacity wait (issue #1801). The deadline is
//! the earliest row of the deadline table that applies, a tie named by the
//! earlier row. All arithmetic saturates. A reset wait's deadline always lies
//! in `now ..= now + 5 h + 120 s`; a capacity wait has no five-hour ceiling.

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
    /// The prompt came on a pipe, which a rerun cannot read again.
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

/// The deadline for a capacity wait starting at `now`: `max` from now or the
/// step timeout less its lead, never before `now`, a tie named `MaxWait`.
/// There is no five-hour ceiling, and `reset` plays no part in it.
pub(crate) fn plan_capacity(
    now: i64,
    reset: Option<i64>,
    max: Option<u64>,
    step: Option<(i64, u64)>,
) -> Plan {
    let max_end = now.saturating_add(secs(max.unwrap_or(CAPACITY_DEFAULT_MAX)));
    let (until, bound) = match step_deadline(now, step) {
        Some(end) if end < max_end => (end, Bound::StepTimeout),
        _ => (max_end, Bound::MaxWait),
    };
    Plan {
        until,
        bound,
        kind: PlanKind::Capacity,
        reset,
    }
}

fn secs(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}
