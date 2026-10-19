//! The wait's deadline (issue #1618): the earliest row of the deadline
//! table that applies, a tie named by the earlier row. All arithmetic
//! saturates, and the result always lies in `now ..= now + 5 h + 120 s`.

/// Added to a reset still in the future.
pub(crate) const MARGIN: i64 = 120;
/// The wait when no reset was read.
pub(crate) const DEFAULT_WAIT: i64 = 30 * 60;
/// A reset is never read as later than this from now.
pub(crate) const FIVE_HOURS: i64 = 5 * 3600;
/// The wait ends this long before the step timeout.
pub(crate) const STEP_TIMEOUT_LEAD: i64 = 60;

/// Which row set the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bound {
    ResetMargin,
    Default,
    MaxWait,
    StepTimeout,
}

impl Bound {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Bound::ResetMargin => "reset+margin",
            Bound::Default => "default",
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
    pub(crate) reset: Option<i64>,
}

/// The deadline for a wait starting at `now`. `step` is the step's start and
/// its `AMPLIHACK_STEP_TIMEOUT` in seconds; a timeout of 0 does not apply.
pub(crate) fn plan(
    now: i64,
    reset: Option<i64>,
    max: Option<u64>,
    step: Option<(i64, u64)>,
) -> Plan {
    let ceiling = now.saturating_add(FIVE_HOURS);
    let (mut until, mut bound) = match reset {
        Some(r) if r <= now => (now, Bound::ResetMargin),
        Some(r) => (r.min(ceiling).saturating_add(MARGIN), Bound::ResetMargin),
        None => (now.saturating_add(DEFAULT_WAIT), Bound::Default),
    };
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
