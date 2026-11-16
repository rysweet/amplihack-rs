//! How many times one step may wait, and for how long in total (issue
//! #2731).
//!
//! Before #2731 a step waited at most once. On a host where every step
//! shares one `~/.claude` login, that meant a step whose login the rotator
//! had already moved on still slept to the old account's reset and then gave
//! up. A step may now wait again, under two rules this module holds:
//!
//! - **every wait records a mark**, the [login key] of the attempt that
//!   failed, and a wait whose mark is already recorded is refused. A wait
//!   that ended before it began records its mark like any other. The marks
//!   bound the chain: the rotator can only move logins the store holds, so a
//!   login that comes round twice ends the step.
//! - `max_wait_secs` is a **per-step** budget. Each wait is planned with what
//!   the earlier ones left, and a second or later wait is refused once it is
//!   spent. The first wait of a step is never refused for it, so
//!   `max_wait_secs = 0` keeps the meaning it has always had.
//!
//! [login key]: super::fingerprint::Watch::login
//!
//! Tested in `tests_marks.rs`.

/// One step's record of what it has already waited for.
pub(crate) struct Marks {
    /// The login key of each wait this step has made, in order. `None` is
    /// the single mark for "no login name could be read", so a step with no
    /// profile store waits exactly once, as it always did.
    waited: Vec<Option<String>>,
    /// Seconds this step has spent waiting, on the wait's clock.
    spent: u64,
}

impl Marks {
    pub(crate) fn new() -> Self {
        Self {
            waited: Vec::new(),
            spent: 0,
        }
    }

    /// Whether this step has waited at all. `auto_switch::rerun` reads it
    /// through `Waiter::used` to stop choosing another candidate once the
    /// step has waited, which #2731 leaves unchanged.
    pub(crate) fn used(&self) -> bool {
        !self.waited.is_empty()
    }

    /// What is left of `max` for this step; `None` when no maximum is set.
    pub(crate) fn remaining(&self, max: Option<u64>) -> Option<u64> {
        max.map(|m| m.saturating_sub(self.spent))
    }

    /// Whether a wait marked `mark` is refused: this step has already waited
    /// on that login, or it has waited before and its budget is spent.
    pub(crate) fn refuses(&self, mark: &Option<String>, max: Option<u64>) -> bool {
        self.waited.contains(mark) || (self.used() && self.remaining(max) == Some(0))
    }

    /// Records a wait on `mark`, whether or not it slept.
    pub(crate) fn record(&mut self, mark: Option<String>) {
        self.waited.push(mark);
    }

    /// Adds `secs` to what this step has spent waiting.
    pub(crate) fn spend(&mut self, secs: u64) {
        self.spent = self.spent.saturating_add(secs);
    }
}
