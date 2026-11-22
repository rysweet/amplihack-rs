//! Issue #2909: the blocks a cycle decided on, kept so the watch loop can
//! tell when the ledger now says something that cycle did not act on. A
//! file of its own so `wake.rs` stays within its line budget.

use super::super::super::ledger::{OverflowBlock, RotateLedger};
use std::collections::BTreeMap;

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

    /// Whether one of these blocks ended after `since` and by `now_unix`.
    ///
    /// A cycle decides on the blocks in force when it reads them, so a reset
    /// that passes while it runs is one it did not act on, and `deadline`
    /// looks only forward. A reset before `since`, when the last cycle
    /// began, was already past when that cycle read the ledger, so it cannot
    /// cause a cycle twice.
    ///
    /// The watch loop asks this of the blocks the last cycle began with, not
    /// only of the ledger, because that cycle's own save prunes a block
    /// whose reset has passed and the ledger read afterwards no longer holds
    /// it. Without that, a login whose reset passed mid-cycle would stay out
    /// until the next event or `--interval`.
    pub fn fell_due(&self, since: i64, now_unix: i64) -> bool {
        self.overflow
            .values()
            .chain(self.platform.iter())
            .any(|b| since < b.until && b.until <= now_unix)
    }

    fn active(&self, now: i64) -> (Vec<(&String, &OverflowBlock)>, Option<&OverflowBlock>) {
        let overflow = self.overflow.iter().filter(|(_, v)| now < v.until);
        (
            overflow.collect(),
            self.platform.as_ref().filter(|p| now < p.until),
        )
    }
}
