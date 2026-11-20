//! The locked start shared by a rotate cycle and `profile switch NAME`
//! (issue #1691): prototype check, lock, recovery of an interrupted
//! auto-switch or rotate swap, hand-change recovery (issue #1693), then who
//! is live. It writes no ALERT of its own; each caller decides what a result
//! other than `Ready` means. Hand-change recovery runs only for a rotate
//! cycle (`hand_change`): a switch must change nothing but `.default` when it
//! refuses, so it refuses a hand-changed login as `LiveUnusable` and leaves
//! recovery to the next cycle, which runs it before reading the ledger.

use super::cycle::{Ctx, LOCK_WAIT};
use super::handfix;
use super::handfix_msg::ledger_unreadable;
use super::identify::Skip;
use super::identify::excluded_offline::local_only_dirs;
use super::log::{Sink, msg, open_or_warn};
use super::observe::{Read, Snap, live_alert, live_name, snapshot};
use super::overflow_record::log_ignored;
use super::seed;
use super::swap::RecoverOutcome;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Lock, State, now, recover_auto_switch_with};
use crate::commands::profile::name::ProfileName;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

#[path = "live_refresh.rs"]
mod live_refresh;

pub enum Prelude {
    /// Recovered and identified, the lock still held. The snapshot is boxed
    /// to keep the variants close in size.
    Ready {
        lock: Lock,
        state: State,
        first: Box<Snap>,
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
    /// Issue #1962: the rotate ledger cannot be read. The live login was
    /// refreshed and read; `line` is the stop, without `ALERT: `. Nothing may
    /// be saved over the ledger.
    LedgerUnreadable {
        line: String,
    },
    /// Issue #1693, rotate cycle only: `~/.claude` was logged in by hand and
    /// could not be recovered safely; the guidance is already in the log and
    /// the ledger.
    Guided,
}

/// The rotate log's sink, also to stdout when `stdout` is set.
pub fn log_sink(ctx: &Ctx, stdout: bool) -> Sink {
    Sink::new(open_or_warn(&ctx.rs.log, &ctx.rs.log1), stdout, true)
}

/// Runs the prelude. The sink writes to the rotate log, and to stdout when
/// `stdout` is set; it is returned for the caller's own lines. `hand_change`
/// runs hand-change recovery (issue #1693); only a rotate cycle sets it.
pub fn locked_prelude(
    ctx: &mut Ctx,
    stdout: bool,
    hand_change: bool,
) -> Result<(Sink, Prelude), ProfileError> {
    ctx.identity_budget.reset();
    prototype_guard(ctx.store.root())?;
    ctx.rs.ensure()?;
    let mut out = log_sink(ctx, stdout);
    let state = State::new(ctx.store.root())?;

    let Some(lock) = state.try_lock(LOCK_WAIT)? else {
        return Ok((out, Prelude::LockBusy));
    };
    if let Err(e) = recover_auto_switch_with(&state, &ctx.procs) {
        if !matches!(e, ProfileError::AutoSwitchInvariant(_)) {
            return Err(e);
        }
        return Ok((out, Prelude::AutoSwitchInconsistent));
    }
    // Issue #2218: once per cycle, before any identity read: excluded
    // folders, and `~/.claude` when it holds an excluded login, are
    // identified from local files only, so no request carries their token.
    ctx.set_excluded_dirs(local_only_dirs(&ctx.store, &ctx.rs, &ctx.home)?);
    match super::recover::recover_with(&ctx.store, &ctx.rs, &ctx.home, ctx.identity_reader()) {
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
    // Issue #1753 (R1): after swap recovery, so no journal is pending, and
    // before anything reads the live login's identity, which never refreshes.
    live_refresh::refresh_expired_live(ctx, &mut out);
    // Issue #1962: only after the refresh, so the stop names a fresh reading.
    if let Err(e) = ctx.rs.load_ledger() {
        let line = ledger_stop(ctx, &e);
        return Ok((out, Prelude::LedgerUnreadable { line }));
    }
    // Issue #1693: after swap recovery, so the ledger it reads is settled,
    // and before the snapshot, so the snapshot reads the recovered ledger.
    // Issue #2218: not on local evidence alone, which may be stale.
    if hand_change
        && !ctx.home_local_only()
        && handfix::step(ctx, &state, &mut out, now())? == handfix::Step::Guided
    {
        return Ok((out, Prelude::Guided));
    }
    let first = snapshot(ctx, &state, Read::Cached)?;
    log_skips(ctx, &mut out, &first);
    for (name, cause) in &first.candidate_refusals {
        out.emit(
            now(),
            &format!("ALERT: trusted login identity unavailable for profile {name}: {cause}"),
        );
    }
    // Issue #1962: a rotate cycle only; `profile switch` never seeds.
    // Issue #2218: no record is seeded for an excluded login.
    if hand_change
        && !ctx.home_local_only()
        && let Some(line) = seed::in_cycle(ctx, &state, &first, &mut out)
    {
        let live = live_name(&first.live).cloned();
        let kind = seed::SAME_ACCOUNT;
        return Ok((out, Prelude::LiveUnusable { kind, live, line }));
    }
    if let Some((kind, line)) = live_alert(&first) {
        let live = live_name(&first.live).cloned();
        return Ok((out, Prelude::LiveUnusable { kind, live, line }));
    }
    let first = Box::new(first);
    Ok((out, Prelude::Ready { lock, state, first }))
}

/// The stop for an unreadable rotate ledger, with a fresh reading of
/// `~/.claude` and, when the load refused a field, which one (issue #1947).
/// Shared with `--dry-run`, which runs it without a refresh.
pub(super) fn ledger_stop(ctx: &Ctx, load: &ProfileError) -> String {
    let read = ctx.identity_reader().oauth(&ctx.home.join(".claude"));
    let cause = match read {
        Ok(_) => LIVE_READABLE,
        Err(cause) => cause,
    };
    ledger_unreadable(super::ledger::refused_field(load), cause)
}

/// The live-identity part of [`ledger_stop`] when `~/.claude` could be read.
const LIVE_READABLE: &str = "readable";

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
