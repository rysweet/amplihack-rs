//! Which logins one step has waited on (issues #2731, #2911).
//!
//! Before #2731 a step waited at most once. On a host where every step
//! shares one `~/.claude` login, that meant a step whose login the rotator
//! had already moved on still slept to the old account's reset and then gave
//! up, so a step may now wait again.
//!
//! #2731 bounded the chain with two refusals: a login already waited on, and
//! a spent `max_wait_secs` budget. Both ended runs that a limit should only
//! have delayed, so both are gone (#2911). What bounds the chain now is that
//! every wait costs time: a limit's plan always ends strictly after `now`
//! (see `plan`), so a step meeting limit after limit waits, and is woken by
//! a login gaining room or by a reset passing, rather than counting its
//! attempts towards an ending.
//!
//! What is left is the record itself. One reader uses it: the `reentered`
//! line, which says whether this step has waited on this login before.
//!
//! Issue #2967 removed the other. `auto_switch::rerun` used to read
//! `Waiter::used` to stop choosing another login once the step had waited,
//! so a limit on the retry a wait earned left the step waiting again while
//! other logins had room. A move to a login that can run the step is not
//! a wait, so a step that has waited may still make one; the moves end
//! because the rerun loop's `tried` list grows with every login it runs on.
//!
//! A step under a stored profile is marked with that profile's name (issue
//! #2967), so its wait lines say `account=<profile>` rather than
//! `account=unknown`.
//!
//! Tested in `tests_marks.rs`.

/// One step's record of which logins it has already waited on.
pub(crate) struct Marks {
    /// The login key of each wait this step has made, in order. `None` is
    /// the single mark for "no login name could be read", so a step with no
    /// profile store records one mark for every wait it makes.
    waited: Vec<Option<String>>,
}

impl Marks {
    pub(crate) fn new() -> Self {
        Self { waited: Vec::new() }
    }

    /// Whether this step has already waited on `mark`. It refuses nothing:
    /// it is what the `reentered` line reports, so a reader can tell a
    /// second wait on the same login from a first wait on a new one.
    pub(crate) fn waited_on(&self, mark: &Option<String>) -> bool {
        self.waited.contains(mark)
    }

    /// Records a wait on `mark`, whether or not it slept.
    pub(crate) fn record(&mut self, mark: Option<String>) {
        self.waited.push(mark);
    }
}
