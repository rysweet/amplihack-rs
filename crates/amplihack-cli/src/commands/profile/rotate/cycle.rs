//! One rotate cycle: prototype check, lock and recover, identify, measure
//! (unlocked), decide, revalidate under the lock and swap, record. See
//! "Rotate cycle" in docs/reference/profile-command.md.

use super::decide::Decision;
use super::identify::{LiveId, Skip};
use super::ledger::{RotateLedger, RotateState, alert_key, clear_alert, should_alert};
use super::log::{Sink, msg, open_or_warn};
use super::observe::{
    Measurement, Snap, dry_run, live_alert, live_name, measure_and_decide, platform_block_reason,
    reading_of, snapshot,
};
use super::overflow_record::{
    alert_line, block_spend_limited, log_ignored, note_live, note_move, record, refresh_capacity,
    stay_note,
};
use super::settings::Settings;
use super::swap::{self, RecoverOutcome, Source};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, exists, now, recover_auto_switch};
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
            Self::Stopped => "stopped",
        }
    }
}

/// What a cycle needs, and what `watch` remembers between cycles: the last
/// skips, so a `skip:` line is written only when it changes, and the count
/// of consecutive failed live probes for failed-probe damping.
pub struct Ctx {
    pub store: ProfileStore,
    pub rs: RotateState,
    pub home: PathBuf,
    pub settings: Settings,
    pub stop: Option<Arc<AtomicBool>>,
    last_skips: Option<Vec<(ProfileName, Skip)>>,
    last_proto: Option<&'static str>,
    pub(super) failed_live: u32,
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
            failed_live: 0,
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
    save(ctx, &mut l, CycleOutcome::Alerted, t, None, None)
}

/// Records the cycle in the ledger; see [`record`].
fn save(
    ctx: &Ctx,
    l: &mut RotateLedger,
    o: CycleOutcome,
    t: i64,
    m: Option<i64>,
    tag: Option<super::overflow::Tag>,
) -> Result<(), ProfileError> {
    record(&ctx.rs, l, o.as_str(), t, m, tag)
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

/// Logs the skipped profiles and the ignored prototype ledger when they
/// differ from what this process last logged, and each API-key profile
/// ignored in `overflow=` once per process.
fn log_skips(ctx: &mut Ctx, out: &mut Sink, snap: &Snap) {
    let t = now();
    log_ignored(out, snap, t);
    if snap.proto != ctx.last_proto {
        if let Some(reason) = snap.proto {
            out.emit(t, &format!("skip: prototype ledger ignored ({reason})"));
        }
        ctx.last_proto = snap.proto;
    }
    let skips: Vec<_> = snap
        .skips
        .iter()
        .filter(|(_, s)| *s != Skip::Live)
        .cloned()
        .collect();
    if ctx.last_skips.as_ref() != Some(&skips) {
        if !snap.sees_sessions {
            out.emit(t, msg::NO_LIVENESS);
        }
        for (n, s) in skips
            .iter()
            .filter(|(_, s)| snap.sees_sessions || *s != Skip::SessionRunning)
        {
            out.emit(t, &format!("skip: {n} ({})", s.as_str()));
        }
        ctx.last_skips = Some(skips);
    }
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

pub fn run_cycle(ctx: &mut Ctx, mode: Mode) -> Result<CycleOutcome, ProfileError> {
    if mode == Mode::DryRun {
        return dry_run(ctx);
    }
    prototype_guard(ctx.store.root())?;
    ctx.rs.ensure()?;
    let log = open_or_warn(&ctx.rs.log, &ctx.rs.log1);
    let mut out = Sink::new(log, mode == Mode::OneShot, true);
    let state = State::new(ctx.store.root())?;

    // Step 3: recover and identify under the lock.
    let Some(lock) = state.try_lock(LOCK_WAIT)? else {
        out.emit(now(), msg::LOCK_BUSY);
        return Ok(CycleOutcome::Skipped);
    };
    if let Err(e) = recover_auto_switch(&state) {
        if !matches!(e, ProfileError::AutoSwitchInvariant(_)) {
            return Err(e);
        }
        alert(
            ctx,
            &mut out,
            "auto-switch",
            None,
            msg::AUTO_SWITCH_INCONSISTENT,
        )?;
        return Ok(CycleOutcome::Alerted);
    }
    let inconsistent = match swap::recover(&ctx.store, &ctx.rs, &ctx.home) {
        Ok(RecoverOutcome::None) => None,
        Ok(RecoverOutcome::RolledBack { from, to }) => {
            out.emit(now(), &msg::recovered(false, &from, &to));
            None
        }
        Ok(RecoverOutcome::RolledForward { from, to }) => {
            out.emit(now(), &msg::recovered(true, &from, &to));
            None
        }
        Ok(RecoverOutcome::Inconsistent(reason))
        | Err(ProfileError::RotateInconsistent(reason)) => Some(reason),
        Err(e) => return Err(e),
    };
    if let Some(reason) = inconsistent {
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
    let first = snapshot(ctx, &state)?;
    log_skips(ctx, &mut out, &first);
    if let Some((kind, line)) = live_alert(&first) {
        alert(ctx, &mut out, kind, live_name(&first.live), &line)?;
        return Ok(CycleOutcome::Alerted);
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
        tag,
        new_blocks,
        next_plan_reset,
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
        save(
            ctx,
            &mut l,
            CycleOutcome::Skipped,
            t,
            Some(measured_at),
            None,
        )?;
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
                    // Issue #1700: a login without OAuth is refused once per
                    // profile, keyed by the profile it would have made live.
                    let (kind, live) = if swap::is_no_oauth(&e) {
                        ("no_oauth".to_string(), Some(&to))
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
            let target = reading_of(&others, &to);
            out.emit(now(), &msg::swap(&from, &to, &live, &target, reason));
            note_move(&mut out, tag, &from, &to, next_plan_reset, first.platform());
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
            let (kind, line) = alert_line(&first, tag, &live, || platform_block_reason(ctx));
            alert_once(&mut out, &mut l, &alert_key(kind, name), t, &line);
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
        first.platform(),
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
