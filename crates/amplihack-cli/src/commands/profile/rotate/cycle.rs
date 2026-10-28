//! One rotate cycle: prototype check, lock and recover, identify, measure
//! (unlocked), decide, revalidate under the lock and swap, record. See
//! "Rotate cycle" in docs/reference/profile-command.md.

use super::alert::{alert_key, alert_once, clear_alert, refusal_kind};
use super::decide::Decision;
use super::held::held_cycle;
use super::identify::{LiveId, Skip};
use super::ledger::{LastCycle, RotateLedger, RotateState, SWITCHED};
use super::log::{Sink, msg};
use super::observe::{
    Measurement, Snap, dry_run, live_name, measure_and_decide, reading_of, snapshot,
};
use super::overflow::Tag;
use super::overflow_record::{self, block_spend_limited, note_move, refresh_capacity, stay_note};
// Under an alias: `profile::overflow::Gate` (whether credits may be spent)
// is a different thing from the pin's gate (whether a `profile switch`
// holds the cycle).
use super::pin::{self, Gate as PinGate};
use super::prelude::{Prelude, locked_prelude};
use super::settings::Settings;
use super::swap::{self, Source};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Liveness, exists, now, scan_system};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::run::resolve_claude_binary;
use crate::commands::profile::shared_dirs;
use crate::commands::profile::store::ProfileStore;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub(super) const LOCK_WAIT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    OneShot,
    DryRun,
    Watch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleOutcome {
    Swapped,
    Stayed,
    Alerted,
    Skipped,
    /// A `profile switch` pin stopped the cycle before measuring (#1691).
    Held,
    /// `watch` received a signal and abandoned the cycle.
    Stopped,
}

impl CycleOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Swapped => "swapped",
            Self::Stayed => "stayed",
            Self::Alerted => "alerted",
            Self::Skipped => "skipped",
            Self::Held => "held",
            Self::Stopped => "stopped",
        }
    }
}

/// Steps 4 and 5 of a cycle: measure, then decide.
pub(super) type MeasureFn = fn(&Ctx, &Snap) -> Result<Measurement, ProfileError>;

/// What a cycle needs, and what `watch` remembers between cycles: the last
/// skips and pin line, so a `skip:` line is written only when it changes,
/// and the count of consecutive failed live probes for failed-probe damping.
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
    /// Finds the running sessions; `scan_system` outside tests. A test sets
    /// it to return `Liveness::Unsupported`, which is what every non-Linux
    /// system gets.
    pub(super) scan: fn(&Path) -> Liveness,
    /// Replaces steps 4 and 5 (find `claude`, measure, decide) when set. Only
    /// tests set it, to act while the cycle is measuring without the lock.
    pub(super) measure: Option<MeasureFn>,
    /// Replaces a held cycle's spend-limit probe when set; only tests do.
    pub(super) probe_held: Option<super::held::HeldProbeFn>,
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
            probe_held: None,
        }
    }

    pub(super) fn stopping(&self) -> bool {
        self.stop
            .as_ref()
            .is_some_and(|s| s.load(Ordering::Relaxed))
    }
}

/// Fails with `RotatePrototypeRunning` while the external rotator holds
/// `<root>/.rotator/lock`. Opens it read-only without following a symlink,
/// creating it or blocking on a FIFO, and releases the lock at once.
pub fn prototype_guard(root: &Path) -> Result<(), ProfileError> {
    use fs4::fs_std::FileExt;
    let path = root.join(".rotator").join("lock");
    let refuse = || ProfileError::RotateRefused(".rotator/lock is not a regular file".into());
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(refuse()),
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(refuse());
    }
    match FileExt::try_lock_exclusive(&file) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ProfileError::RotatePrototypeRunning),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            Err(ProfileError::RotatePrototypeRunning)
        }
        Err(e) => Err(e.into()),
    }
}

/// Writes an ALERT once per key and records the cycle as alerted.
fn alert(
    ctx: &Ctx,
    out: &mut Sink,
    kind: &str,
    live: Option<&ProfileName>,
    line: &str,
) -> Result<(), ProfileError> {
    let t = now();
    let mut l = ctx.rs.load_ledger()?.unwrap_or_default();
    alert_once(out, &mut l, &alert_key(kind, live), t, line);
    save(ctx, &mut l, CycleOutcome::Alerted, t, None, None)
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

/// Whether `last` was written after a cycle began measuring at
/// `measured_at`, so another writer used the ledger while that cycle held no
/// lock. A switch record always counts, whatever the clock says.
pub(super) fn newer_than(last: Option<&LastCycle>, measured_at: i64) -> bool {
    last.is_some_and(|c| c.outcome == SWITCHED || c.at >= measured_at)
}

/// Records a known live profile and its account, so `profile usage` and the
/// next cycle read it from the ledger.
pub(super) fn note_live(l: &mut RotateLedger, snap: &Snap, t: i64) {
    if let (LiveId::Known(n), Some(key)) = (&snap.live, &snap.account) {
        if l.live.as_deref() != Some(n.as_str()) {
            l.live = Some(n.to_string());
            l.since = Some(t);
        }
        l.account = Some(key.clone());
    }
}

pub fn run_cycle(ctx: &mut Ctx, mode: Mode) -> Result<CycleOutcome, ProfileError> {
    if mode == Mode::DryRun {
        return dry_run(ctx);
    }
    // Step 3: recover and identify under the lock.
    let (mut out, prelude) = locked_prelude(ctx, mode == Mode::OneShot)?;
    let (lock, state, first) = match prelude {
        Prelude::Ready { lock, state, first } => (lock, state, first),
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

    // Steps 4 and 5: measure without the lock, then decide. Recovery does
    // not need `claude`, so it is looked for only now.
    let Measurement {
        live,
        others,
        decision,
        note,
        failed,
        tag,
        new_blocks,
        next_plan_reset,
        measured_at,
    } = match ctx.measure {
        Some(stub) => stub(ctx, &first)?,
        None => {
            let binary = resolve_claude_binary()
                .map_err(|e| ProfileError::Io(std::io::Error::other(e.to_string())))?;
            measure_and_decide(ctx, &binary, &first)?
        }
    };
    ctx.failed_live = failed;
    out.emit(now(), &msg::cycle(live_name(&first.live), &live, &others));
    if ctx.stopping() {
        out.emit(now(), msg::STOPPED);
        return Ok(CycleOutcome::Stopped);
    }

    // Step 6: revalidate and swap under the lock.
    let Some(lock) = state.try_lock(LOCK_WAIT)? else {
        out.emit(now(), msg::LOCK_BUSY);
        return Ok(CycleOutcome::Skipped);
    };
    // The external rotator may have started while this one measured.
    prototype_guard(ctx.store.root())?;
    let t = now();
    if exists(&ctx.rs.journal) || snapshot(ctx, &state)? != first {
        out.emit(t, msg::CHANGED);
        let mut l = ctx.rs.load_ledger()?.unwrap_or_default();
        // A record written while this cycle measured unlocked (a `profile
        // switch`, or another cycle) is newer than anything this cycle knows.
        // Overwriting it with `skipped` would lose a switch's record, and the
        // held cycles after it would then write a bare `held`, so the
        // limit-resume hook would never wake sessions stopped before the
        // switch (#1691).
        if !newer_than(l.last_cycle.as_ref(), measured_at) {
            save(
                ctx,
                &mut l,
                CycleOutcome::Skipped,
                t,
                Some(measured_at),
                None,
            )?;
        }
        return Ok(CycleOutcome::Skipped);
    }
    let mut l = ctx.rs.load_ledger()?.unwrap_or_default();
    let stay_line = stay_note(&l, tag, live_name(&first.live), next_plan_reset);
    // Saved before any swap, which reloads the ledger from disk.
    if block_spend_limited(&mut out, &mut l, &new_blocks, t) {
        ctx.rs.save_ledger(&l)?;
    }
    let mut swapped = None;
    let outcome = match decision {
        Decision::Swap { to, reason } => {
            let source = match &first.live {
                LiveId::Known(n) => Source::Profile(n.clone()),
                _ => Source::Adopt,
            };
            let from = match swap::swap(&ctx.store, &ctx.rs, &ctx.home, source, &to, t) {
                Ok(from) => from,
                // A refusal repeats every cycle until a person acts: one
                // ALERT per reason, and the cycle is recorded.
                Err(e @ ProfileError::RotateRefused(_)) => {
                    let live = live_name(&first.live);
                    alert(
                        ctx,
                        &mut out,
                        &refusal_kind(&e),
                        live,
                        &format!("ALERT: {e}"),
                    )?;
                    return match mode {
                        Mode::Watch => Ok(CycleOutcome::Alerted),
                        _ => Err(e),
                    };
                }
                Err(e) => return Err(e),
            };
            swapped = Some([to.clone(), from.clone()]);
            let target = reading_of(&others, &to);
            out.emit(now(), &msg::swap(&from, &to, &live, &target, reason));
            note_move(&mut out, tag, &from, &to, next_plan_reset);
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
            // A damped stay is a stay: no alert applies, so it clears one.
            clear_alert(&mut l);
            CycleOutcome::Stayed
        }
        Decision::Alert => {
            note_live(&mut l, &first, t);
            let name = live_name(&first.live);
            let line = msg::alert_no_capacity_for(name.map_or("-", ProfileName::as_str), &live);
            alert_once(&mut out, &mut l, &alert_key("no-capacity", name), t, &line);
            CycleOutcome::Alerted
        }
    };
    refresh_capacity(
        ctx.store.root(),
        live_name(&first.live),
        &live,
        &others,
        t,
        tag,
    );
    save(ctx, &mut l, outcome, now(), Some(measured_at), tag)?;
    // Issue #1634: link the session dirs of both profiles swapped, after the
    // lock is released so a long merge never stalls a launch waiting on it.
    drop(lock);
    for p in swapped.iter().flatten() {
        shared_dirs::ensure_for(&ctx.store, p);
    }
    Ok(outcome)
}
