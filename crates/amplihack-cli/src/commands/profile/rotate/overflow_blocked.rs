//! What an active spend block means to the overflow rule (issues #1606,
//! #2890): whether a block is still in force, and what a blocked plan login
//! contributes in place of its own reading.

use super::super::decide::target::standing_block;
use super::super::decide::{Reading, Windows};
use super::super::ledger::OverflowBlock;
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;

/// Whether `name` has a spend block still in force at `now`, by the
/// decision's one test of a standing block (`target::standing_block`).
pub(crate) fn active(
    blocks: &BTreeMap<String, OverflowBlock>,
    name: &ProfileName,
    now: i64,
) -> bool {
    standing_block(blocks, name, now).is_some()
}

/// The reading and windows a plan login with an active spend block
/// contributes in place of its own (issue #2890).
///
/// A spend limit is invisible to the `/usage` probe, so such a login reads
/// as healthy and the rules would keep it live or move work onto it. This is
/// a substitution on one input, not a new branch: `Exhausted` with no figures
/// is the value `needs_swap`, `target::fit`, `plan_room::has_room` and
/// `plan_room::has_room_under` already treat as "no room", and
/// `Probed::excluded` (issue #2218) already uses it for a login that is never
/// read. The windows are blanked so that the blocked login's own reset cannot
/// shorten the overflow gate through `regains`: a login that cannot spend
/// says nothing about when plan capacity returns.
///
/// `unknown` is deliberately left as read. The block is positive evidence
/// that this login cannot be used, not an unread probe, so it must not start
/// holding the per-token tiers back.
pub(crate) fn no_room() -> (Reading, Windows) {
    let exhausted = Reading::Exhausted {
        session: None,
        week: None,
    };
    (exhausted, Windows::default())
}
