//! What a cycle records and says beyond the swap itself (issue #1606):
//! spend blocks with their one ALERT, the `credits:` log lines, and the
//! no-capacity file, which reads `ok` while the live login runs on credits.

use super::alert::{alert_key, should_alert};
use super::cycle::Ctx;
use super::decide::Reading;
use super::identify::LiveId;
use super::ledger::{
    LIVE_KIND_PLATFORM, LastCycle, MAX_BLOCKS, OverflowBlock, RotateLedger, RotateState,
    active_blocks, prune_blocks,
};
use super::log::{Sink, msg};
use super::observe::{Snap, live_name};
use super::overflow::Tag;
use super::overflow_msg::{CheckNote, GateNote};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Rotated, capacity_from_rotation, now};
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

/// The login check for a swap onto `to` (issue #1804): under the lock, after
/// revalidation and before the journal. Only the API-key profile has one.
pub fn target_check(ctx: &Ctx, snap: &Snap, to: &ProfileName) -> Result<CheckNote, String> {
    if snap.platform() == Some(to) {
        (ctx.key_check)(&ctx.store, to).map(CheckNote::Key)
    } else {
        Ok(CheckNote::Credit)
    }
}

/// Writes a failed login check: the `key_check_refused` line and its ALERT
/// go to the log once per profile; a repeat is only printed.
pub fn refuse_key(out: &mut Sink, l: &mut RotateLedger, to: &ProfileName, why: &str, t: i64) {
    let new = should_alert(l, &alert_key("key_check", Some(to)));
    for line in [
        msg::key_check_refused(to, why),
        msg::alert_key_check(to, why),
    ] {
        if new {
            out.emit(t, &line);
        } else {
            out.print(t, &line);
        }
    }
}

/// The reason line for a move onto per-token profile `to`, written before
/// the swap line. Nothing for any other move.
pub fn note_per_token(
    out: &mut Sink,
    snap: &Snap,
    tag: Option<Tag>,
    to: &ProfileName,
    next_plan_reset: Option<i64>,
    check: CheckNote,
) {
    if tag != Some(Tag::In) {
        return;
    }
    let gate = next_plan_reset.map_or(GateNote::Unknown, GateNote::Earliest);
    if let Some(e) = snap.overflow.iter().find(|e| &e.name == to) {
        out.emit(now(), &msg::per_token_reason(to, e.tier, gate, check));
    }
}

/// The `overflow:` line after a swap from `from` to `to`, if it moved onto
/// or off overflow; the `moved ~/.claude` line instead when the move was
/// onto the API-key profile (`onto_key`) or off a live API-key profile
/// (`off_platform`, issue #1700).
pub fn note_move(
    out: &mut Sink,
    tag: Option<Tag>,
    from: &ProfileName,
    to: &ProfileName,
    next_plan_reset: Option<i64>,
    off_platform: bool,
    onto_key: bool,
) {
    let t = now();
    match tag {
        Some(Tag::In) if onto_key => out.emit(t, &msg::onto_platform(to)),
        Some(Tag::In) => out.emit(t, &msg::onto_overflow(to, next_plan_reset)),
        Some(Tag::Out) if off_platform => out.emit(t, &msg::off_platform(from, to)),
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

/// Rewrites the no-capacity file from this cycle's readings: the live
/// profile's and every other candidate's, as measured. On credits it reads
/// `ok`, since work can go on. The API-key profile's synthetic reading
/// (issue #1700) is left out: it is never plan capacity.
pub fn refresh_capacity(
    root: &Path,
    live: Option<&ProfileName>,
    reading: &Reading,
    others: &[(ProfileName, Reading)],
    t: i64,
    tag: Option<Tag>,
    platform: Option<&ProfileName>,
) {
    let readings: Vec<(ProfileName, Rotated)> = rotated(live, reading, others)
        .into_iter()
        .filter(|(n, _)| Some(n) != platform)
        .collect();
    capacity_from_rotation(root, &readings, t, Tag::on_overflow(tag));
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

/// Records a known live profile and its account, so `profile usage` and the
/// next cycle read it from the ledger. An API-key profile (issue #1700) is
/// recorded with `live_kind` and no account: `$HOME/.claude.json` may still
/// hold the previous login's account, which is not this login's.
pub fn note_live(l: &mut RotateLedger, snap: &Snap, t: i64) {
    let LiveId::Known(n) = &snap.live else {
        return;
    };
    let account = if snap.platform_live() {
        None
    } else if let Some(key) = &snap.account {
        Some(key.clone())
    } else {
        return;
    };
    if l.live.as_deref() != Some(n.as_str()) {
        l.live = Some(n.to_string());
        l.since = Some(t);
    }
    l.account = account;
    l.live_kind = snap.platform_live().then(|| LIVE_KIND_PLATFORM.to_string());
}

/// Logs, once per process, each API-key profile that `overflow=` does not
/// mark (issue #1804) and each marked one that is not used because another
/// comes first by name.
pub fn log_ignored(out: &mut Sink, snap: &Snap, t: i64) {
    static IGNORED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut seen) = IGNORED.lock() else {
        return;
    };
    let lines = snap
        .unmarked
        .iter()
        .map(msg::platform_not_marked)
        .chain(snap.ignored.iter().map(msg::platform_not_used));
    for line in lines {
        if !seen.contains(&line) {
            out.emit(t, &line);
            seen.push(line);
        }
    }
}

/// The ALERT key kind and line for an alerted cycle: the blocked API key
/// with nowhere to go (issue #1700; it stays, with no swap), else no
/// capacity, which counts the failed and exhausted readings in `others`.
/// `reason` gives the block's reason, read only when needed.
pub fn alert_line(
    snap: &Snap,
    tag: Option<Tag>,
    live: &Reading,
    others: &[(ProfileName, Reading)],
    reason: impl FnOnce() -> String,
) -> (&'static str, String) {
    match &snap.live {
        LiveId::Known(n) if tag == Some(Tag::On) && snap.platform_live() => {
            ("platform-blocked", msg::alert_platform_stuck(&reason(), n))
        }
        id => {
            let name = live_name(id).map_or("-", ProfileName::as_str);
            (
                "no-capacity",
                msg::alert_no_capacity_for(name, live, others),
            )
        }
    }
}

/// This cycle's readings for the no-capacity file: the live profile's and
/// every other candidate's, as measured.
fn rotated(
    live: Option<&ProfileName>,
    reading: &Reading,
    others: &[(ProfileName, Reading)],
) -> Vec<(ProfileName, Rotated)> {
    let of = |r: &Reading| match *r {
        Reading::Measured { session, week } => Rotated::Measured { session, week },
        Reading::Exhausted => Rotated::Exhausted,
        Reading::Failed => Rotated::Failed,
    };
    live.map(|n| (n.clone(), of(reading)))
        .into_iter()
        .chain(others.iter().map(|(n, r)| (n.clone(), of(r))))
        .collect()
}
