//! Whether a usage limit can be waited out, and the wait's deadline (issues
//! #1618, #2022). A wait needs the harness's own limit message with a reset
//! that parses; a reset at any distance is waited for, because no default
//! time limit applies to work (issue #2022). An unreadable reset is skipped
//! unless another login could take over: [`upgrade`] then makes it a capacity
//! wait (issue #1801), and does the same for a reset further away than
//! [`FAR_RESET_THRESHOLD`]. The deadline is the earliest row of the deadline
//! table that applies, a tie named by the earlier row. All arithmetic
//! saturates. A reset wait ends no later than its reset plus the margin; a
//! capacity wait also ends at the failed login's reset plus the margin, and
//! with no reset, no `recipe.usage_limit_max_wait_secs` and no step timeout it
//! has no deadline at all (`Bound::Unbounded`). Either way a stop signal, the
//! end of the step's session, a login change or capacity elsewhere ends the
//! wait sooner. To set a ceiling, set `recipe.usage_limit_max_wait_secs` or
//! `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS`.

use crate::commands::recipe::run::limit_patterns::{Harness, LimitMessage};

/// Added to a reset still in the future.
pub(crate) const MARGIN: i64 = 120;
/// A reset further away than this may become a capacity wait when another
/// login could take over (see [`upgrade`]). It only chooses the kind of wait;
/// it never limits how long a wait lasts (issue #2022).
pub(crate) const FAR_RESET_THRESHOLD: i64 = 5 * 3600;
/// The wait ends this long before the step timeout.
pub(crate) const STEP_TIMEOUT_LEAD: i64 = 60;

/// Why a usage limit is not waited out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoWait {
    /// The output has no line in the harness's own limit-message shape.
    NoLimitMessage,
    /// An organisation spend limit that states no reset: none lifts it.
    SpendLimit,
    /// The message states no reset that parses.
    NoReset,
    /// The prompt came on a pipe, which a rerun cannot read again.
    PipedPrompt,
}

impl NoWait {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            NoWait::NoLimitMessage => "no_limit_message",
            NoWait::SpendLimit => "spend_limit",
            NoWait::NoReset => "no_reset",
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

/// What the attempt's parsed limit message allows. Any stated reset
/// is waited for, however far away (issue #2022).
pub(crate) fn wait_for_message(message: Option<LimitMessage>) -> WaitFor {
    let Some(m) = message else {
        return WaitFor::Skip(NoWait::NoLimitMessage);
    };
    // A spend-limit line that also says when the session or weekly limit
    // resets is waited out like any other: that limit is what stops work.
    match m.reset {
        None if m.spend => WaitFor::Skip(NoWait::SpendLimit),
        None => WaitFor::Skip(NoWait::NoReset),
        Some(reset) => WaitFor::Reset {
            harness: m.harness,
            reset,
        },
    }
}

/// `w`, made a capacity wait when it is an unreadable reset, or a reset more
/// than [`FAR_RESET_THRESHOLD`] after `now`, and another login could take
/// over. `seen` is the limit as read; `None` for a spend limit, which another
/// login of the same organisation would not lift, so a spend limit never
/// becomes a capacity wait. `takeover` reads the profile store, so it runs
/// only for a limit that qualifies.
pub(crate) fn upgrade(
    w: WaitFor,
    seen: Option<(Harness, Option<i64>)>,
    now: i64,
    takeover: impl Fn() -> bool,
) -> WaitFor {
    let Some((harness, reset)) = seen else {
        return w;
    };
    let qualifies = match w {
        WaitFor::Skip(NoWait::NoReset) => true,
        WaitFor::Reset { reset: r, .. } => r > now.saturating_add(FAR_RESET_THRESHOLD),
        _ => false,
    };
    if qualifies && takeover() {
        WaitFor::Capacity { harness, reset }
    } else {
        w
    }
}

/// Which row set the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bound {
    ResetMargin,
    MaxWait,
    StepTimeout,
    /// No row applies: a capacity wait with no reset, no maximum and no step
    /// timeout (issue #2022). It has no deadline.
    Unbounded,
}

impl Bound {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Bound::ResetMargin => "reset+margin",
            Bound::MaxWait => "max_wait",
            Bound::StepTimeout => "step_timeout",
            Bound::Unbounded => "unbounded",
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
    /// When the wait ends; `None` only for [`Bound::Unbounded`].
    pub(crate) until: Option<i64>,
    pub(crate) bound: Bound,
    pub(crate) kind: PlanKind,
    /// The reset as read, for the log line; a capacity wait may have none.
    pub(crate) reset: Option<i64>,
}

/// The earliest of the candidate deadlines, in table order; a later row
/// replaces an earlier one only when strictly sooner, so a tie is named by
/// the earlier row.
fn earliest(rows: [Option<(i64, Bound)>; 3]) -> (Option<i64>, Bound) {
    let mut best: Option<(i64, Bound)> = None;
    for (candidate, bound) in rows.into_iter().flatten() {
        if best.is_none_or(|(until, _)| candidate < until) {
            best = Some((candidate, bound));
        }
    }
    match best {
        Some((until, bound)) => (Some(until), bound),
        None => (None, Bound::Unbounded),
    }
}

/// The deadline for a wait starting at `now` for `reset`, at any distance
/// (issue #2022: no five-hour ceiling). `step` is the step's start and its
/// `AMPLIHACK_STEP_TIMEOUT` in seconds; a timeout of 0 does not apply. A
/// reset wait always has a deadline: its reset plus the margin, or now for a
/// reset already past.
pub(crate) fn plan(now: i64, reset: i64, max: Option<u64>, step: Option<(i64, u64)>) -> Plan {
    let reset_end = if reset <= now {
        now
    } else {
        reset.saturating_add(MARGIN)
    };
    let (until, bound) = earliest([
        Some((reset_end, Bound::ResetMargin)),
        max.map(|max| (now.saturating_add(secs(max)), Bound::MaxWait)),
        step_deadline(now, step).map(|end| (end, Bound::StepTimeout)),
    ]);
    Plan {
        until,
        bound,
        kind: PlanKind::Reset,
        reset: Some(reset),
    }
}

/// When `step` requires the wait to end, never before `now`. A timeout of 0
/// does not apply, nor does one whose end lies past the last representable
/// second.
fn step_deadline(now: i64, step: Option<(i64, u64)>) -> Option<i64> {
    let (start, timeout) = step.filter(|(_, t)| *t > 0)?;
    Some(
        start
            .checked_add(secs(timeout))?
            .saturating_sub(STEP_TIMEOUT_LEAD)
            .max(now),
    )
}

/// The deadline for a capacity wait starting at `now`: the earliest of the
/// failed login's own `reset` plus the margin (when still in the future), `max`
/// from now and the step timeout less its lead, never before `now`. A tie
/// between the reset and the maximum is named `ResetMargin`, and one between
/// the maximum and the step timeout `MaxWait`. With none of them there is no
/// deadline (`Bound::Unbounded`, issue #2022). Reaching the reset retries the
/// step, because that login has recovered.
pub(crate) fn plan_capacity(
    now: i64,
    reset: Option<i64>,
    max: Option<u64>,
    step: Option<(i64, u64)>,
) -> Plan {
    let (until, bound) = earliest([
        reset
            .filter(|r| *r > now)
            .map(|r| (r.saturating_add(MARGIN), Bound::ResetMargin)),
        max.map(|max| (now.saturating_add(secs(max)), Bound::MaxWait)),
        step_deadline(now, step).map(|end| (end, Bound::StepTimeout)),
    ]);
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
