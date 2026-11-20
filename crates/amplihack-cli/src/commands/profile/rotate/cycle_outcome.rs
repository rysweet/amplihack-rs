//! A rotate cycle's mode and outcome. Kept apart from `cycle.rs` to hold
//! that file within its line budget.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    OneShot,
    DryRun,
    Watch,
}

impl Mode {
    /// The mode of a plain `profile rotate`, with or without `--dry-run`.
    pub fn once(dry_run: bool) -> Self {
        if dry_run { Self::DryRun } else { Self::OneShot }
    }
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
