//! What an active spend block means to the overflow rule (issues #1606,
//! #2890): whether a block is still in force, and the reading a blocked
//! plan login contributes in place of its own.

use super::super::decide::{Measured, Probed, Reading, Windows};
use super::super::ledger::OverflowBlock;
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;

/// Whether `name` has a block still in force at `now`.
pub(crate) fn active(
    blocks: &BTreeMap<String, OverflowBlock>,
    name: &ProfileName,
    now: i64,
) -> bool {
    blocks.get(name.as_str()).is_some_and(|b| now < b.until)
}

/// The reading a plan login with an active spend block contributes
/// (issue #2890): exhausted with no figures, and no windows.
///
/// A spend limit is invisible to the `/usage` probe, so such a login reads
/// as healthy and the rules below would keep it live or move work onto it.
/// This is a substitution on one input, not a new branch: `Exhausted` with
/// no figures is the value `needs_swap`, `best`, `plan_room::has_room` and
/// `plan_room::has_room_under` already treat as "no room", and
/// `Probed::excluded` (issue #2218) already uses it for a login that is
/// never read. The windows are blanked so that the blocked login's own
/// reset cannot shorten the overflow gate through `regains`: a login that
/// cannot spend says nothing about when plan capacity returns.
///
/// `unknown` is deliberately left as read. The block is positive evidence
/// that this login cannot be used, not an unread probe, so it must not
/// start holding the per-token tiers back.
pub(crate) fn without_room(p: &Probed) -> Probed {
    Probed {
        reading: NO_ROOM,
        windows: Windows::default(),
        ..p.clone()
    }
}

/// [`without_room`] for a candidate.
pub(crate) fn measured_without_room(c: &Measured) -> Measured {
    Measured {
        reading: NO_ROOM,
        windows: Windows::default(),
        ..c.clone()
    }
}

/// The reading [`without_room`] substitutes: exhausted, with no figures.
const NO_ROOM: Reading = Reading::Exhausted {
    session: None,
    week: None,
};
