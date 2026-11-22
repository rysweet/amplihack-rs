//! Step 5 of a cycle: the overflow-aware decision on the cycle's probes
//! (moved from `observe.rs` for the line budget). Issue #2910: decided with
//! the cycle's [`Extra`] (probe labels and last good readings), on the one
//! target rule, and with what the cycle saw of each login kept in
//! [`Measurement::seen`] for its lines. The decision opens no file but the
//! ledger: the history is read once a cycle, at the edge (`Extra::for_cycle`).

use super::super::cycle::Ctx;
use super::super::damp::{damp, is_failed_probe_swap, keep_for_home_key, next_failed};
use super::super::decide::target::{self, LiveRoom, NoMove};
use super::super::decide::target_seen::{self, Extra, reading_used};
use super::super::decide::{Decision, Measured, Probed, Reading, Reason, needs_swap};
use super::super::key_check;
use super::super::overflow::{OverflowInput, Tag};
use super::{Measurement, Snap, affinity, live_name};
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use amplihack_utils::organisation_affinity::{Affinity, Need};

/// Step 5 of a cycle: the overflow-aware decision on this cycle's probes,
/// the live profile's and each candidate's, measured from `measured_at`,
/// with nothing substituted. Tests that stub probing call this, so they run
/// the real decision.
#[cfg(test)]
pub(in crate::commands::profile::rotate) fn decide_measured(
    ctx: &Ctx,
    snap: &Snap,
    probe: Probed,
    probes: Vec<(ProfileName, Probed)>,
    measured_at: i64,
) -> Result<Measurement, ProfileError> {
    decide_with(ctx, snap, probe, probes, measured_at, &Extra::default())
}

/// The reading of a login with an active recorded block: no room (issue
/// #2890, `overflow_blocked::no_room`).
const NO_ROOM: Reading = Reading::Exhausted {
    session: None,
    week: None,
};

/// [`decide_measured`] with the cycle's `extra`. A candidate whose probe said
/// nothing and that has a last good reading is decided on that reading; it
/// stays unknown, so it still keeps work off the API key, and its probe (in
/// `probed`, `others`, the history and the capacity file) is never changed.
pub(in crate::commands::profile::rotate) fn decide_with(
    ctx: &Ctx,
    snap: &Snap,
    probe: Probed,
    probes: Vec<(ProfileName, Probed)>,
    measured_at: i64,
    extra: &Extra,
) -> Result<Measurement, ProfileError> {
    let measured: Vec<Measured> = probes
        .iter()
        .map(|(name, p)| {
            let mut m = Measured::new(name.clone(), p);
            m.reading = reading_used(extra, name, p);
            m
        })
        .collect();
    let s = &ctx.settings;
    let ledger = ctx.rs.load_ledger()?;
    let t = (ctx.now)();
    let blocked = |n: &ProfileName| {
        ledger.as_ref().is_some_and(|l| {
            l.overflow_blocked
                .get(n.as_str())
                .is_some_and(|b| t < b.until)
                || (snap.platform() == Some(n)
                    && l.platform_block.as_ref().is_some_and(|b| t < b.until))
        })
    };
    let on_overflow =
        live_name(&snap.live).is_some_and(|n| snap.overflow.iter().any(|e| &e.name == n));
    // Issue #2218: an excluded live login is decided on as if no profile
    // owned it, so neither the hold, damping nor the home key keeps it, and
    // no synthetic reading of it is taken for a measured one.
    let live_name = snap.measured_live();
    let overflow = snap.decision_overflow();
    let is_overflow = |n: &ProfileName| overflow.iter().any(|e| &e.name == n);
    let live_over = live_name.is_some_and(is_overflow);
    // E11: a live plan login with an active block has no room, whatever its
    // probe reads, so the move needs no margin and may cross.
    let live_blocked = live_name.is_some_and(|n| !is_overflow(n) && blocked(n));
    let live_reading = if live_blocked { NO_ROOM } else { probe.reading };
    let need = Need::of(on_overflow, !on_overflow && needs_swap(s, &live_reading));
    let room = target::live_room(&probe.reading, live_blocked);
    let may_cross = match need {
        Need::LeaveOverflow => true,
        Need::Relief => room == LiveRoom::NoRoom,
        Need::Upgrade => false,
    };
    let eligible: Vec<bool> = measured
        .iter()
        .map(|c| !c.spend_limit && !blocked(&c.name))
        .collect();
    // A ledger that cannot be read ends the cycle here, before any move onto
    // credits: its spend blocks are unknown. Only a missing ledger means no
    // blocks.
    let swapped_at = ledger.as_ref().and_then(|l| l.swapped_at);
    let platform_block = ledger.as_ref().and_then(|l| l.platform_block.clone());
    let blocks = ledger
        .as_ref()
        .map(|l| l.overflow_blocked.clone())
        .unwrap_or_default();
    let unmeasured_plan = snap.unmeasured_plan();
    let refused = snap.refused_plan();
    // Every rule runs on the live reading as measured.
    let input = OverflowInput {
        settings: s,
        live_name,
        live: &probe,
        candidates: &measured,
        unmeasured_plan: &unmeasured_plan,
        refused: &refused,
        overflow: &overflow,
        blocks: &blocks,
        platform_block: platform_block.as_ref(),
        swapped_at,
        now: t,
    };
    let verdict = affinity::decide(snap, &input, &measured, &eligible, may_cross);
    // Issue #2211: this cycle's outlook, for this live plan login only. A
    // projected or substituted figure never makes a confirmed crossing
    // (`affinity::with_outlook`), and `live::with_outlook` takes from it only
    // a threshold swap or the ALERT. The rules for leaving an overflow
    // profile do not read the live session figure, so an outlook there
    // changes nothing.
    let outlook = ctx
        .live_outlook
        .as_ref()
        .filter(|o| !snap.platform_live() && !live_over && live_name == Some(&o.name));
    let alert_before = verdict.decision == Decision::Alert;
    let verdict = match outlook {
        None => verdict,
        Some(o) => affinity::with_outlook(snap, &input, verdict, o, &measured, &eligible),
    };
    // A spend limit on a live overflow profile is handled by its block, so
    // it does not count towards the failed live probes.
    let failed = if live_over && probe.spend_limit {
        ctx.failed_live
    } else {
        next_failed(ctx.failed_live, &probe.reading)
    };
    let (decision, note) = damp(verdict.decision, &probe.reading, failed, swapped_at, t, s);
    // Only here is the file read: a swap on this ground is rare. The strict
    // reader, not `key_check::home_key`: a key Claude may not read must not
    // keep a dead login live, and the swap it would have stopped is harmless.
    let home_key = (is_failed_probe_swap(&decision, &probe.reading) && !snap.platform_live())
        .then(|| key_check::reachable(&ctx.home).ok())
        .flatten();
    let (decision, note) = keep_for_home_key(decision, note, home_key, &probe.reading);
    // D15: a relief move onto a login past a threshold is a plan-room swap.
    let decision = match decision {
        Decision::Swap {
            to,
            reason: Reason::Threshold,
        } => {
            let reason = measured
                .iter()
                .find(|c| c.name == to)
                .map_or(Reason::Threshold, |c| target::swap_reason(s, &c.reading));
            Decision::Swap { to, reason }
        }
        d => d,
    };
    // A damped move off credits is a stay on them.
    let tag = match (&decision, verdict.tag) {
        (Decision::Stay, Some(Tag::Out)) => Some(Tag::On),
        (_, tag) => tag,
    };
    let mut seen = target_seen::build(snap, extra.clone(), &probes, &blocks, s.target_week_max, t);
    seen.live_no_room = !on_overflow && !live_over && room == LiveRoom::NoRoom;
    seen.may_cross = may_cross;
    if decision == Decision::Alert {
        // Why no move: on the logins the rotator may move to (plan logins it
        // may reach by organisation, a blocked one with no room), and on the
        // live figures the ALERT came from.
        let source = snap.source_org();
        let reachable = |c: &&Measured| {
            may_cross || Affinity::between(source, snap.org_of(&c.name)) != Affinity::Crossing
        };
        let movable: Vec<Measured> = measured
            .iter()
            .filter(|c| !is_overflow(&c.name))
            .filter(reachable)
            .map(|c| Measured {
                reading: if blocked(&c.name) { NO_ROOM } else { c.reading },
                ..c.clone()
            })
            .collect();
        let why_live = match outlook {
            Some(o) if !alert_before => o.reading(),
            _ => live_reading,
        };
        let mut why = target::no_move(s, &why_live, &movable, swapped_at, t);
        // S1: only a login in another organisation could take the session,
        // and a live login with room never crosses. A stay, not the
        // no-target line: nothing the rotator may move to has the room.
        if let (NoMove::NoTarget, LiveRoom::Room(r)) = (&why, room)
            && seen.logins.values().any(|st| st.fit.is_ok())
        {
            why = NoMove::LessRoom {
                most: 0,
                room: r,
                needs: target::relief_floor(room, s.margin),
            };
        }
        seen.why = Some(why);
    }
    let others = probes.iter().map(|(n, p)| (n.clone(), p.reading)).collect();
    // `probed` is always `Some` here: `Room::of` treats `None` (test stubs
    // only) as clean.
    Ok(Measurement {
        live: probe.reading,
        others,
        probed: Some((probe, probes)),
        decision,
        note,
        failed,
        tag,
        new_blocks: verdict.new_blocks,
        next_plan_reset: verdict.next_plan_reset,
        unknown_plan: verdict.unknown_plan,
        key_held: verdict.key_held,
        measured_at,
        fresh_lines: Vec::new(),
        seen,
    })
}
