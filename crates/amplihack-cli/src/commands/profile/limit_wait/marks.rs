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

    /// Whether the one wait a *contradicted signal* earns is refused (issue
    /// #2857).
    ///
    /// The retry a wait earned met a limit at once, so what ended that wait
    /// was wrong about the login. Waiting again on that same login is the
    /// right answer, and the same-login rule is the one thing lifted here:
    /// it exists to stop a step sleeping twice for the same reason, and this
    /// is a different reason. This takes no mark for that reason.
    ///
    /// The budget rule is not lifted, and that is what bounds the chain. An
    /// unset `max_wait_secs` is not an unlimited one: the step's budget is
    /// then the length a capacity wait is planned with when nothing sets one
    /// ([`plan::CAPACITY_DEFAULT_MAX`]), so a login that keeps claiming room
    /// and keeps rejecting the retry runs out of budget rather than running
    /// for ever. No count and no timer is added.
    ///
    /// The budget bounds a chain of waits that sleep. A wait that ends the
    /// moment it starts spends none of it, so the budget alone does not bound
    /// a chain of those. A re-entry for a reset that has already passed is
    /// therefore not left as one: it is planned from capacity instead
    /// ([`plan::upgrade_passed_reset`]), which gives it the step's remaining
    /// budget as its deadline and so brings it back under this rule. That
    /// holds whether or not another login could take over: with no rotator
    /// and no second login nothing ends the wait early, so it spends the
    /// budget out and the retry follows. Such a chain therefore ends.
    ///
    /// A reset plan with a deadline of now can still reach the caller from
    /// three other sources. They are listed in a comment inside the body of
    /// [`Waiter::after_limit`], above its [`Plan::ends_at_once`] check, which
    /// rustdoc does not render, so read it in the source. For those sources
    /// that check ends the step as a backstop.
    ///
    /// [`Plan::ends_at_once`]: super::plan::Plan::ends_at_once
    /// [`Waiter::after_limit`]: super::Waiter::after_limit
    /// [`plan::upgrade_passed_reset`]: super::plan::upgrade_passed_reset
    /// [`plan::CAPACITY_DEFAULT_MAX`]: super::plan::CAPACITY_DEFAULT_MAX
    pub(crate) fn refuses_reentry(&self, max: Option<u64>) -> bool {
        self.spent >= max.unwrap_or(super::plan::CAPACITY_DEFAULT_MAX)
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
