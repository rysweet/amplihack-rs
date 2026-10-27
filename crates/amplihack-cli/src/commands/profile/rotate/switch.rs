//! `profile switch NAME` (issue #1691): moves NAME's login into `~/.claude`
//! with the rotator's own prelude (lock, recovery, identify) and swap, then,
//! still under the lock, pins rotation to NAME, makes it the default and
//! records the switch as the ledger's last cycle, so the limit-resume hook
//! wakes sessions stopped before it. `release` removes the pin and the
//! default together, under the same lock, for `profile switch --none`. Prints nothing,
//! never looks for `claude` and never starts the rotator; see "Rotate
//! switch" in docs/reference/profile-command.md.

use super::cycle::{Ctx, LOCK_WAIT, prototype_guard};
use super::identify::LiveId;
use super::ledger::{LastCycle, RotateState, SWITCHED, clear_alert};
use super::log::msg;
use super::observe::live_alert;
use super::pin;
use super::prelude::{Prelude, locked_prelude};
use super::settings;
use super::swap::{self, NO_STORED_LOGIN_HINT, Source};
use super::{Released, TuneFlags};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{CREDS, State, exists, now};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::pointer;
use crate::commands::profile::shared_dirs;
use crate::commands::profile::store::ProfileStore;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Switched {
    Moved { from: ProfileName, to: ProfileName },
    AlreadyLive(ProfileName),
}

/// The login moved, but a write after the swap failed; `what` names it.
/// Rerunning the switch takes the already-live path and repeats the writes.
#[derive(Debug)]
pub struct AfterSwapFailure {
    pub what: &'static str,
    pub to: ProfileName,
    pub err: ProfileError,
}

fn refused(line: &str) -> ProfileError {
    let text = line.strip_prefix("ALERT: ").unwrap_or(line);
    ProfileError::RotateRefused(text.strip_prefix("skip: ").unwrap_or(text).to_string())
}

fn lock_busy() -> ProfileError {
    refused(msg::LOCK_BUSY)
}

/// Moves `name`'s login into `~/.claude` and pins rotation to it. Every
/// refusal is `RotateRefused` and writes no pin, no `.default` and no ALERT;
/// recovery of an interrupted swap may already have run and stays done.
pub fn switch_live(
    store: &ProfileStore,
    home: &Path,
    name: &ProfileName,
) -> Result<Result<Switched, AfterSwapFailure>, ProfileError> {
    // Defaults only: a switch decides nothing, so no tuning applies.
    let s = settings::resolve(&TuneFlags::default(), None, &|_| None)?;
    let mut ctx = Ctx::new(store.clone(), home.to_path_buf(), s);
    let (mut out, prelude) = locked_prelude(&mut ctx, false)?;
    let (lock, first) = match prelude {
        Prelude::Ready { lock, first, .. } => (lock, first),
        Prelude::LockBusy => return Err(lock_busy()),
        Prelude::AutoSwitchInconsistent => return Err(refused(msg::AUTO_SWITCH_INCONSISTENT)),
        Prelude::Inconsistent(reason) => return Err(refused(&msg::inconsistent(&reason))),
        Prelude::LiveUnusable { line, .. } => return Err(refused(&line)),
    };
    let source = match &first.live {
        LiveId::Known(live) if live == name => None,
        LiveId::Known(live) => Some(Source::Profile(live.clone())),
        LiveId::Adopt => Some(Source::Adopt),
        // The prelude already refuses these as `LiveUnusable`.
        other => {
            let line = live_alert(other).map_or_else(|| msg::NO_LOGIN.to_string(), |a| a.1);
            return Err(refused(&line));
        }
    };
    let switched = match source {
        None => {
            out.emit(now(), &msg::switched_already_live(name));
            Switched::AlreadyLive(name.clone())
        }
        Some(source) => {
            // `lstat` only: the swap makes the real checks on the login.
            if !exists(&store.root().join(name.as_str()).join(CREDS)) {
                return Err(ProfileError::RotateRefused(format!(
                    "profile {name} {NO_STORED_LOGIN_HINT} {name}` first"
                )));
            }
            let from = swap::swap(store, &ctx.rs, home, source, name, now())?;
            out.emit(now(), &msg::switched(&from, name));
            Switched::Moved {
                from,
                to: name.clone(),
            }
        }
    };
    // Pin and pointer under the lock, so two switches cannot leave them
    // naming different profiles. Already live, nothing moved: a failure is a
    // plain error.
    let moved = matches!(switched, Switched::Moved { .. });
    let after = |what: &'static str, err: ProfileError| {
        if moved {
            Ok(Err(AfterSwapFailure {
                what,
                to: name.clone(),
                err,
            }))
        } else {
            Err(err)
        }
    };
    if let Err(err) = pin::write(&ctx.rs, name) {
        return after("rotation hold", err);
    }
    if let Err(err) = pointer::write(store.root(), name) {
        return after("default pointer", err);
    }
    if let Err(err) = record_switch(&ctx.rs) {
        return after("rotate ledger", err);
    }
    drop(lock);
    // Issue #1634: link the session dirs of both profiles, as a rotate swap
    // does, after the lock is released.
    if let Switched::Moved { from, to } = &switched {
        shared_dirs::ensure_for(store, to);
        shared_dirs::ensure_for(store, from);
    }
    Ok(Ok(switched))
}

/// Records the switch as the ledger's last cycle: outcome
/// [`SWITCHED`], measured now, and no alert. The person chose this login, so
/// the limit-resume hook treats it as capacity measured at the switch: a
/// session stopped on a limit before now is woken, one stopped later is not.
/// Held cycles leave this record in place (`cycle.rs`).
fn record_switch(rs: &RotateState) -> Result<(), ProfileError> {
    let mut l = rs.load_ledger()?.unwrap_or_default();
    let t = now();
    clear_alert(&mut l);
    l.last_cycle = Some(LastCycle {
        at: t,
        measured_at: Some(t),
        outcome: SWITCHED.into(),
    });
    rs.save_ledger(&l)
}

/// `profile switch --none`: under the rotate lock that `switch_live` holds
/// while it writes the pin and the default pointer, checks for and removes
/// both, so a concurrent switch either finishes first (and is undone) or
/// starts after (and stands); pin and default never disagree. When the lock is busy nothing is removed.
pub fn release(store: &ProfileStore) -> Result<Released, ProfileError> {
    let rs = RotateState::new(store.root());
    prototype_guard(store.root())?;
    let state = State::new(store.root())?;
    let Some(_lock) = state.try_lock(LOCK_WAIT)? else {
        return Err(lock_busy());
    };
    // The pin first: when it cannot be removed, `.default` stays too.
    let pin = pin::clear(&rs)?;
    let default = pointer::exists(store.root());
    pointer::clear(store.root())?;
    Ok(Released { pin, default })
}
