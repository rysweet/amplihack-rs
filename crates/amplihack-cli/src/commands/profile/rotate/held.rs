//! A cycle held by a `profile switch` pin (#1691), and its spend-limit check
//! (#1606). The pin stops every rotator move, but not that check: while the
//! pin holds the live login on an overflow profile with no active spend
//! block, each held cycle probes that login, and a spend limit blocks it
//! with one ALERT that names `profile switch --none`. See "Rotate pin" and
//! "Pin and overflow" in docs/reference/profile-command.md, and
//! `tests_pin_overflow`.

use super::alert::clear_alert;
use super::cycle::{Ctx, CycleOutcome, LOCK_WAIT, prototype_guard, save};
use super::decide::Decision;
use super::identify::LiveId;
use super::ledger::{RotateLedger, SWITCHED, active_blocks};
use super::log::{Sink, msg};
use super::measure::{self, Live};
use super::observe::{Snap, snapshot};
use super::overflow_record::block_pinned_spend_limited;
use super::overflow_record::note_live;
use super::pin::PinState;
use super::revalidate::changed;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Lock, State, exists, now};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::run::resolve_claude_binary;

/// The spend-limit probe of a held cycle: whether the live login, the
/// pinned overflow profile named, reported a spend limit.
pub(super) type HeldProbeFn = fn(&Ctx, &ProfileName) -> Result<bool, ProfileError>;

/// Records a held cycle under `lock`, then releases it and, for a pinned
/// live overflow profile with no active block, runs the spend-limit check.
/// Measures nothing else and never swaps.
pub(super) fn held_cycle(
    ctx: &Ctx,
    out: &mut Sink,
    lock: Lock,
    state: &State,
    first: &Snap,
) -> Result<(), ProfileError> {
    let t = now();
    let mut l = ctx.rs.load_ledger()?.unwrap_or_default();
    note_live(&mut l, first, t);
    clear_alert(&mut l);
    let watch = pinned_overflow_unblocked(first, &l, t);
    if l.last_cycle.as_ref().is_some_and(|c| c.outcome == SWITCHED) {
        // The switch's record stands for the whole hold: it is what lets
        // the limit-resume hook wake sessions stopped before the switch.
        ctx.rs.save_ledger(&l)?;
    } else {
        // Nothing was measured, so no `measured_at` (#1659), and `held`
        // is not capacity: a hold with no switch record (a pin written
        // by hand or by an older build) wakes no session.
        save(ctx, &mut l, CycleOutcome::Held, t, None, None)?;
    }
    drop(lock);
    match watch {
        Some(name) => watch_pinned_spend(ctx, out, state, first, &name),
        None => Ok(()),
    }
}

/// The pinned profile, when the pin holds the live login on an overflow
/// profile that has no active spend block: a held cycle probes it for the
/// spend limit, and nothing else.
fn pinned_overflow_unblocked(snap: &Snap, l: &RotateLedger, t: i64) -> Option<ProfileName> {
    let PinState::Pinned(p) = &snap.pin else {
        return None;
    };
    let live = matches!(&snap.live, LiveId::Known(n) if n == p);
    let overflow = snap.overflow.iter().any(|e| &e.name == p);
    (live && overflow && !active_blocks(l, t).contains_key(p.as_str())).then(|| p.clone())
}

/// A held cycle on a pinned overflow profile (#1606 with #1691): the pin
/// stops every move, but not the spend-limit check, so a login held on
/// credits that run out is still blocked with one ALERT, which names
/// `profile switch --none`. Probes the live login only, without the lock,
/// as a measuring cycle does; then, under the lock and only if nothing a
/// stay depends on changed meanwhile, records the block. Never swaps. A
/// probe that fails or reads anything but a spend limit records nothing.
fn watch_pinned_spend(
    ctx: &Ctx,
    out: &mut Sink,
    state: &State,
    first: &Snap,
    name: &ProfileName,
) -> Result<(), ProfileError> {
    let spend_limit = match ctx.probe_held {
        Some(stub) => stub(ctx, name)?,
        None => {
            let binary = resolve_claude_binary()
                .map_err(|e| ProfileError::Io(std::io::Error::other(e.to_string())))?;
            let (probe, _) = measure::measure(&ctx.store, &binary, Live::Profile(name), &[], now());
            probe.spend_limit
        }
    };
    if !spend_limit || ctx.stopping() {
        return Ok(());
    }
    let Some(_lock) = state.try_lock(LOCK_WAIT)? else {
        out.emit(now(), msg::LOCK_BUSY);
        return Ok(());
    };
    prototype_guard(ctx.store.root())?;
    // Only what the live login's block depends on, as for a stay (#1721):
    // another profile's churn does not stop it.
    let fresh = snapshot(ctx, state)?;
    let why = changed(first, &fresh, &Decision::Stay, exists(&ctx.rs.journal));
    if !why.is_empty() {
        // The probe may have read another login; the next cycle probes again.
        out.emit(now(), &msg::changed(&why));
        return Ok(());
    }
    let t = now();
    let mut l = ctx.rs.load_ledger()?.unwrap_or_default();
    if block_pinned_spend_limited(out, &mut l, name, t) {
        ctx.rs.save_ledger(&l)?;
    }
    Ok(())
}
