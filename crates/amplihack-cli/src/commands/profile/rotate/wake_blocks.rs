//! Issue #2909: the blocks a cycle decided on, kept so the watch loop can
//! tell when the ledger now says something that cycle did not act on. A
//! file of its own so `wake.rs` stays within its line budget.

use super::super::super::ledger::{OverflowBlock, RotateLedger};
use std::collections::BTreeMap;

/// Whether one of `blocks` ended after `since` and by `now_unix`.
fn fell_due<'a>(mut blocks: impl Iterator<Item = &'a OverflowBlock>, since: i64, now: i64) -> bool {
    blocks.any(|b| since < b.until && b.until <= now)
}

/// Whether a reset recorded in `l` fell due after `since`, when the last
/// cycle began, and by `now_unix`.
///
/// A cycle decides on the blocks in force when it reads them, so a reset
/// that passes while it runs is one it did not act on, and `deadline`
/// looks only forward. A reset before `since` was already in the past when
/// the last cycle read the ledger, so it cannot cause a cycle twice.
///
/// The cycle's own save prunes a block whose reset has passed, so the
/// ledger read after that save no longer holds it. The watch loop
/// therefore asks [`Blocks::fell_due`] as well, over the blocks the cycle
/// began with; this one still answers for a ledger the save did not reach.
pub fn reset_fell_due(l: &RotateLedger, since: i64, now_unix: i64) -> bool {
    let blocks = l.overflow_blocked.values().chain(l.platform_block.iter());
    fell_due(blocks, since, now_unix)
}

/// Every block the ledger records, as a cycle could have read them.
///
/// A step that meets a limit records it under the rotate lock, and a cycle
/// holds that lock only from revalidation to its last save, not while it
/// measures. A record written while a cycle measures is therefore kept by
/// that cycle's own save, under the lock, but the cycle decided without
/// it. The file then holds, byte for byte, what this process wrote, so
/// `is_self_write` alone would take the record for the rotator's own write
/// and the limit would wait for `--interval`. Comparing these with the
/// blocks the cycle began with catches it. A block the cycle added itself
/// differs too, and costs one extra cycle: the cheaper error.
#[derive(Default)]
pub struct Blocks {
    overflow: BTreeMap<String, OverflowBlock>,
    platform: Option<OverflowBlock>,
}

impl Blocks {
    pub fn of(l: &RotateLedger) -> Self {
        Self {
            overflow: l.overflow_blocked.clone(),
            platform: l.platform_block.clone(),
        }
    }

    /// Whether the blocks in force at `now` differ between the two. A
    /// block that expired in between is out of both, so a prune is not a
    /// change; a reset that passed during a cycle is [`Self::fell_due`]'s.
    pub fn differ_at(&self, other: &Self, now: i64) -> bool {
        self.active(now) != other.active(now)
    }

    /// Whether one of these blocks, the ones the last cycle began with,
    /// ended after `since` and by `now_unix`: the cycle held that login out
    /// and its reset has passed since.
    ///
    /// Asked of these and not of the ledger, because the cycle's own save
    /// prunes the block (`prune_blocks` at the time of the save) and the
    /// ledger read afterwards no longer says it was there. Without this, a
    /// login whose reset passed mid-cycle would stay out until the next
    /// event or `--interval`.
    pub fn fell_due(&self, since: i64, now_unix: i64) -> bool {
        fell_due(
            self.overflow.values().chain(self.platform.iter()),
            since,
            now_unix,
        )
    }

    fn active(&self, now: i64) -> (Vec<(&String, &OverflowBlock)>, Option<&OverflowBlock>) {
        let overflow = self.overflow.iter().filter(|(_, v)| now < v.until);
        (
            overflow.collect(),
            self.platform.as_ref().filter(|p| now < p.until),
        )
    }
}
