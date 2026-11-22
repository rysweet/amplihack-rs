//! Blocks on a login (issues #1606, #2909): when one ends and which are
//! still in force. The records live in the rotate ledger
//! (`RotateLedger::overflow_blocked`).
//!
//! Issue #2909 widened the `reason` field from the single `spend_limit`
//! token to any limit kind, so whatever meets a usage, spend or credit
//! limit records it for the login it ran under and the rotator keeps that
//! login out of its choices until the recorded reset. The field is
//! admitted by a short-token rule ([`valid_kind`]) rather than a closed
//! list, so a writer this binary does not know about still blocks its
//! login; [`kind_label`] keeps a stored token out of every log line.

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

impl OverflowBlock {
    /// A spend-limit block recorded at `at` that ends at the time the limit
    /// message stated (issue #2890), rather than at the inferred monthly
    /// reset of [`OverflowBlock::spend_limit`]. The reason is the same, so
    /// every reader, [`valid_overflow`] included, is unchanged. The caller
    /// checks the span with [`valid_span`].
    pub fn spend_limit_until(at: i64, until: i64) -> Self {
        Self {
            at,
            until,
            reason: SPEND_LIMIT.to_string(),
        }
    }
}

/// The time rule every stored block obeys: recorded at a positive time,
/// ending after it, and within [`MAX_BLOCK_SECS`] of it, so no block can be
/// made to last for ever. An end that cannot be computed counts as invalid.
///
/// The one expression, shared by the loader's [`valid_overflow`] and by
/// `step_block`, which applies it *before* inserting so that a block a step
/// writes can never make the whole ledger unreadable.
pub(super) fn valid_span(b: &OverflowBlock) -> bool {
    b.at > 0
        && b.at < b.until
        && b.at
            .checked_add(MAX_BLOCK_SECS)
            .is_some_and(|max| b.until <= max)
}

/// The overflow fields of a loaded ledger: a known tag, at most
/// [`MAX_BLOCKS`] blocks, each keyed by a profile name, for a spend limit,
/// recorded at a positive time and ending after it but within
/// [`MAX_BLOCK_SECS`], so no block can be made to last for ever. An end
/// that cannot be computed counts as invalid. Issue #1700: `live_kind` is
/// absent or `platform` with a `live`, and `platform_block` has a key
/// reason and the same time rules.
pub(super) fn valid_overflow(l: &RotateLedger) -> bool {
    let live_kind_ok = l
        .live_kind
        .as_deref()
        .is_none_or(|k| k == LIVE_KIND_PLATFORM && l.live.is_some());
    let platform_block_ok = l
        .platform_block
        .as_ref()
        .is_none_or(|b| KEY_REASONS.contains(&b.reason.as_str()) && valid_span(b));
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
            ProfileName::parse(name).is_ok() && valid_kind(&b.reason) && valid_span(b)
        })
}

/// The five-hour session window ran out (issue #2909).
pub const SESSION_LIMIT: &str = "session_limit";
/// The seven-day window ran out.
pub const WEEK_LIMIT: &str = "week_limit";
/// The account's credit balance ran out.
pub const CREDIT_LIMIT: &str = "credit_limit";

/// The longest limit-kind token a stored block may carry.
const MAX_KIND: usize = 32;

/// Whether `kind` is a limit-kind token a stored block may carry:
/// `^[a-z0-9_]{1,32}$`, checked on bytes, so no case folding, no Unicode
/// and no separator can smuggle a path, a newline or a NUL through.
///
/// Issue #2909 widened the rule from an equality against [`SPEND_LIMIT`].
/// A token this binary does not recognise still blocks its login and still
/// contributes a wake time, which is the point: the rotator must honour a
/// limit a newer writer recorded rather than refuse the whole ledger. Every
/// other rule on a block is unchanged: the key is a [`ProfileName`], the
/// span passes [`valid_span`], and a ledger holds at most [`MAX_BLOCKS`].
pub fn valid_kind(kind: &str) -> bool {
    !kind.is_empty()
        && kind.len() <= MAX_KIND
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Fixed text for a stored limit kind, for a log line or an ALERT. The
/// return type is `&'static str`, so the compiler guarantees that no byte a
/// writer chose can reach the rotate log: a token outside the kinds this
/// binary knows is labelled, not quoted.
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        SPEND_LIMIT => "spend limit",
        SESSION_LIMIT => "session limit",
        WEEK_LIMIT => "week limit",
        CREDIT_LIMIT => "credit limit",
        KEY_EXHAUSTED => "API key exhausted",
        KEY_REVOKED => "API key revoked",
        KEY_RATE_LIMITED => "API key rate limited",
        _ => "a recorded limit",
    }
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

#[cfg(test)]
#[path = "tests_issue_2909.rs"]
mod tests_issue_2909;
