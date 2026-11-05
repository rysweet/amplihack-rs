//! One rotate cycle: prototype check, lock and recover, identify, measure
//! (unlocked), decide, revalidate under the lock and swap, record. See
//! "Rotate cycle" in docs/reference/profile-command.md.

use super::alert::{alert, alert_key, refusal_kind};
use super::capacity::{self, Room};
use super::cred_identity::{DEFAULT_READ, ReadFn};
use super::decide::Decision;
use super::dry_run::dry_run;
use super::fresh::{self, Reread, RereadFn};
use super::held::held_cycle;
use super::history;
use super::identify::{LiveId, Skip};
use super::key_check::{self, KeyCheckFn};
use super::ledger::{RotateLedger, RotateState, newer_than};
use super::log::{msg, shown, shown_of};
use super::observe::{Measurement, Snap, live_name, measure_and_decide, platform_block_reason};
use super::overflow::Tag;
use super::overflow_record::{
    self, alert_line, block_spend_limited, note_home_key, note_live, note_move, note_per_token,
    refresh_capacity, refuse_key, stay_note, target_check,
};
use super::pin::{self, Gate as PinGate}; // the switch-hold gate
use super::plan_room::no_room;
pub use super::prelude::prototype_guard;
use super::prelude::{Prelude, locked_prelude};
use super::revalidate::{self, Verdict};
use super::settings::Settings;
use super::swap::{self, Source};
use super::swap_back;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Liveness, now, scan_system};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::shared_dirs;
use crate::commands::profile::store::ProfileStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub(super) const LOCK_WAIT: Duration = Duration::from_secs(2);
#[path = "cycle_outcome.rs"]
mod outcome;
pub use outcome::{CycleOutcome, Mode};
/// Steps 4 and 5 of a cycle: measure, then decide.
pub(super) type MeasureFn = fn(&Ctx, &Snap) -> Result<Measurement, ProfileError>;

/// Cycle state and the watch process's retained readings.
pub struct Ctx {
    pub store: ProfileStore,
    pub rs: RotateState,
    pub home: PathBuf,
    pub settings: Settings,
    pub stop: Option<Arc<AtomicBool>>,
    pub(super) last_skips: Option<Vec<(ProfileName, Skip)>>,
    pub(super) last_proto: Option<&'static str>,
    last_gate: Option<String>,
    pub(super) failed_live: u32,
    /// Finds running sessions; tests inject synthetic liveness.
    pub(super) scan: fn(&Path) -> Liveness,
    /// Tests inject measurement while the lock is released.
    pub(super) measure: Option<MeasureFn>,
    /// Freshness re-read by the configured method; None reads nothing.
    pub(super) reread: Option<RereadFn>,
    /// Replaces a held cycle's spend-limit probe when set; only tests do.
    pub(super) probe_held: Option<super::held::HeldProbeFn>,
    /// Waits before step 6 reads the state again; tests act in it instead.
    pub(super) pause: fn(&Ctx),
    /// Login check before a move onto the API-key profile.
    pub(super) key_check: KeyCheckFn,
    /// The `$HOME` key setting `note_home_key` last logged.
    pub(super) home_key: Option<&'static str>,
    /// Usage reads carried across watch cycles.
    pub(super) in_flight: Option<std::sync::Mutex<super::measure::Reads>>,
    /// Bounded login identity reader, injected by tests.
    pub(super) identity: ReadFn,
    pub(super) identity_budget: super::login_identity::Budget,
}

impl Ctx {
    pub fn new(store: ProfileStore, home: PathBuf, settings: Settings) -> Self {
        let rs = RotateState::new(store.root());
        Self {
            store,
            rs,
            home,
            settings,
            stop: None,
            last_skips: None,
            last_proto: None,
            last_gate: None,
            failed_live: 0,
            scan: scan_system,
            measure: None,
            reread: None,
            probe_held: None,
            pause: |_| std::thread::sleep(revalidate::REREAD_PAUSE),
            key_check: key_check::reachable,
            home_key: None,
            in_flight: None,
            identity: DEFAULT_READ,
            identity_budget: super::login_identity::Budget::new(),
        }
    }

    pub(super) fn stopping(&self) -> bool {
        self.stop
            .as_ref()
            .is_some_and(|s| s.load(Ordering::Relaxed))
    }
}

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
            alert(ctx, &mut out, kind, live.as_ref(), &line)?;
            if kind == "account_mismatch" && mode != Mode::Watch {
                let reason = line.strip_prefix("ALERT: swap refused: ").unwrap_or(&line);
                return Err(ProfileError::RotateRefused(reason.into()));
            }
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
    // Steps 4 and 5: measure without the lock, then decide. `claude` is
    // looked for only when a read falls back to it (issue #1844).
    let measuring = std::time::Instant::now();
    let m = match ctx.measure {
        Some(stub) => stub(ctx, &first)?,
        None => measure_and_decide(ctx, &first)?,
    };
    let (live_shown, others_shown) = shown(&m);
    let (name, platform) = (live_name(&first.live), first.shown_platform());
    out.emit(
        now(),
        &msg::cycle(name, &live_shown, &others_shown, platform),
    );
    // Issue #1870: before `failed_live` moves on, so it is counted once.
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
    let (live_shown, others_shown) = shown(&m);
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
    let outcome = match decision {
        Decision::Swap { to, reason } => 'swap: {
            // Issue #1804: a refusal moves nothing and writes no journal.
            let check = match target_check(ctx, &first, &to) {
                Ok(check) => check,
                Err(why) => {
                    refuse_key(&mut out, &mut l, &to, &why, t);
                    note_live(&mut l, &first, t);
                    tag = None;
                    break 'swap CycleOutcome::Alerted;
                }
            };
            let source = match &first.live {
                LiveId::Known(n) => Source::Profile(n.clone()),
                _ => Source::Adopt,
            };
            let from = match swap::swap_with(
                &ctx.store,
                &ctx.rs,
                &ctx.home,
                source,
                &to,
                t,
                ctx.identity_reader(),
                Some(&first),
            ) {
                Ok(from) => from,
                // A refusal repeats each cycle: one ALERT per reason.
                Err(e @ ProfileError::RotateRefused(_)) => {
                    // Issue #1700: a login without OAuth is refused once per
                    // profile, keyed by the profile it would have made live.
                    let (kind, live) = if swap::is_no_oauth(&e) {
                        ("no_oauth".to_string(), Some(&to))
                    } else if swap::is_account_mismatch(&e) {
                        // Keep the outgoing profile's label on repeated refusals.
                        ("account_mismatch".to_string(), live_name(&first.live))
                    } else {
                        (refusal_kind(&e), live_name(&first.live))
                    };
                    alert(ctx, &mut out, &kind, live, &format!("ALERT: {e}"))?;
                    return match mode {
                        Mode::Watch => Ok(CycleOutcome::Alerted),
                        _ => Err(e),
                    };
                }
                Err(e) => return Err(e),
            };
            swapped = Some([to.clone(), from.clone()]);
            let target = shown_of(&others_shown, &to);
            let seen = (next_plan_reset, unknown_plan.len());
            note_per_token(&mut out, &first, tag, &to, seen, check);
            out.emit(now(), &msg::swap(&from, &to, &live_shown, &target, reason));
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
            note_live(&mut l, &first, t);
            let reason = || platform_block_reason(ctx);
            let (kind, line) = alert_line(&first, tag, &live, &others, &key_held, reason);
            let key = alert_key(kind, live_name(&first.live));
            room.alert(&mut out, &mut l, kind, &key, t, &line)
        }
    };
    refresh_capacity(
        ctx,
        live_name(&first.live),
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
    let left = swapped.as_ref().map(|[_, from]| (from, &live));
    let (s, live_n) = (&ctx.settings, live_name(&first.live));
    for line in swap_back::update(&mut l, s, live_n, probed.as_ref(), left, measured_at, t) {
        out.emit(now(), &line);
    }
    let at = capacity::recorded_at(outcome, probed.as_ref(), measured_at);
    save(ctx, &mut l, outcome, now(), Some(at), tag)?;
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
    Ok(outcome)
}
