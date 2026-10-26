//! Whether a usage limit can be waited out, and the wait's deadline (issue
//! #1618). A wait needs the harness's own limit message with a reset at most
//! five hours away; anything else is skipped, because no amount of waiting
//! is known to help. The deadline is the earliest row of the deadline table
//! that applies, a tie named by the earlier row. All arithmetic saturates,
//! and the result always lies in `now ..= now + 5 h + 120 s`.

use crate::commands::recipe::run::limit_patterns::{self, Harness};

/// Added to a reset still in the future.
pub(crate) const MARGIN: i64 = 120;
/// A reset further away than this is not waited for.
pub(crate) const FIVE_HOURS: i64 = 5 * 3600;
/// The wait ends this long before the step timeout.
pub(crate) const STEP_TIMEOUT_LEAD: i64 = 60;

/// Why a usage limit is not waited out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoWait {
    /// The output has no line in the harness's own limit-message shape.
    NoLimitMessage,
    /// An organisation spend limit, which no reset lifts.
    SpendLimit,
    /// The message states no reset that parses.
    NoReset,
    /// The stated reset is more than five hours away.
    ResetTooFar,
}

impl NoWait {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            NoWait::NoLimitMessage => "no_limit_message",
            NoWait::SpendLimit => "spend_limit",
            NoWait::NoReset => "no_reset",
            NoWait::ResetTooFar => "reset_too_far",
        }
    }
}

/// What a usage-limited attempt allows: a wait for `reset`, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitFor {
    Reset { harness: Harness, reset: i64 },
    Skip(NoWait),
}

/// Reads the attempt's stderr tail and stdout tail at `now`.
pub(crate) fn wait_for(stderr: &[u8], stdout: &[u8], now: i64) -> WaitFor {
    let Some(m) = limit_patterns::limit_message(stderr, stdout, now) else {
        return WaitFor::Skip(NoWait::NoLimitMessage);
    };
    match m.reset {
        _ if m.spend => WaitFor::Skip(NoWait::SpendLimit),
        None => WaitFor::Skip(NoWait::NoReset),
        Some(r) if r > now.saturating_add(FIVE_HOURS) => WaitFor::Skip(NoWait::ResetTooFar),
        Some(reset) => WaitFor::Reset {
            harness: m.harness,
            reset,
        },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) until: i64,
    pub(crate) bound: Bound,
    /// The reset as read, for the log line.
    pub(crate) reset: i64,
}

/// The deadline for a wait starting at `now` for `reset`, which
/// [`wait_for`] has kept within five hours. `step` is the step's start and
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
    if let Some((start, timeout)) = step.filter(|(_, t)| *t > 0) {
        let end = start
            .saturating_add(secs(timeout))
            .saturating_sub(STEP_TIMEOUT_LEAD);
        consider(end.max(now), Bound::StepTimeout);
    }
    Plan {
        until,
        bound,
        reset,
    }
}

fn secs(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}
