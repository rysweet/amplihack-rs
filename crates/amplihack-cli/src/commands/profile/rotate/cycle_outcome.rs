//! A rotate cycle's mode and outcome. Kept apart from `cycle.rs` to hold
//! that file within its line budget.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    OneShot,
    DryRun,
    Watch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleOutcome {
    Swapped,
    Stayed,
    /// Issue #1907: a stay without a fresh reading of the live login below
    /// 100% (a failed, carried or unknown probe). Not capacity for the
    /// limit-resume hook, which reads any outcome it does not know as none.
    StayedUnread,
    Alerted,
    Skipped,
    /// A `profile switch` pin stopped the cycle before measuring (#1691).
    Held,
    /// `watch` received a signal and abandoned the cycle.
    Stopped,
}

impl CycleOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Swapped => "swapped",
            Self::Stayed => "stayed",
            Self::StayedUnread => "stayed-unread",
            Self::Alerted => "alerted",
            Self::Skipped => "skipped",
            Self::Held => "held",
            Self::Stopped => "stopped",
        }
    }
}

/// Whether `last` was written after a cycle began measuring at
/// `measured_at`, so another writer used the ledger while that cycle held no
/// lock. A switch record always counts, whatever the clock says.
pub(super) fn newer_than(last: Option<&super::super::ledger::LastCycle>, measured_at: i64) -> bool {
    last.is_some_and(|c| c.outcome == super::super::ledger::SWITCHED || c.at >= measured_at)
}
