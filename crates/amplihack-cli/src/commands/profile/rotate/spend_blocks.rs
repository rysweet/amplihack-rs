//! Spend blocks on overflow profiles (issue #1606): when one ends and which
//! are still in force. The records live in the rotate ledger
//! (`RotateLedger::overflow_blocked`).

use super::ledger::{
    MAX_BLOCK_SECS, MAX_BLOCKS, OVERFLOW_TAGS, OverflowBlock, RotateLedger, SPEND_LIMIT,
};
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;

/// `live_kind` while an API-key profile is live (issue #1700).
pub const LIVE_KIND_PLATFORM: &str = "platform";
/// The API-key block reasons (issue #1700).
pub const KEY_EXHAUSTED: &str = "key_exhausted";
pub const KEY_REVOKED: &str = "key_revoked";
pub const KEY_RATE_LIMITED: &str = "key_rate_limited";
pub const KEY_REASONS: [&str; 3] = [KEY_EXHAUSTED, KEY_REVOKED, KEY_RATE_LIMITED];

/// The overflow fields of a loaded ledger: a known tag, at most
/// [`MAX_BLOCKS`] blocks, each keyed by a profile name, for a spend limit,
/// recorded at a positive time and ending after it but within
/// [`MAX_BLOCK_SECS`], so no block can be made to last for ever. An end
/// that cannot be computed counts as invalid. Issue #1700: `live_kind` is
/// absent or `platform` with a `live`, and `platform_block` has a key
/// reason and the same time rules.
pub(super) fn valid_overflow(l: &RotateLedger) -> bool {
    let span_ok = |b: &OverflowBlock| {
        b.at > 0
            && b.at < b.until
            && b.at
                .checked_add(MAX_BLOCK_SECS)
                .is_some_and(|max| b.until <= max)
    };
    let live_kind_ok = l
        .live_kind
        .as_deref()
        .is_none_or(|k| k == LIVE_KIND_PLATFORM && l.live.is_some());
    let platform_block_ok = l
        .platform_block
        .as_ref()
        .is_none_or(|b| KEY_REASONS.contains(&b.reason.as_str()) && span_ok(b));
    let tag_ok = l
        .last_cycle
        .as_ref()
        .and_then(|c| c.overflow.as_deref())
        .is_none_or(|t| OVERFLOW_TAGS.contains(&t));
    tag_ok
        && live_kind_ok
        && platform_block_ok
        && l.overflow_blocked.len() <= MAX_BLOCKS
        && l.overflow_blocked.iter().all(|(name, b)| {
            ProfileName::parse(name).is_ok() && b.reason == SPEND_LIMIT && span_ok(b)
        })
}

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

/// Drops the blocks that ended at or before `now`, the API-key block
/// (issue #1700) included.
pub fn prune_blocks(l: &mut RotateLedger, now: i64) {
    l.overflow_blocked.retain(|_, b| now < b.until);
    if l.platform_block.as_ref().is_some_and(|b| now >= b.until) {
        l.platform_block = None;
    }
}
