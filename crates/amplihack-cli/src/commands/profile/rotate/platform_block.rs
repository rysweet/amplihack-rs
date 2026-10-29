//! The API-key block (issue #1700): one record, `platform_block` in the
//! rotate ledger, that keeps the rotator and auto-switch off every API-key
//! profile after a session reports that the key is out of funds, revoked or
//! rate-limited. See "API-key block" in docs/reference/profile-command.md.
//!
//! A block is written under the rotate state lock and only when none is
//! active, so two sessions failing at once record one block and one ALERT.

use super::ledger::{
    KEY_EXHAUSTED, KEY_RATE_LIMITED, KEY_REVOKED, MAX_BLOCK_SECS, OverflowBlock, RotateLedger,
    RotateState,
};
use super::log::{self, msg};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, exists};
use std::path::Path;
use std::time::Duration;

/// How long a session waits for the rotate state lock.
const LOCK_WAIT: Duration = Duration::from_secs(10);
/// A rate limit that gives no usable time lasts this long.
const DEFAULT_RATE_LIMIT_SECS: i64 = 3600;

/// What [`record`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// A new block: the caller writes the one ALERT.
    Created,
    /// A block was already active; nothing changed.
    AlreadyBlocked,
}

/// The active block at `now`, if any.
pub fn active(l: &RotateLedger, now: i64) -> Option<&OverflowBlock> {
    l.platform_block.as_ref().filter(|b| now < b.until)
}

/// When a block for `reason` recorded at `now` ends. `key_exhausted` and
/// `key_revoked` last until cleared, stored as the longest block a ledger
/// may hold. `key_rate_limited` lasts until `until`; a missing or past one
/// is an hour, one beyond the longest block is clamped to it.
fn block_end(reason: &str, until: Option<i64>, now: i64) -> i64 {
    let longest = now.saturating_add(MAX_BLOCK_SECS);
    if reason != KEY_RATE_LIMITED {
        return longest;
    }
    match until {
        Some(t) if t > now => t.min(longest),
        _ => now.saturating_add(DEFAULT_RATE_LIMIT_SECS).min(longest),
    }
}

/// Records an API-key block for `reason` (one of `KEY_REASONS`) unless one
/// is active. `until` is the reset a rate-limit error gave, epoch seconds.
pub fn record(
    root: &Path,
    reason: &'static str,
    until: Option<i64>,
    now: i64,
) -> Result<Recorded, ProfileError> {
    debug_assert!([KEY_EXHAUSTED, KEY_REVOKED, KEY_RATE_LIMITED].contains(&reason));
    let rs = RotateState::new(root);
    rs.ensure()?;
    let state = State::new(&rs.root)?;
    let Some(_lock) = state.try_lock(LOCK_WAIT)? else {
        return Err(ProfileError::RotateDaemon("rotator busy, retry".into()));
    };
    let mut l = rs.load_ledger()?.unwrap_or_default();
    if active(&l, now).is_some() {
        return Ok(Recorded::AlreadyBlocked);
    }
    l.platform_block = Some(OverflowBlock::key(
        reason,
        now,
        block_end(reason, until, now),
    ));
    rs.save_ledger(&l)?;
    Ok(Recorded::Created)
}

/// A short, fixed description of why a block could not be stored: an I/O
/// error's kind, or the kind of failure. Never a path or file content.
fn error_kind(e: &ProfileError) -> String {
    match e {
        ProfileError::Io(io) => io.kind().to_string(),
        ProfileError::RotateDaemon(_) => "state lock busy".into(),
        ProfileError::RotateInconsistent(_) => "rotate ledger unreadable".into(),
        ProfileError::RotateRefused(_) => "state directory refused".into(),
        _ => "error".into(),
    }
}

/// Appends one line to the rotate log; a log that cannot be written warns
/// once on stderr.
fn log_line(rs: &RotateState, now: i64, line: &str) {
    if let Some(mut l) = log::open_or_warn(&rs.log, &rs.log1)
        && let Err(e) = l.write(now, line)
    {
        eprintln!("warning: could not write the rotate log: {e}");
    }
}

/// [`record`], and the one ALERT for a new block. A failure is logged by
/// its kind only and never stops the caller.
pub fn record_and_alert(root: &Path, reason: &'static str, until: Option<i64>, now: i64) {
    let rs = RotateState::new(root);
    match record(root, reason, until, now) {
        Ok(Recorded::Created) => {
            let end = block_end(reason, until, now);
            log_line(&rs, now, &msg::alert_platform_block(reason, end));
        }
        Ok(Recorded::AlreadyBlocked) => {}
        Err(e) => {
            let line = msg::platform_block_not_recorded(&error_kind(&e));
            eprintln!("amplihack: {line}");
            if rs.ensure().is_ok() {
                log_line(&rs, now, &line);
            }
        }
    }
}

/// Removes the block under the rotate state lock. `Ok(None)` when there was
/// none; else its reason. Never creates the ledger.
pub fn clear_reason(root: &Path) -> Result<Option<String>, ProfileError> {
    let rs = RotateState::new(root);
    if !exists(&rs.ledger) {
        return Ok(None);
    }
    let state = State::new(&rs.root)?;
    let Some(_lock) = state.try_lock(LOCK_WAIT)? else {
        return Err(ProfileError::RotateDaemon("rotator busy, retry".into()));
    };
    let Some(mut l) = rs.load_ledger()? else {
        return Ok(None);
    };
    let Some(b) = l.platform_block.take() else {
        return Ok(None);
    };
    rs.save_ledger(&l)?;
    Ok(Some(b.reason))
}

/// Removes the block. `Ok(false)` when there was none.
#[cfg(test)]
pub fn clear(root: &Path) -> Result<bool, ProfileError> {
    clear_reason(root).map(|r| r.is_some())
}
