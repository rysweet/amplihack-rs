//! What a cycle records and says beyond the swap itself (issue #1606):
//! spend blocks with their one ALERT, the `credits:` log lines, and the
//! no-capacity file, which reads `ok` while the live login runs on credits.

use super::decide::Reading;
use super::ledger::{MAX_BLOCKS, OverflowBlock, RotateLedger, active_blocks, prune_blocks};
use super::log::{Sink, msg};
use super::overflow::Tag;
use crate::commands::profile::auto_switch::kit::{Rotated, capacity_from_rotation};
use crate::commands::profile::name::ProfileName;
use std::path::Path;

/// Stores a spend block for each of `names` that has no active one, with
/// one ALERT per block. The block record, not `ledger.alert`, prevents a
/// repeat, so the hook still sees capacity when the cycle moves elsewhere.
/// Returns whether the ledger changed.
pub fn block_spend_limited(
    out: &mut Sink,
    l: &mut RotateLedger,
    names: &[ProfileName],
    t: i64,
) -> bool {
    let mut changed = false;
    for n in names {
        if active_blocks(l, t).contains_key(n.as_str()) {
            continue;
        }
        prune_blocks(l, t);
        if l.overflow_blocked.len() >= MAX_BLOCKS {
            eprintln!("warning: rotate: too many overflow blocks; {n} is not recorded");
            continue;
        }
        let block = OverflowBlock::spend_limit(t);
        out.emit(t, &msg::alert_spend_limit(n, block.until));
        l.overflow_blocked.insert(n.to_string(), block);
        changed = true;
    }
    changed
}

/// The previous recorded cycle left the live login `live` on credits.
fn was_on_credits(l: &RotateLedger, live: Option<&ProfileName>) -> bool {
    let tag = l.last_cycle.as_ref().and_then(|c| c.overflow.as_deref());
    matches!(tag, Some("in" | "on")) && live.is_some_and(|n| l.live.as_deref() == Some(n.as_str()))
}

/// The `credits:` line after a swap from `from` to `to`, if it moved onto
/// or off credits.
pub fn note_move(
    out: &mut Sink,
    tag: Option<Tag>,
    from: &ProfileName,
    to: &ProfileName,
    next_plan_reset: Option<i64>,
) {
    let t = crate::commands::profile::auto_switch::kit::now();
    match tag {
        Some(Tag::In) => out.emit(t, &msg::on_credits(to, next_plan_reset)),
        Some(Tag::Out) => out.emit(t, &msg::off_credits(from, to)),
        Some(Tag::On) | None => {}
    }
}

/// The `credits: staying on` line for a stay on overflow profile `live`,
/// when the previous recorded cycle in `l` did not leave it on credits.
/// Computed from the ledger as it was before this cycle changes it.
pub fn stay_note(
    l: &RotateLedger,
    tag: Option<Tag>,
    live: Option<&ProfileName>,
    next_plan_reset: Option<i64>,
) -> Option<String> {
    let live = live.filter(|_| tag == Some(Tag::On) && !was_on_credits(l, live))?;
    Some(msg::stay_on_credits(live, next_plan_reset))
}

/// Rewrites the no-capacity file from this cycle's readings: the live
/// profile's and every other candidate's, as measured. On credits it reads
/// `ok`, since work can go on.
pub fn refresh_capacity(
    root: &Path,
    live: Option<&ProfileName>,
    reading: &Reading,
    others: &[(ProfileName, Reading)],
    t: i64,
    tag: Option<Tag>,
) {
    let of = |r: &Reading| match *r {
        Reading::Measured { session, week } => Rotated::Measured { session, week },
        Reading::Exhausted => Rotated::Exhausted,
        Reading::Failed => Rotated::Failed,
    };
    let readings: Vec<(ProfileName, Rotated)> = live
        .map(|n| (n.clone(), of(reading)))
        .into_iter()
        .chain(others.iter().map(|(n, r)| (n.clone(), of(r))))
        .collect();
    capacity_from_rotation(root, &readings, t, Tag::on_credits(tag));
}
