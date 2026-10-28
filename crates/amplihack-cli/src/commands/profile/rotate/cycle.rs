//! One rotate cycle: prototype check, lock and recover, identify, measure
//! (unlocked), decide, revalidate under the lock and swap, record. See
//! "Rotate cycle" in docs/reference/profile-command.md.

use super::decide::{Decision, Reading};
use super::identify::{LiveId, Skip};
use super::ledger::{
    LastCycle, RotateLedger, RotateState, SWITCHED, alert_key, clear_alert, should_alert,
};
use super::log::{Sink, msg};
use super::observe::{
    Measurement, Snap, dry_run, live_name, measure_and_decide, reading_of, snapshot,
};
use super::pin::{self, Gate};
use super::prelude::{Prelude, locked_prelude};
use super::settings::Settings;
use super::swap::{self, Source};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    Liveness, Rotated, capacity_from_rotation, exists, now, scan_system,
};
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
        }
    }

    fn stopping(&self) -> bool {
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
    record(ctx, &mut l, CycleOutcome::Alerted, t, None)
}

/// Logs the ALERT only when its key changed; the one-shot command still
/// prints it every time, so a repeated run does not look like success.
fn alert_once(out: &mut Sink, l: &mut RotateLedger, key: &str, t: i64, line: &str) {
    if should_alert(l, key) {
        out.emit(t, line);
    } else {
        out.print(t, line);
    }
}

/// Records the cycle as ended at `t`, after measuring from `measured_at`.
fn record(
    ctx: &Ctx,
    l: &mut super::ledger::RotateLedger,
    o: CycleOutcome,
    t: i64,
    measured_at: Option<i64>,
) -> Result<(), ProfileError> {
    l.last_cycle = Some(LastCycle {
        at: t,
        measured_at,
        outcome: o.as_str().into(),
    });
    ctx.rs.save_ledger(l)
}

/// The ALERT key kind for a refused swap: its reason, without control
/// characters and short enough that the key passes the ledger's 320-byte
/// check with any profile name.
fn refusal_kind(e: &ProfileError) -> String {
    let reason: String = e.to_string().chars().filter(|c| !c.is_control()).collect();
    let mut kind = String::from("refused ");
    for c in reason.chars() {
        if kind.len() + c.len_utf8() > 200 {
            break;
        }
        kind.push(c);
    }
    kind
}

/// Records a known live profile and its account, so `profile usage` and the
/// next cycle read it from the ledger.
fn note_live(l: &mut RotateLedger, snap: &Snap, t: i64) {
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
    // `claude` is looked for, so nothing is measured while pinned.
    let (line, held) = match pin::gate(&first.pin) {
        Gate::Open => (None, false),
        Gate::Ignored(line) => (Some(line), false),
        Gate::Held(line) => (Some(line), true),
    };
    if let Some(line) = &line
        && ctx.last_gate.as_ref() != Some(line)
    {
        out.emit(now(), line);
    }
    ctx.last_gate = line;
    if held {
        let t = now();
        let mut l = ctx.rs.load_ledger()?.unwrap_or_default();
        note_live(&mut l, &first, t);
        clear_alert(&mut l);
        if l.last_cycle.as_ref().is_some_and(|c| c.outcome == SWITCHED) {
            // The switch's record stands for the whole hold: it is what lets
            // the limit-resume hook wake sessions stopped before the switch.
            ctx.rs.save_ledger(&l)?;
        } else {
            // Nothing was measured, so no `measured_at` (#1659), and `held`
            // is not capacity: a hold with no switch record (a pin written
            // by hand or by an older build) wakes no session.
            record(ctx, &mut l, CycleOutcome::Held, t, None)?;
        }
        return Ok(CycleOutcome::Held);
    }
    drop(lock);

    // Steps 4 and 5: measure without the lock, then decide. Recovery does
    // not need `claude`, so it is looked for only now.
    let binary = resolve_claude_binary()
        .map_err(|e| ProfileError::Io(std::io::Error::other(e.to_string())))?;
    let Measurement {
        live,
        others,
        decision,
        note,
        failed,
        measured_at,
    } = measure_and_decide(ctx, &binary, &first)?;
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
        record(ctx, &mut l, CycleOutcome::Skipped, t, Some(measured_at))?;
        return Ok(CycleOutcome::Skipped);
    }
    let mut l = ctx.rs.load_ledger()?.unwrap_or_default();
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
            ctx.failed_live = 0;
            l = ctx.rs.load_ledger()?.unwrap_or_default();
            CycleOutcome::Swapped
        }
        Decision::Stay => {
            if let Some(n) = &note {
                out.emit(t, n);
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
    capacity_from_rotation(
        ctx.store.root(),
        &rotated(live_name(&first.live), &live, &others),
        t,
    );
    record(ctx, &mut l, outcome, now(), Some(measured_at))?;
    // Issue #1634: link the session dirs of both profiles swapped, after the
    // lock is released so a long merge never stalls a launch waiting on it.
    drop(lock);
    for p in swapped.iter().flatten() {
        shared_dirs::ensure_for(&ctx.store, p);
    }
    Ok(outcome)
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
