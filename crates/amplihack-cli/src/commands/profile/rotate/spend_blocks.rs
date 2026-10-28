//! Spend blocks on overflow profiles (issue #1606): when one ends and which
//! are still in force. The records live in the rotate ledger
//! (`RotateLedger::overflow_blocked`).

use super::ledger::{OverflowBlock, RotateLedger};
use std::collections::BTreeMap;

/// When an organisation's monthly spend resets, as assumed for a block
/// recorded at `at`: the 1st of the next calendar month, 00:00 UTC. Claude's
/// message gives no reset time, so this is the documented assumption.
pub fn spend_reset(at: i64) -> i64 {
    use chrono::{Datelike, TimeZone, Utc};
    let Some(t) = chrono::DateTime::from_timestamp(at, 0) else {
        return at;
    };
    let (y, m) = if t.month() == 12 {
        (t.year() + 1, 1)
    } else {
        (t.year(), t.month() + 1)
    };
    Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0)
        .single()
        .map_or(at, |d| d.timestamp())
}

/// The blocks still in force at `now` (`now < until`).
pub fn active_blocks(l: &RotateLedger, now: i64) -> BTreeMap<String, OverflowBlock> {
    l.overflow_blocked
        .iter()
        .filter(|(_, b)| now < b.until)
        .map(|(n, b)| (n.clone(), b.clone()))
        .collect()
}

/// Drops the blocks that ended at or before `now`.
pub fn prune_blocks(l: &mut RotateLedger, now: i64) {
    l.overflow_blocked.retain(|_, b| now < b.until);
}
