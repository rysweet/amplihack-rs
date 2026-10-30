//! The locked start shared by a rotate cycle and `profile switch NAME`
//! (issue #1691): prototype check, lock, recovery of an interrupted
//! auto-switch or rotate swap, hand-change recovery (issue #1693), then who
//! is live. It writes no ALERT of its own; each caller decides what a result
//! other than `Ready` means. Hand-change recovery runs only for a rotate
//! cycle (`hand_change`): a switch must change nothing but `.default` when it
//! refuses, so it refuses a hand-changed login as `LiveUnusable` and leaves
//! recovery to the next cycle, which runs it before reading the ledger.

use super::cycle::{Ctx, LOCK_WAIT, prototype_guard};
use super::handfix;
use super::identify::Skip;
use super::log::{Sink, msg, open_or_warn};
use super::observe::{Snap, live_alert, live_name, snapshot};
use super::overflow_record::log_ignored;
use super::swap::{self, RecoverOutcome};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Lock, State, now, recover_auto_switch};
use crate::commands::profile::name::ProfileName;

pub enum Prelude {
    /// Recovered and identified, the lock still held.
    Ready {
        lock: Lock,
        state: State,
        first: Snap,
    },
    LockBusy,
    AutoSwitchInconsistent,
    Inconsistent(String),
    /// `~/.claude` holds no login, someone else's, or an ambiguous one.
    LiveUnusable {
        kind: &'static str,
        live: Option<ProfileName>,
        line: String,
    },
    /// Issue #1693, rotate cycle only: `~/.claude` was logged in by hand and
    /// could not be recovered safely; the guidance is already in the log and
    /// the ledger.
    Guided,
}

/// Runs the prelude. The sink writes to the rotate log, and to stdout when
/// `stdout` is set; it is returned for the caller's own lines. `hand_change`
/// runs hand-change recovery (issue #1693); only a rotate cycle sets it.
pub fn locked_prelude(
    ctx: &mut Ctx,
    stdout: bool,
    hand_change: bool,
) -> Result<(Sink, Prelude), ProfileError> {
    prototype_guard(ctx.store.root())?;
    ctx.rs.ensure()?;
    let log = open_or_warn(&ctx.rs.log, &ctx.rs.log1);
    let mut out = Sink::new(log, stdout, true);
    let state = State::new(ctx.store.root())?;

    let Some(lock) = state.try_lock(LOCK_WAIT)? else {
        return Ok((out, Prelude::LockBusy));
    };
    if let Err(e) = recover_auto_switch(&state) {
        if !matches!(e, ProfileError::AutoSwitchInvariant(_)) {
            return Err(e);
        }
        return Ok((out, Prelude::AutoSwitchInconsistent));
    }
    match swap::recover(&ctx.store, &ctx.rs, &ctx.home) {
        Ok(RecoverOutcome::None) => {}
        Ok(RecoverOutcome::RolledBack { from, to }) => {
            out.emit(now(), &msg::recovered(false, &from, &to));
        }
        Ok(RecoverOutcome::RolledForward { from, to }) => {
            out.emit(now(), &msg::recovered(true, &from, &to));
        }
        Ok(RecoverOutcome::Inconsistent(reason))
        | Err(ProfileError::RotateInconsistent(reason)) => {
            return Ok((out, Prelude::Inconsistent(reason)));
        }
        Err(e) => return Err(e),
    }
    // Issue #1693: after swap recovery, so the ledger it reads is settled,
    // and before the snapshot, so the snapshot reads the recovered ledger.
    if hand_change && handfix::step(ctx, &state, &mut out, now())? == handfix::Step::Guided {
        return Ok((out, Prelude::Guided));
    }
    let first = snapshot(ctx, &state)?;
    log_skips(ctx, &mut out, &first);
    if let Some((kind, line)) = live_alert(&first) {
        let live = live_name(&first.live).cloned();
        return Ok((out, Prelude::LiveUnusable { kind, live, line }));
    }
    Ok((out, Prelude::Ready { lock, state, first }))
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
