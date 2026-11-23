//! The logins whose resets bound a step's wait (issue #2967).
//!
//! On 21 November 2026 a step waited two hours for its own login's reset
//! while other stored logins could have run it or were about to. When a
//! step meets a limit it first moves to another login by the candidate rule
//! (`rerun::candidate`, the auto-switch threshold, 90% by default), whether
//! or not it has waited before. Only when no login passes that rule does it
//! wait, and [`cross_login_reset`] then answers the one question left: when
//! does each login the step could still move to come back under that same
//! threshold?
//!
//! There is one rule for "this login can run the step": the candidate rule's
//! threshold, read from the auto-switch configuration. This module adds no
//! second bound; [`candidate::regains_room`] is the moment
//! [`candidate::is_exhausted`] stops holding for a reading.
//!
//! Both functions run under the eligibility `rerun::with_choice` sets up for
//! the candidate rule (not excluded, not leased, not hosting another account,
//! holding a login of its own, not in the caller's skip list), so neither can
//! admit a login the candidate rule refuses for another reason. Neither ever
//! takes an overflow or API-key profile, so a wait that ends here never
//! spends money.
//!
//! Nothing here reads a credential, makes a request or writes a file.

use super::candidate;
use super::rerun::{self, Choice, Reads};
use crate::commands::profile::name::ProfileName;

/// Whether `p` is a plan login the step may move to: in the store, not the
/// login that failed, not excluded, neither overflow nor an API key, and
/// passing the shared eligibility (not skipped, leased or hosting, and
/// ready).
fn is_target(choice: &Choice<'_, '_>, p: &ProfileName) -> bool {
    let ov = choice.overflow.pool();
    !ov.kinds_unknown
        && p.as_str() != choice.current
        && p.as_str() != choice.source_host
        && !choice.pool.exclude.contains(p)
        && ov.is_plan(p)
        && (choice.eligible)(p)
}

/// When each login the step could still move to comes back under the
/// auto-switch threshold, on the wait's clock: for every plan login that is
/// a target ([`is_target`]), is not in `tried`, has a reading and is
/// exhausted by the candidate rule, the instant [`candidate::regains_room`]
/// gives. The readings are stamped on the real clock and the wait may run on
/// its own, so each instant is moved over by its distance from now,
/// saturating.
///
/// Only instants strictly after `wait_now` are returned. A login under the
/// threshold, a login with no reading or no reset ahead, and a login barred
/// by another rule contribute nothing: the first two are the candidate
/// rule's to take, and the wait's own probe picks one up when its bar lifts.
/// Whether an instant moves a wait's deadline, and the hold to the weekly
/// window, are the plan's (`limit_wait::plan::earliest`), not this
/// function's.
///
/// Organisation affinity removes nothing here. It narrows a choice among
/// logins that have capacity at the same moment, and a login that is the
/// first to reset has, at that moment, no rival of the failed login's own
/// organisation with capacity, which is the case in which the policy permits
/// a move across organisations.
pub(super) fn cross_login_reset(
    reads: Reads<'_>,
    current: &str,
    tried: &[String],
    wait_now: i64,
) -> Vec<(ProfileName, i64)> {
    rerun::with_choice(reads, current, tried, |choice| {
        let (real_now, threshold) = (choice.pool.now, choice.pool.threshold);
        choice
            .pool
            .profiles
            .iter()
            .filter(|p| is_target(choice, p))
            .filter_map(|p| {
                let reading = choice.pool.readings.get(p.as_str())?;
                let regains = candidate::regains_room(reading, threshold, real_now)?;
                let at = wait_now.saturating_add(regains.saturating_sub(real_now));
                (at > wait_now).then(|| (p.clone(), at))
            })
            .collect()
    })
}

/// Whether the step may be run on `login` by the checks that do not depend
/// on a reading: it is a stored plan login, not excluded, not leased or
/// hosting another account, and holds a login of its own. Asked for the
/// retry after an earliest-reset wait, which needs no fresh reading because
/// the reset is what the wait was for, but must not land on a login that was
/// excluded, leased or removed while the step waited.
pub(super) fn may_run_on(reads: Reads<'_>, current: &str, login: &ProfileName) -> bool {
    rerun::with_choice(reads, current, &[], |choice| {
        choice.pool.profiles.contains(login) && is_target(choice, login)
    })
}
