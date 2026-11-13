//! Decisions over retained observations; attempted candidates remain policy evidence.
use super::super::damp::{damp, is_failed_probe_swap, keep_for_home_key, next_failed};
use super::super::decide::Measured;
use super::super::key_check;
use super::super::overflow::OverflowInput;
use super::*;

/// Step 5 of a cycle: the overflow-aware decision on this cycle's probes,
/// the live profile's and each candidate's, measured from `measured_at`.
/// Tests that stub probing call this, so they run the real decision.
pub(in crate::commands::profile::rotate) fn decide_measured(
    ctx: &Ctx,
    snap: &Snap,
    probe: Probed,
    probes: Vec<(ProfileName, Probed)>,
    measured_at: i64,
) -> Result<Measurement, ProfileError> {
    decide_available(ctx, snap, probe, probes, measured_at, &[])
}

pub(in crate::commands::profile::rotate) fn decide_available(
    ctx: &Ctx,
    snap: &Snap,
    probe: Probed,
    probes: Vec<(ProfileName, Probed)>,
    measured_at: i64,
    attempted: &[ProfileName],
) -> Result<Measurement, ProfileError> {
    let measured: Vec<Measured> = probes
        .iter()
        .map(|(name, p)| Measured::new(name.clone(), p))
        .collect();
    let ledger = ctx.rs.load_ledger()?;
    let t = (ctx.now)();
    let blocked = |n: &ProfileName| {
        super::super::spending::refusal(&ctx.store, snap, ledger.as_ref(), n, t).is_some()
            || ledger.as_ref().is_some_and(|l| {
                snap.platform() == Some(n) && l.platform_block.as_ref().is_some_and(|b| t < b.until)
            })
    };
    let on_overflow =
        live_name(&snap.live).is_some_and(|n| snap.overflow.iter().any(|e| &e.name == n));
    let need = amplihack_utils::organisation_affinity::Need::of(
        on_overflow,
        !on_overflow && super::super::decide::needs_swap(&ctx.settings, &probe.reading),
    );
    let eligible: Vec<bool> = measured
        .iter()
        .map(|c| !c.spend_limit && !blocked(&c.name))
        .collect();
    // A ledger that cannot be read ends the cycle here, before any move onto
    // credits: its spend blocks are unknown. Only a missing ledger means no
    // blocks.
    let swapped_at = ledger.as_ref().and_then(|l| l.swapped_at);
    let platform_block = ledger.as_ref().and_then(|l| l.platform_block.clone());
    let blocks = ledger.map(|l| l.overflow_blocked).unwrap_or_default();
    // Issue #2218: an excluded live login is decided on as if no profile
    // owned it, so neither the hold, damping nor the home key keeps it, and
    // no synthetic reading of it is taken for a measured one.
    let live_name = snap.measured_live();
    let overflow = snap.decision_overflow();
    let is_overflow = |n: &ProfileName| overflow.iter().any(|e| &e.name == n);
    let live_over = live_name.is_some_and(is_overflow);
    let unmeasured_plan = snap.unmeasured_plan();
    let refused = snap.refused_plan();
    // Every rule runs on the live reading as measured.
    let input = OverflowInput {
        settings: &ctx.settings,
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
    let verdict = affinity::decide(snap, &input, &measured, &eligible, need, attempted);
    // Issue #2211: this cycle's outlook, for this live plan login only. It is
    // decided with the cycle's one `need`, worked out above on the reading as
    // measured, so a projected or substituted figure never makes a confirmed
    // organisation crossing, nor makes one sooner (`affinity::with_outlook`),
    // and `live::with_outlook` takes from it only a threshold swap or the
    // ALERT. The rules for leaving an overflow profile do not read the live
    // session figure, so an outlook there changes nothing.
    let outlook = ctx
        .live_outlook
        .as_ref()
        .filter(|o| !snap.platform_live() && !live_over && live_name == Some(&o.name));
    let verdict = match outlook {
        None => verdict,
        Some(o) => affinity::with_outlook(
            snap,
            &input,
            verdict,
            o,
            (&measured, &eligible),
            need,
            attempted,
        ),
    };
    // A spend limit on a live overflow profile is handled by its block, so
    // it does not count towards the failed live probes.
    let failed = if live_over && probe.spend_limit {
        ctx.failed_live
    } else {
        next_failed(ctx.failed_live, &probe.reading)
    };
    let (decision, note) = damp(
        verdict.decision,
        &probe.reading,
        failed,
        swapped_at,
        t,
        &ctx.settings,
    );
    // Only here is the file read: a swap on this ground is rare. The strict
    // reader, not `key_check::home_key`: a key Claude may not read must not
    // keep a dead login live, and the swap it would have stopped is harmless.
    let home_key = (is_failed_probe_swap(&decision, &probe.reading) && !snap.platform_live())
        .then(|| key_check::reachable(&ctx.home).ok())
        .flatten();
    let (decision, note) = keep_for_home_key(decision, note, home_key, &probe.reading);
    // A damped move off credits is a stay on them.
    let tag = match (&decision, verdict.tag) {
        (Decision::Stay, Some(Tag::Out)) => Some(Tag::On),
        (_, tag) => tag,
    };
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
    })
}
