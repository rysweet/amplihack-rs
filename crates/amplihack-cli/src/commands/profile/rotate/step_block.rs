//! The block a failed agent step records for the login it ran under (issues
//! #2890, #2911).
//!
//! The rotator's `/usage` probe cannot see an individual spend limit, and it
//! learns of a session or weekly limit only on its own cycle, minutes after
//! the step that met one. The step is the first witness either way, so it
//! writes the block itself, into the same `overflow_blocked` map and with
//! the same `spend_limit` reason the rotator already uses. Nothing else
//! changes: `active_blocks` and `prune_blocks` expire it at its end (the
//! stated reset, held to the kind's window), `profile overflow clear`
//! removes it, and every reader that already honours a block honours this
//! one.
//!
//! The reason stays `spend_limit` for every kind on purpose. `valid_overflow`
//! refuses a whole ledger holding a block with any other reason, so a host
//! still running an older binary would lose its rotator on the first cycle
//! after reading a ledger this writer had widened. The *kind* is not lost: it
//! names the limit in the ALERT line and on the step's own stderr.
//!
//! This is data the rotator reads, not a gate. [`note_limit_block`] returns
//! `()` so that no caller can branch on the outcome: whatever happens here,
//! the step goes on to plan its wait exactly as it would have. Every failure
//! is reported, on stderr and in the rotate log, and nothing is stored.
//!
//! The order is fixed, and each step exists for a reason:
//!
//! 0. **The ledger exists.** A step never creates the rotator's state, and a
//!    host with no rotator says nothing at all. It is asked before the reset
//!    is resolved, so an unresolvable reset on such a host is silent too.
//! 1. **The reset resolves** ([`resolved_until`]). A spend limit that states
//!    none falls back to the monthly reset the rotator already assumes; any
//!    other kind with no stated reset writes nothing, because a block with
//!    an invented end would hold a healthy login out of rotation.
//! 2. **The span is valid, and the block is held to its kind's window**
//!    ([`bounded`]). The stated reset must pass [`valid_span`], the loader's
//!    own rule: a block outside it would make the whole ledger unreadable on
//!    the next load, which would stop the rotator's prelude on every cycle.
//!    Then a session block is cut to at most five hours and a weekly block
//!    to at most seven days, however far away the message put the reset.
//!    The step cannot tell the harness's own message from a model that
//!    printed one as its last line, so this is what limits how long a forged
//!    session or weekly message can keep a healthy login out of rotation. A
//!    genuine reset always falls inside its window, so a genuine message is
//!    never shortened. A spend or credit limit has no fixed window and keeps
//!    the span rule alone.
//! 3. **The lock**, waited for at most [`CLEAR_LOCK_WAIT`] and then given
//!    up. A busy rotator can hold a step up for that long and no longer;
//!    there is no retry, so the wait has a ceiling even under contention.
//! 4. **Load**; a ledger that disappeared between 0 and 3 is left alone.
//! 5. **Expired blocks are pruned**, so that a block left for this login is
//!    an active one. **It is then kept as it is**: never extended, never
//!    alerted about twice.
//! 6. **The cardinality cap**, before the insert.
//! 7. **Save**; the lock is released as `store` returns.
//! 8. **Log**, outside the lock.

use super::control::CLEAR_LOCK_WAIT;
use super::ledger::{
    MAX_BLOCKS, OverflowBlock, RotateState, active_blocks, prune_blocks, spend_reset,
};
use super::log::msg;
use super::spend_blocks::valid_span;
use crate::commands::profile::auto_switch::kit::{State, exists};
use crate::commands::profile::limit_wait::plan::window;
use crate::commands::profile::name::ProfileName;
use crate::commands::recipe::run::limit_patterns::LimitKind;
use std::path::Path;

/// Why a block was not stored. A closed set of fixed strings: the ALERT
/// lines interpolate one of these, a parsed [`ProfileName`], a
/// [`LimitKind`]'s own noun and a formatted time, and nothing else. No
/// limit-message text, no reset as read, no process output and no path can
/// reach the log through them.
const OUT_OF_RANGE: &str = "stated reset out of range";
const ALREADY_PASSED: &str = "stated reset already passed";
const NO_RESET: &str = "no stated reset";
const BUSY: &str = "rotator busy";
const UNREADABLE: &str = "ledger unreadable";
const NOT_WRITTEN: &str = "ledger not written";
const TOO_MANY: &str = "too many blocks";

/// Records `login` as blocked from `at` until `until` in the rotate ledger
/// under `root`, with one ALERT naming `kind`.
///
/// `at` is the real clock and `until` the reset the limit message stated,
/// moved onto the same clock by the caller, or `None` when the message
/// stated none. Both are checked here before anything is written.
///
/// Never ends, delays or refuses the run, and never panics: every outcome
/// but a stored block is a line on stderr and in the rotate log. The return
/// type is what enforces that; there is nothing for a caller to branch on.
pub fn note_limit_block(
    root: &Path,
    login: &ProfileName,
    at: i64,
    until: Option<i64>,
    kind: LimitKind,
) {
    let rs = RotateState::new(root);
    // 0. No ledger: the rotator has never run under this root, so there is
    //    nothing that reads a block, nothing to create, and nothing worth
    //    saying. Hoisted above the resolution below so that a host with no
    //    rotator stays silent whatever the message said.
    if !exists(&rs.ledger) {
        return;
    }
    let stored = resolved_until(until, at, kind)
        .ok_or(NO_RESET)
        .and_then(|until| bounded(at, until, kind))
        .and_then(|until| store(&rs, login, at, until));
    match stored {
        Ok(Some(until)) => report(&rs, at, &msg::alert_step_limit_block(login, kind, until)),
        Ok(None) => {}
        Err(why) => report(
            &rs,
            at,
            &msg::step_limit_block_not_written(login, kind, why),
        ),
    }
}

/// The earliest end after `now` of a block the rotate ledger under `root`
/// records on one of `logins`, and that login (issue #2967).
///
/// This is the one reader a step's wait has of another login's reset. It
/// reads the blocks [`note_limit_block`] writes, each ending at the reset a
/// limit message stated, and the blocks the rotator records itself, by the
/// rule every block reader uses (`active_blocks`). The rotator wakes at the
/// same ends (`wake::deadline`), so the step and the rotator cannot disagree
/// about when a block ends. A tie goes to the first name.
///
/// `None` when the host has no ledger, as on a host the rotator has never
/// run on, where no block is recorded, and when no such block stands. A
/// ledger that cannot be read also gives `None`, and the step says so on
/// stderr.
pub fn earliest_end<'a>(root: &Path, logins: &[&'a str], now: i64) -> Option<(&'a str, i64)> {
    let ledger = match RotateState::new(root).load_ledger() {
        Ok(l) => l?,
        Err(e) => {
            eprintln!(
                "amplihack: the rotate ledger cannot be read: {e}; no other login's recorded \
                 reset can end this step's wait (auto-switch)"
            );
            return None;
        }
    };
    let blocks = active_blocks(&ledger, now);
    logins
        .iter()
        .filter_map(|login| blocks.get(*login).map(|b| (*login, b.until)))
        .min_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)))
}

/// The end of the block to write, or `None` when none can be known.
///
/// A spend limit that states no reset falls back to [`spend_reset`], the
/// month boundary the rotator already assumes for one; that is the only
/// assumption made anywhere here. Every other kind writes nothing rather
/// than invent an end, because a block that outlives the limit keeps a
/// healthy login out of rotation.
fn resolved_until(until: Option<i64>, at: i64, kind: LimitKind) -> Option<i64> {
    match (until, kind) {
        (Some(u), _) => Some(u),
        (None, LimitKind::Spend) => Some(spend_reset(at)),
        (None, _) => None,
    }
}

/// Step 2: the end of the block to store, or one of the fixed reasons
/// above.
///
/// The loader's span rule is applied to the reset as stated, before the
/// kind's window shortens it, so a reset the loader would refuse is still
/// reported as out of range. The window then gives the end that is stored:
/// the stated reset or the kind's [`window`] from `at`, whichever comes
/// first. The usage-limit wait holds a stated reset to the same windows.
fn bounded(at: i64, until: i64, kind: LimitKind) -> Result<i64, &'static str> {
    // A reset the clock has already gone past is named separately:
    // `valid_span` refuses it for the same reason it refuses a reset beyond
    // its 62 days, but the two read nothing alike to whoever finds the
    // ALERT, so the lapsed one is taken first and `OUT_OF_RANGE` keeps the
    // rest.
    if until <= at {
        return Err(ALREADY_PASSED);
    }
    if !valid_span(&OverflowBlock::spend_limit_until(at, until)) {
        return Err(OUT_OF_RANGE);
    }
    Ok(match window(kind) {
        Some(window) => until.min(at.saturating_add(window)),
        None => until,
    })
}

/// Steps 3 to 7, for an end [`bounded`] has already checked.
/// `Ok(Some(until))` stored a block, `Ok(None)` found nothing to do,
/// `Err(why)` is one of the fixed reasons above. The lock is released on
/// every path before this returns, so the log append never holds it.
fn store(
    rs: &RotateState,
    login: &ProfileName,
    at: i64,
    until: i64,
) -> Result<Option<i64>, &'static str> {
    let block = OverflowBlock::spend_limit_until(at, until);
    // 3. The rotate state lock, as `profile overflow clear` takes it. No
    //    retry loop: a rotator that holds it longer loses this block, and
    //    the next failing step records one.
    let state = State::new(&rs.root).map_err(|e| {
        eprintln!("amplihack: could not open the rotate state: {e}");
        NOT_WRITTEN
    })?;
    let _lock = match state.try_lock(CLEAR_LOCK_WAIT) {
        Ok(Some(lock)) => lock,
        Ok(None) => return Err(BUSY),
        Err(e) => {
            eprintln!("amplihack: could not take the rotate state lock: {e}");
            return Err(NOT_WRITTEN);
        }
    };
    // 4. Load under the lock. A ledger removed since step 0 is left alone:
    //    returning here is what keeps the writer from creating one.
    let mut l = match rs.load_ledger() {
        Ok(Some(l)) => l,
        Ok(None) => return Ok(None),
        Err(e) => {
            eprintln!("amplihack: could not read the rotate ledger: {e}");
            return Err(UNREADABLE);
        }
    };
    // 5. Pruning first leaves only blocks in force at `at`, so the
    //    membership test below is the active-block test. An active block
    //    stands as it is: no extension and no second ALERT, so a run of
    //    failing steps writes one line, not one per step.
    prune_blocks(&mut l, at);
    if l.overflow_blocked.contains_key(login.as_str()) {
        return Ok(None);
    }
    // 6. The cap the loader enforces, checked before the insert.
    if l.overflow_blocked.len() >= MAX_BLOCKS {
        return Err(TOO_MANY);
    }
    // 7. Save. The lock is released as this returns, before the log
    //    append in `report`.
    l.overflow_blocked.insert(login.to_string(), block);
    rs.save_ledger(&l).map_err(|e| {
        eprintln!("amplihack: could not write the rotate ledger: {e}");
        NOT_WRITTEN
    })?;
    Ok(Some(until))
}

/// One line on stderr, where the step's own output goes, and one in the
/// rotate log, beside the rotator's own spend-block lines. A log that cannot
/// be written is a warning and nothing more.
fn report(rs: &RotateState, at: i64, line: &str) {
    eprintln!("{line}");
    let wrote = rs.ensure().map_err(|e| e.to_string()).and_then(|()| {
        let mut log = super::log::RotateLog::open(&rs.log, &rs.log1).map_err(|e| e.to_string())?;
        log.write(at, line).map_err(|e| e.to_string())
    });
    if let Err(e) = wrote {
        eprintln!("warning: could not write the rotate log: {e}");
    }
}
