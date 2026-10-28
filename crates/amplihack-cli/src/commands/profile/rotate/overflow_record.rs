//! What a cycle records and says beyond the swap itself (issue #1606):
//! spend blocks with their one ALERT, the `credits:` log lines, and the
//! no-capacity file, which reads `ok` while the live login runs on credits.

use super::decide::Reading;
use super::ledger::{
    LastCycle, MAX_BLOCKS, OverflowBlock, RotateLedger, RotateState, active_blocks, prune_blocks,
};
use super::log::{Sink, msg};
use super::observe::rotated;
use super::overflow::Tag;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::capacity_from_rotation;
use crate::commands::profile::name::ProfileName;
use std::path::Path;

/// Stores a spend block for each of `names` that has no active one, with
/// one ALERT per block. The block record, not `ledger.alert`, prevents a
/// repeat, so the hook still sees capacity when the cycle moves elsewhere.
/// A block that would not end after it starts (a time outside chrono's
/// range) is not stored, since the ledger would then fail to load. Returns
/// whether the ledger changed.
pub fn block_spend_limited(
    out: &mut Sink,
    l: &mut RotateLedger,
    names: &[ProfileName],
    t: i64,
) -> bool {
    block_with(out, l, names, t, msg::alert_spend_limit)
}

/// [`block_spend_limited`] for the overflow profile a `profile switch` pin
/// holds the live login on: the one ALERT also says that the rotator will
/// not move off it, and names `profile switch --none`.
pub fn block_pinned_spend_limited(
    out: &mut Sink,
    l: &mut RotateLedger,
    name: &ProfileName,
    t: i64,
) -> bool {
    block_with(
        out,
        l,
        std::slice::from_ref(name),
        t,
        msg::alert_spend_limit_pinned,
    )
}

fn block_with(
    out: &mut Sink,
    l: &mut RotateLedger,
    names: &[ProfileName],
    t: i64,
    alert: fn(&ProfileName, i64) -> String,
) -> bool {
    let mut changed = false;
    for n in names {
        if active_blocks(l, t).contains_key(n.as_str()) {
            continue;
        }
        prune_blocks(l, t);
        if l.overflow_blocked.len() >= MAX_BLOCKS {
            out.emit(t, &msg::spend_limit_not_blocked(n, "too many blocks"));
            continue;
        }
        let block = OverflowBlock::spend_limit(t);
        if block.until <= block.at {
            out.emit(t, &msg::spend_limit_not_blocked(n, "no end time"));
            continue;
        }
        out.emit(t, &alert(n, block.until));
        l.overflow_blocked.insert(n.to_string(), block);
        changed = true;
    }
    changed
}

/// The previous recorded cycle left the live login `live` on credits.
fn was_on_overflow(l: &RotateLedger, live: Option<&ProfileName>) -> bool {
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
        Some(Tag::In) => out.emit(t, &msg::onto_overflow(to, next_plan_reset)),
        Some(Tag::Out) => out.emit(t, &msg::off_overflow(from, to)),
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
    let live = live.filter(|_| tag == Some(Tag::On) && !was_on_overflow(l, live))?;
    Some(msg::stay_on_overflow(live, next_plan_reset))
}

/// Rewrites the no-capacity file from this cycle's readings
/// ([`rotated`]: the live profile's and every other candidate's, as
/// measured). On credits it reads `ok`, since work can go on: the
/// limit-resume hook and the recipe-run wait must see overflow as capacity
/// (issue #1694 point 4), so the tag is passed through, never dropped.
pub fn refresh_capacity(
    root: &Path,
    live: Option<&ProfileName>,
    reading: &Reading,
    others: &[(ProfileName, Reading)],
    t: i64,
    tag: Option<Tag>,
) {
    capacity_from_rotation(
        root,
        &rotated(live, reading, others),
        t,
        Tag::on_overflow(tag),
    );
}

/// Records the cycle as ended at `t` with `outcome`, after measuring from
/// `measured_at`, with its overflow tag, drops expired spend blocks, and
/// saves the ledger.
pub fn record(
    rs: &RotateState,
    l: &mut RotateLedger,
    outcome: &str,
    t: i64,
    measured_at: Option<i64>,
    tag: Option<Tag>,
) -> Result<(), ProfileError> {
    l.last_cycle = Some(LastCycle {
        at: t,
        measured_at,
        outcome: outcome.into(),
        overflow: tag.map(|t| t.as_str().to_string()),
    });
    prune_blocks(l, t);
    rs.save_ledger(l)
}
