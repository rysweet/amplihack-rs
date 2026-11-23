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
//! The readings and blocks read here follow the rotator's rules for the same
//! data (issue #3105, #3111), so a wait is never timed by, and a retry never
//! moves on, what the rotator would refuse:
//!
//! - A reading older than [`MAX_READING_AGE_SECS`], one session window, is
//!   not read. That is the oldest stored reading the rotator ranks a login on
//!   (`rotate::decide::LAST_GOOD_MAX_AGE_SECS`).
//! - A reading older than one already held is never read: the readings file
//!   keeps only the newest reading of each login (`usage::merge`).
//! - A login with a standing block in the rotate ledger does not come back
//!   before the block's recorded end, whatever its reading says, by the
//!   rotator's own test (`rotate::decide::standing_block`).
//! - The retry after an earliest-reset wait does not move onto a login that
//!   anything recorded while the step waited still holds out
//!   ([`held_since`]).
//!
//! Nothing here reads a credential, makes a request or writes a file.

use super::candidate;
use super::fsx;
use super::rerun::{self, Choice, Reads};
use super::usage::Reading;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::{self, ledger::OverflowBlock};
use std::collections::BTreeMap;
use std::path::Path;

/// The oldest stored reading this module reads, in seconds: one session
/// window, the rotator's own bound on a stored reading it ranks a login on.
/// A reading older than that has seen its session window reset at least
/// once since it was taken, so it says nothing about that window now, and a
/// window it showed under the threshold may have filled since.
pub(super) const MAX_READING_AGE_SECS: i64 = rotate::decide::LAST_GOOD_MAX_AGE_SECS;

/// Whether `r` was taken no more than [`MAX_READING_AGE_SECS`] before `now`,
/// and not after it: the age rule `fresh_reading` applies to a stored
/// reading (issue #1844).
fn recent(r: &Reading, now: i64) -> bool {
    now.checked_sub(r.observed_at)
        .is_some_and(|age| (0..=MAX_READING_AGE_SECS).contains(&age))
}

/// The limit blocks the rotate ledger records, which the rotator holds a
/// login out by until their end (`rotate::decide::standing_block`). A host with no
/// rotator has none. A ledger that cannot be read holds no login out, as it
/// holds none out of the candidate rule (`OverflowInputs::load`), and the
/// step says so on stderr ([`blocks_unread_line`]).
fn recorded_blocks(root: &Path) -> BTreeMap<String, OverflowBlock> {
    match rotate::ledger::RotateState::new(root).load_ledger() {
        Ok(l) => l.map(|l| l.overflow_blocked).unwrap_or_default(),
        Err(e) => {
            eprintln!("{}", blocks_unread_line(&e.to_string()));
            BTreeMap::new()
        }
    }
}

/// The stderr line for a rotate ledger that cannot be read, with its error.
pub(super) fn blocks_unread_line(error: &str) -> String {
    format!(
        "amplihack: the rotate ledger cannot be read: {error}; no block it records on another \
         login is known to this step's wait (auto-switch)"
    )
}

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
/// a target ([`is_target`]), is not in `tried`, has a reading no older than
/// [`MAX_READING_AGE_SECS`] and is exhausted by the candidate rule, the
/// instant [`candidate::regains_room`] gives, or the end of a standing block
/// on it when that is later. The readings and blocks are stamped on the real
/// clock and the wait may run on its own, so each instant is moved over by
/// its distance from now, saturating.
///
/// Only instants strictly after `wait_now` are returned. A login under the
/// threshold, a login with no reading, an older one or no reset ahead, and a
/// login barred by another rule contribute nothing: the first two are the
/// candidate rule's to take, and the wait's own probe picks one up when its
/// bar lifts, on a reading taken during the wait (`takeover::is_fresh`).
///
/// A stale reading could only time the wait too early, never too late: the
/// usage of a window over the threshold does not fall before its reset, but
/// a window under it may have filled since. The age rule bounds that, and the
/// retry checks again ([`held_since`]).
///
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
        let blocks = recorded_blocks(&choice.reads.state.root);
        choice
            .pool
            .profiles
            .iter()
            .filter(|p| is_target(choice, p))
            .filter_map(|p| {
                let reading = choice.pool.readings.get(p.as_str());
                let reading = reading.filter(|r| recent(r, real_now))?;
                let regains = candidate::regains_room(reading, threshold, real_now)?;
                // Issue #3105: a standing block holds the login out until
                // its recorded end, whatever its reading shows.
                let back = rotate::decide::standing_block(&blocks, p, real_now)
                    .map_or(regains, |(until, _)| regains.max(until));
                let at = wait_now.saturating_add(back.saturating_sub(real_now));
                (at > wait_now).then(|| (p.clone(), at))
            })
            .collect()
    })
}

/// Whether the step may be run on `login` by the checks that do not depend
/// on a reading: it is a stored plan login, not excluded, not leased or
/// hosting another account, and holds a login of its own. Asked for the
/// retry after an earliest-reset wait, which must not land on a login that
/// was excluded, leased or removed while the step waited.
pub(super) fn may_run_on(reads: Reads<'_>, current: &str, login: &ProfileName) -> bool {
    rerun::with_choice(reads, current, &[], |choice| {
        choice.pool.profiles.contains(login) && is_target(choice, login)
    })
}

/// Whether something recorded about `login` after `since`, the real time the
/// wait's deadline was set, still holds it out now: a block in the rotate
/// ledger that still stands, or a reading that has it over the auto-switch
/// threshold (issue #3105). Asked for the retry after an earliest-reset
/// wait.
///
/// What was known at `since` already set the deadline, which is the moment
/// it stops holding the login out, so only what is newer can say otherwise.
/// The readings file keeps the newest reading of each login, so a reading
/// taken after `since` is newer than the one the deadline was set from.
///
/// No reading taken after the reset is required. The step has no reader of
/// its own, and on a host whose readings come only from the rotator or
/// `amplihack profile usage` one may never come, so requiring it would undo
/// the move #2967 is for. A rotator that does read the login during the
/// wait ends the wait early through the wait's own probe, which needs such a
/// reading. A retry that meets a limit anyway records it, adds the login to
/// the step's tried set and goes back to the move-or-wait loop.
pub(super) fn held_since(reads: Reads<'_>, login: &ProfileName, since: i64) -> bool {
    let now = fsx::now();
    let mut blocks = recorded_blocks(&reads.state.root);
    blocks.retain(|_, b| b.at > since);
    rotate::decide::standing_block(&blocks, login, now).is_some()
        || reads
            .readings
            .get(login.as_str())
            .filter(|r| r.observed_at > since)
            .is_some_and(|r| candidate::is_exhausted(Some(r), reads.cfg.threshold, now))
}
