//! The spend block a failed agent step records for the login it ran under
//! (issue #2890).
//!
//! The rotator's `/usage` probe cannot see an individual spend limit, so a
//! plan login that has reached one reads as healthy and the rotator keeps it
//! live while every step on it fails. The step that meets the limit is the
//! only witness, so it writes the block itself, into the same
//! `overflow_blocked` map and with the same `spend_limit` reason the rotator
//! already uses. Nothing else changes: `active_blocks` and `prune_blocks`
//! expire it at the stated reset, `profile overflow clear` removes it, and
//! every reader that already honours a block honours this one.
//!
//! This is data the rotator reads, not a gate. [`note_spend_block`] returns
//! `()` so that no caller can branch on the outcome: whatever happens here,
//! the step goes on to plan its wait exactly as it would have. Every failure
//! is reported, on stderr and in the rotate log, and nothing is stored.
//!
//! The order is fixed, and each step exists for a reason:
//!
//! 1. **The ledger exists.** A step never creates the rotator's state.
//! 2. **The span is valid** ([`valid_span`], the loader's own rule). A block
//!    outside it would make the whole ledger unreadable on the next load,
//!    which would stop the rotator's prelude on every cycle.
//! 3. **The lock**, waited for a bounded time and then given up. A step is
//!    never stalled by a busy rotator.
//! 4. **Load**; a ledger that disappeared between 1 and 3 is left alone.
//! 5. **An active block is kept as it is**: never extended, never alerted
//!    about twice. An expired one is pruned and replaced.
//! 6. **The cardinality cap**, before the insert.
//! 7. **Save**; the lock is released as `store` returns.
//! 8. **Log**, outside the lock.

use super::control::CLEAR_LOCK_WAIT;
use super::ledger::{MAX_BLOCKS, OverflowBlock, RotateState, active_blocks, prune_blocks};
use super::log::msg;
use super::spend_blocks::valid_span;
use crate::commands::profile::auto_switch::kit::{State, exists};
use crate::commands::profile::name::ProfileName;
use std::path::Path;

/// Why a block was not stored. A closed set of fixed strings: the ALERT
/// lines interpolate one of these, a parsed [`ProfileName`] and a formatted
/// time, and nothing else. No limit-message text, no reset as read, no
/// process output and no path can reach the log through them.
const OUT_OF_RANGE: &str = "stated reset out of range";
const BUSY: &str = "rotator busy";
const UNREADABLE: &str = "ledger unreadable";
const NOT_WRITTEN: &str = "ledger not written";
const TOO_MANY: &str = "too many blocks";

/// Records `login` as spend-blocked from `at` until `until` in the rotate
/// ledger under `root`, with one ALERT.
///
/// `at` is the real clock and `until` the reset the limit message stated,
/// moved onto the same clock by the caller. Both are checked here before
/// anything is written.
///
/// Never ends, delays or refuses the run, and never panics: every outcome
/// but a stored block is a line on stderr and in the rotate log.
pub fn note_spend_block(root: &Path, login: &ProfileName, at: i64, until: i64) {
    let rs = RotateState::new(root);
    match store(&rs, login, at, until) {
        Ok(Some(until)) => report(&rs, at, &msg::alert_step_spend_limit(login, until)),
        Ok(None) => {}
        Err(why) => report(&rs, at, &msg::step_spend_limit_not_blocked(login, why)),
    }
}

/// Steps 1 to 7. `Ok(Some(until))` stored a block, `Ok(None)` found nothing
/// to do, `Err(why)` is one of the fixed reasons above. The lock is released
/// on every path before this returns, so the log append never holds it.
fn store(
    rs: &RotateState,
    login: &ProfileName,
    at: i64,
    until: i64,
) -> Result<Option<i64>, &'static str> {
    // 1. No ledger: the rotator has never run under this root, so there is
    //    nothing that reads a block and nothing to create.
    if !exists(&rs.ledger) {
        return Ok(None);
    }
    // 2. The loader's own span rule, applied before the insert.
    let block = OverflowBlock::spend_limit_until(at, until);
    if !valid_span(&block) {
        return Err(OUT_OF_RANGE);
    }
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
    // 4. Load under the lock. A ledger removed since step 1 is left alone:
    //    returning here is what keeps the writer from creating one.
    let mut l = match rs.load_ledger() {
        Ok(Some(l)) => l,
        Ok(None) => return Ok(None),
        Err(e) => {
            eprintln!("amplihack: could not read the rotate ledger: {e}");
            return Err(UNREADABLE);
        }
    };
    // 5. An active block stands as it is: no extension and no second ALERT,
    //    so a run of failing steps writes one line, not one per step.
    if active_blocks(&l, at).contains_key(login.as_str()) {
        return Ok(None);
    }
    prune_blocks(&mut l, at);
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
