//! One rotate cycle: prototype check, lock and recover, identify, measure
//! (unlocked), decide, revalidate under the lock and swap, record. See
//! "Rotate cycle" in docs/reference/profile-command.md.

use super::alert::{alert, alert_key, alert_once};
use super::capacity::{self, Room};
use super::decide::Decision;
use super::dry_run::dry_run;
use super::fresh::{self, Reread};
use super::held::held_cycle;
use super::history;
use super::identify::excluded_offline::{exclude_paused, unproven_live};
use super::ledger::{RotateLedger, newer_than};
use super::log::{msg, shown, shown_of};
use super::observe::{Measurement, Snap, live_name, measure_and_decide, platform_block_reason};
use super::overflow::Tag;
use super::overflow_record::{
    self, alert_line, block_spend_limited, note_home_key, note_live, note_move, note_per_token,
    refresh_capacity, stay_note, target_check,
};
use super::pin::{self, Gate as PinGate}; // the switch-hold gate
use super::plan_room::no_room;
pub use super::prelude::prototype_guard;
use super::prelude::{Prelude, locked_prelude};
use super::revalidate::{self, Verdict};

use super::swap_back;
use super::swap_record;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::shared_dirs;
use std::time::Duration;

pub(super) const LOCK_WAIT: Duration = Duration::from_secs(2);
#[path = "cycle_ctx.rs"]
mod ctx;
#[path = "cycle_outcome.rs"]
mod outcome;
pub use ctx::Ctx;
pub use outcome::{CycleOutcome, Mode};
/// Steps 4 and 5 of a cycle: measure, then decide.
pub(super) type MeasureFn = fn(&Ctx, &Snap) -> Result<Measurement, ProfileError>;

/// Records the cycle as ended at `t`, after measuring from `measured_at`,
/// with its overflow tag; see [`overflow_record::record`].
pub(super) fn save(
    ctx: &Ctx,
    l: &mut RotateLedger,
    o: CycleOutcome,
    t: i64,
    measured_at: Option<i64>,
    tag: Option<Tag>,
) -> Result<(), ProfileError> {
    overflow_record::record(&ctx.rs, l, o.as_str(), t, measured_at, tag)
}

pub fn run_cycle(ctx: &mut Ctx, mode: Mode) -> Result<CycleOutcome, ProfileError> {
    // Issue #1962: set for each run, so a real cycle never inherits it.
    ctx.read_only = mode == Mode::DryRun;
    if mode == Mode::DryRun {
        return dry_run(ctx);
    }
    // Step 3: recover and identify under the lock.
    let (mut out, prelude) = locked_prelude(ctx, mode == Mode::OneShot, true)?;
    let (lock, state, mut first) = match prelude {
        Prelude::Ready { lock, state, first } => (lock, state, *first),
        Prelude::LockBusy => {
            out.emit(now(), msg::LOCK_BUSY);
            return Ok(CycleOutcome::Skipped);
        }
        Prelude::AutoSwitchInconsistent => {
            let line = msg::AUTO_SWITCH_INCONSISTENT;
            alert(ctx, &mut out, "auto-switch", None, line)?;
            return Ok(CycleOutcome::Alerted);
        }
        Prelude::Inconsistent(reason) => {
            alert(
                ctx,
                &mut out,
                "inconsistent",
                None,
                &msg::inconsistent(&reason),
            )?;
            return match mode {
                Mode::Watch => Ok(CycleOutcome::Alerted),
                _ => Err(ProfileError::RotateInconsistent(reason)),
            };
        }
        Prelude::LiveUnusable { kind, live, line } => {
            if kind == "needs-login" {
                out.emit((ctx.now)(), &line);
                return Ok(CycleOutcome::Alerted);
            }
            alert(ctx, &mut out, kind, live.as_ref(), &line)?;
            if kind == "account_mismatch" && mode != Mode::Watch {
                let reason = line.strip_prefix("ALERT: swap refused: ").unwrap_or(&line);
                return Err(ProfileError::RotateRefused(reason.into()));
            }
            return Ok(CycleOutcome::Alerted);
        }
        // Issue #1962: every cycle, and nothing is saved over the ledger.
        Prelude::LedgerUnreadable { line } => {
            out.emit(now(), &format!("ALERT: {line}"));
            return Ok(CycleOutcome::Alerted);
        }
        // Issue #1693: the prelude already wrote the guidance.
        Prelude::Guided => {
            let mut l = ctx.rs.load_ledger()?.unwrap_or_default();
            save(ctx, &mut l, CycleOutcome::Alerted, now(), None, None)?;
            return Ok(CycleOutcome::Alerted);
        }
    };
    // Issue #1691: a `profile switch` pin stops the cycle here, before
    // `claude` is looked for, so nothing is measured while pinned. The pin
    // wins over every overflow rule (#1606): the overflow policy runs inside
    // the measurement, so a held cycle neither moves off a pinned overflow
    // profile nor moves a pinned plan login onto credits; `held_cycle` only
    // checks a pinned overflow profile for the spend limit. See "Pin and
    // overflow" in docs/reference/profile-command.md.
    let (line, held) = match pin::gate(&first.pin) {
        PinGate::Open => (None, false),
        PinGate::Ignored(line) => (Some(line), false),
        PinGate::Held(line) => (Some(line), true),
    };
    if let Some(line) = &line
        && ctx.last_gate.as_ref() != Some(line)
    {
        out.emit(now(), line);
    }
    ctx.last_gate = line;
    if held {
        held_cycle(ctx, &mut out, lock, &state, &first)?;
        return Ok(CycleOutcome::Held);
    }
    drop(lock);
    // Issue #2218: an unreadable exclude list pauses rotation.
    if let Some(alert) = exclude_paused(&ctx.store, |w| out.emit(now(), w)) {
        out.emit(now(), &format!("ALERT: {alert}"));
        return Ok(CycleOutcome::Alerted);
    }
    // Steps 4 and 5: measure without the lock, then decide. `claude` is
    // looked for only when a read falls back to it (issue #1844).
    let measuring = std::time::Instant::now();
    ctx.live_outlook = None;
    let m = match ctx.measure {
        Some(stub) => stub(ctx, &first)?,
        None => measure_and_decide(ctx, &first)?,
    };
    let (live_shown, others_shown) = shown(&m, first.live_excluded);
    let (name, platform) = (live_name(&first.live), first.shown_platform());
    out.emit(
        now(),
        &msg::cycle(name, &live_shown, &others_shown, platform),
    );
    // Issues #2211, #1870: before `failed_live` moves on, so it is counted once.
    let m = fresh::live::step(ctx, &first, m)?;
    let m = fresh::gate(ctx, &first, m, Reread::Allowed)?;
    ctx.identity_budget.exclude_measurement(measuring.elapsed());
    m.fresh_lines.iter().for_each(|line| out.emit(now(), line));
    let failed_before = std::mem::replace(&mut ctx.failed_live, m.failed);
    note_home_key(ctx, &mut out, &first);
    if ctx.stopping() {
        out.emit(now(), msg::STOPPED);
        return Ok(CycleOutcome::Stopped);
    }
    // Step 6: revalidate and swap under the lock, held until the swap.
    let Some(lock) = state.try_lock(LOCK_WAIT)? else {
        out.emit(now(), msg::LOCK_BUSY);
        return Ok(CycleOutcome::Skipped);
    };
    // The external rotator may have started while this one measured.
    prototype_guard(ctx.store.root())?;
    let t = now();
    let checked = revalidate::check(ctx, &state, &first, &m, failed_before)?;
    if checked.reread {
        out.emit(t, revalidate::REREAD);
    }
    let m = match checked.verdict {
        Verdict::Proceed => m,
        // Issue #1721: only the target changed, and the probes taken chose
        // another; the cycle is now that one. `ctx.failed_live` stays: the
        // live probe, live name and overflow entries are as they were, so
        // `retry.failed == m.failed`. Widen the retry rule, assign it here.
        Verdict::Reconsidered { m, fresh, line } => {
            out.emit(t, &line);
            first = *fresh;
            *m
        }
        Verdict::Skip(why) => {
            out.emit(t, &msg::changed(&why));
            let mut l = ctx.rs.load_ledger()?.unwrap_or_default();
            // A record written while this cycle measured unlocked (a `profile
            // switch`, or another cycle) is newer than anything this cycle
            // knows. Overwriting it with `skipped` would lose a switch's
            // record, and the held cycles after it would then write a bare
            // `held`, so the limit-resume hook would never wake sessions
            // stopped before the switch (#1691).
            if !newer_than(l.last_cycle.as_ref(), m.measured_at) {
                let at = Some(m.measured_at);
                save(ctx, &mut l, CycleOutcome::Skipped, t, at, None)?;
            }
            return Ok(CycleOutcome::Skipped);
        }
    };
    let (live_shown, others_shown) = shown(&m, first.live_excluded);
    let attempts_measurement = m.clone();
    let Measurement {
        live,
        others,
        decision,
        note,
        mut tag,
        new_blocks,
        next_plan_reset,
        unknown_plan,
        key_held,
        measured_at,
        probed,
        ..
    } = m;
    let mut l = ctx.rs.load_ledger()?.unwrap_or_default();
    let blocked = capacity::spend_blocked(&l, live_name(&first.live), &new_blocks, t);
    let room = Room::live(probed.as_ref(), &live, first.platform_live(), tag, blocked);
    let stay_line = stay_note(&l, tag, live_name(&first.live), next_plan_reset);
    // Saved before any swap, which reloads the ledger from disk.
    if block_spend_limited(&mut out, &mut l, &new_blocks, t) {
        ctx.rs.save_ledger(&l)?;
    }
    let mut swapped = None;
    let mut refusal = None;
    let mut uncertain = None;
    // Issue #2085: the swap record's time is when the swap finished, not the
    // cycle's `t`, which is taken before revalidation and the swap. A session
    // that stops on the outgoing login in between must read as stopped
    // before the swap, or the limit-resume hook judges the record stale.
    let mut swapped_at = t;
    // Issue #2218 (S4): an excluded live login named only by evidence that
    // may be stale is neither moved into that folder nor recorded live.
    let unproven = unproven_live(ctx, &first);
    let outcome = match decision {
        Decision::Swap { .. } => 'swap: {
            // An excluded live login may move only on established ownership.
            if let Some(n) = unproven {
                let key = alert_key("excluded_unproven", Some(n));
                alert_once(&mut out, &mut l, &key, t, &msg::alert_excluded_unproven(n));
                tag = None;
                break 'swap CycleOutcome::Alerted;
            }
            let success = match super::cycle_attempts::run(
                ctx,
                &state,
                &first,
                &attempts_measurement,
                &mut out,
                failed_before,
            ) {
                Ok(super::cycle_attempts::Attempts::Swapped(success)) => success,
                Err(error) => {
                    out.emit((ctx.now)(), &format!("swap recovery required: {error}"));
                    uncertain = Some(error);
                    l = ctx.rs.load_ledger()?.unwrap_or_default();
                    tag = None;
                    break 'swap CycleOutcome::Alerted;
                }
                Ok(super::cycle_attempts::Attempts::Refused(error)) => {
                    refusal = Some(error);
                    l = ctx.rs.load_ledger()?.unwrap_or_default();
                    note_live(&mut l, &first, t);
                    tag = None;
                    break 'swap CycleOutcome::Alerted;
                }
            };
            let (from, to, reason) = (success.from, success.to, success.reason);
            let check = target_check(ctx, &first, &to).map_err(ProfileError::RotateRefused)?;
            swapped_at = (ctx.now)();
            swapped = Some([to.clone(), from.clone()]);
            let target = shown_of(&others_shown, &to);
            if first.live_excluded {
                out.emit(now(), &msg::excluded_moving_off(&from));
            }
            let seen = (next_plan_reset, unknown_plan.len());
            note_per_token(&mut out, &first, tag, &to, seen, check);
            out.emit(now(), &msg::swap(&from, &to, &live_shown, &target, reason));
            if let Some(line) = amplihack_utils::organisation_affinity::Affinity::between(
                first.source_org(),
                first.org_of(&to),
            )
            .log_line(from.as_str(), to.as_str())
            {
                out.emit(now(), &line);
            }
            let (off_key, onto_key) = (first.platform_live(), first.platform() == Some(&to));
            note_move(
                &mut out,
                tag,
                &from,
                &to,
                next_plan_reset,
                off_key,
                onto_key,
            );
            ctx.failed_live = 0;
            ctx.live_series.clear();
            l = ctx.rs.load_ledger()?.unwrap_or_default();
            CycleOutcome::Swapped
        }
        Decision::Stay => {
            if let Some(n) = &note {
                out.emit(t, n);
            }
            if let Some(line) = &stay_line {
                out.emit(t, line);
            }
            note_live(&mut l, &first, t);
            // Issue #1907: capacity only on a fresh reading below 100%.
            room.stay(&mut l)
        }
        Decision::Alert => {
            if unproven.is_none() {
                note_live(&mut l, &first, t);
            }
            let reason = || platform_block_reason(ctx);
            let (kind, line) = alert_line(&first, tag, &live, &others, &key_held, reason);
            let key = alert_key(kind, live_name(&first.live));
            room.alert(&mut out, &mut l, kind, &key, t, &line)
        }
    };
    // Issue #2218: nothing measured is recorded under an excluded live
    // login's name; it was never probed.
    let recorded_live = live_name(&first.live).filter(|_| !first.live_excluded);
    refresh_capacity(
        ctx,
        recorded_live,
        &live,
        &others,
        t,
        tag,
        first.shown_platform(),
    );
    no_room::record(
        &mut l,
        ctx,
        &first,
        (&live, &others),
        probed.as_ref(),
        measured_at,
    );
    let left = swapped
        .as_ref()
        .filter(|_| !first.live_excluded)
        .map(|[_, from]| (from, &live));
    let (s, live_n) = (&ctx.settings, recorded_live);
    for line in swap_back::update(&mut l, s, live_n, probed.as_ref(), left, measured_at, t) {
        out.emit(now(), &line);
    }
    let at = capacity::recorded_at(outcome, probed.as_ref(), measured_at);
    save(ctx, &mut l, outcome, now(), Some(at), tag)?;
    // Issue #2085: the record the limit-resume hook waits for, under the lock.
    if swapped.is_some()
        && let Some(mut j) = ctx.rs.load_journal()?
    {
        if let Err(error) = swap_record::complete(&ctx.rs, &mut j, swapped_at) {
            out.emit(
                (ctx.now)(),
                &format!("swap event publication pending: {error}"),
            );
            return Err(error);
        }
    }
    // Issue #1634: link the session dirs of both profiles swapped, after the
    // lock is released so a long merge never stalls a launch waiting on it.
    drop(lock);
    // Issue #1797: one usage-history line per measured cycle, after the lock.
    if let Some(p) = &probed {
        history::record(ctx.store.root(), &mut out, &first, p, measured_at);
    }
    for p in swapped.iter().flatten() {
        shared_dirs::ensure_for(&ctx.store, p);
    }
    if mode != Mode::Watch
        && let Some(error) = refusal
    {
        return Err(error);
    }
    if let Some(error) = uncertain {
        return Err(error);
    }
    Ok(outcome)
}
