//! Recovery of an interrupted swap (D10): settles it by where the recorded
//! inodes are, with fresh credential-bound authorization before completion.
//! Login movement uses renames and never deletes a login.

use super::identify::{LoginKind, login_kind};
use super::ledger::{Phase, RotateJournal, RotateState};
use super::login_identity::{Reader, UNAVAILABLE};
use super::swap::Source;
use super::swap::{LOGINS_MISPLACED, drop_locks, finish, ino_at, remove_unused_adoption};
use super::swap_authorize::authorize;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::CREDS;
use crate::commands::profile::kind::{self, Kind};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverOutcome {
    None,
    RolledBack { from: ProfileName, to: ProfileName },
    RolledForward { from: ProfileName, to: ProfileName },
    Inconsistent(String),
}

/// Marks the journal `inconsistent`: no swap until a person resolves it.
/// Claude's refresh locks the swap took are released first (by inode), so
/// Claude can still refresh its token while the journal waits.
fn freeze(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    mut j: RotateJournal,
    reason: &str,
) -> Result<RecoverOutcome, ProfileError> {
    drop_locks(store, rs, home, &mut j)?;
    j.phase = Phase::Inconsistent;
    rs.save_journal(&j)?;
    Ok(RecoverOutcome::Inconsistent(reason.to_string()))
}

/// Settles an interrupted swap by where the recorded inodes are; the
/// phase never decides. Fresh trusted identity authorizes completion;
/// login movement uses renames and never deletes a login.
pub fn recover(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Result<RecoverOutcome, ProfileError> {
    // Issue #2218: as in a cycle, an excluded login's identity comes from
    // local files (daemon start-up recovery).
    let budget = super::identify::excluded_offline::gated_budget(store, rs, home)?;
    let read = super::login_identity::default_read();
    recover_with(
        store,
        rs,
        home,
        Reader {
            read: &read,
            budget: &budget,
        },
    )
}

pub(super) fn recover_with(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    reader: Reader<'_>,
) -> Result<RecoverOutcome, ProfileError> {
    recover_at(
        store,
        rs,
        home,
        reader,
        crate::commands::profile::auto_switch::kit::now,
    )
}

pub(super) fn recover_at(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    reader: Reader<'_>,
    clock: fn() -> i64,
) -> Result<RecoverOutcome, ProfileError> {
    rs.ensure()?;
    let Some(mut j) = rs.load_journal()? else {
        return Ok(RecoverOutcome::None);
    };
    if j.phase == Phase::Inconsistent && j.version == 1 {
        return Ok(RecoverOutcome::Inconsistent(
            "an earlier swap stopped half way".into(),
        ));
    }
    if j.version == 2
        && j.movement_directories.is_none()
        && (j.phase != Phase::Checking || j.parking.is_some() || j.outgoing.is_some())
    {
        return Ok(RecoverOutcome::Inconsistent(
            "credential directory bindings are missing; journal retained for repair".into(),
        ));
    }
    // Another HOME (sudo, cron) changes nothing: the run with the journal's
    // HOME recovers it.
    if home.to_str() != Some(j.home.as_str()) {
        return Ok(RecoverOutcome::Inconsistent(
            "the journal is for another home directory".into(),
        ));
    }
    // Names were checked when the journal was loaded.
    let (from, to) = (ProfileName::parse(&j.from)?, ProfileName::parse(&j.to)?);
    super::swap::check_home(home)?;
    let live_dir = super::verified_dirs::Directory::open_login(
        &home.join(".claude"),
        j.movement_directories.map(|d| d[0]),
    )
    .map_err(super::verified_dirs::incomplete)?;
    let target_dir = super::verified_dirs::Directory::open(
        &store.root().join(to.as_str()),
        j.movement_directories.map(|d| d[1]),
    )
    .map_err(super::verified_dirs::incomplete)?;
    let pre_exchange = ino_at(&home.join(".claude").join(CREDS)) == Some(j.live_ino)
        && store
            .verify_profile_dir(&to)
            .ok()
            .and_then(|dir| ino_at(&dir.join(CREDS)))
            == Some(j.target_ino);
    if !super::swap::source_current(store, &j) {
        return Ok(RecoverOutcome::Inconsistent(
            "outgoing profile directory changed; journal retained".into(),
        ));
    }
    if j.parking.is_some() && j.phase == Phase::Checking {
        super::swap::check_home(home)?;
        // Parking is movement even though exchange has not started. Reacquire
        // missing operation locks; never break a different writer's lock.
        for (dir, which) in [
            (home.join(".claude"), 0),
            (store.verify_profile_dir(&to)?, 1),
            (store.verify_profile_dir(&from)?, 2),
        ] {
            let held = match which {
                0 => j.home_lock_ino,
                1 => j.to_lock_ino,
                _ => j.from_lock_ino,
            };
            match (
                held,
                ino_at(&dir.join(crate::commands::profile::auto_switch::kit::REFRESH_LOCK)),
            ) {
                (Some(a), Some(b)) if a == b => {}
                (_, None) => {
                    let ino = super::swap::take_lock(&dir)?;
                    match which {
                        0 => j.home_lock_ino = Some(ino),
                        1 => j.to_lock_ino = Some(ino),
                        _ => j.from_lock_ino = Some(ino),
                    }
                    rs.save_journal(&j)?;
                }
                _ => {
                    return Ok(RecoverOutcome::Inconsistent(
                        "duplicate parking refresh lock changed".into(),
                    ));
                }
            }
        }
    }
    if let Some(p) = &j.parking {
        p.recover(rs, store, home, pre_exchange, reader)?;
    }
    if j.phase == Phase::Checking && (pre_exchange || j.version == 1) {
        // Preflight never authorizes login movement or creates adoption profiles.
        if super::swap::check_home(home).is_err() || store.verify_profile_dir(&to).is_err() {
            return Ok(RecoverOutcome::Inconsistent(
                "preflight lock paths changed".into(),
            ));
        }
        drop_locks(store, rs, home, &mut j)?;
        if let Some(c) = &j.outgoing {
            c.rollback(store)?;
        }
        rs.delete_journal()?;
        return Ok(RecoverOutcome::RolledBack { from, to });
    }
    let (Ok(from_dir), Ok(to_dir)) = (
        super::custody::destination(store, &j),
        store.verify_profile_dir(&to),
    ) else {
        return freeze(
            store,
            rs,
            home,
            j,
            "a profile directory failed the directory check",
        );
    };
    let l = home.join(".claude").join(CREDS);
    if !fs::symlink_metadata(&l).is_ok_and(|m| m.is_file()) {
        return freeze(store, rs, home, j, "no login in ~/.claude");
    }
    let (t, s) = (to_dir.join(CREDS), from_dir.join(CREDS));
    let (at_t, at_s) = (ino_at(&t), ino_at(&s));
    if at_t == Some(j.target_ino) && at_s.is_none() && ino_at(&l) == Some(j.live_ino) {
        drop_locks(store, rs, home, &mut j)?;
        if let Some(c) = &j.outgoing {
            c.rollback(store)?;
        }
        rs.delete_journal()?;
        remove_unused_adoption(store, &j);
        return Ok(RecoverOutcome::RolledBack { from, to });
    }
    let outgoing = if at_t == Some(j.live_ino) && at_s.is_none() {
        &to_dir
    } else if at_s == Some(j.live_ino) && at_t.is_none() {
        &from_dir
    } else {
        return freeze(store, rs, home, j, LOGINS_MISPLACED);
    };
    // Issue #2218: an excluded outgoing login is never read over the
    // network, whichever folder the interrupted swap left it in.
    if super::identify::excluded_offline::is_excluded(store, &from) {
        reader.budget.add_local_only(outgoing);
    }
    // Recovery may run after an earlier uncertain attempt released its
    // locks. Reacquire them before fresh evidence authorizes any action.
    let locked = (|| {
        for (dir, held) in [
            (&home.join(".claude"), &mut j.home_lock_ino),
            (&to_dir, &mut j.to_lock_ino),
        ] {
            match (
                *held,
                ino_at(&dir.join(crate::commands::profile::auto_switch::kit::REFRESH_LOCK)),
            ) {
                (Some(a), Some(b)) if a == b => {}
                (_, None) => {
                    *held = Some(super::swap::take_lock(dir)?);
                }
                _ => return Err(super::swap_authorize::refused(UNAVAILABLE)),
            }
        }
        if j.outgoing.is_some() {
            let dir = store.verify_profile_dir(&from)?;
            match (
                j.from_lock_ino,
                ino_at(&dir.join(crate::commands::profile::auto_switch::kit::REFRESH_LOCK)),
            ) {
                (Some(a), Some(b)) if a == b => {}
                (_, None) => {
                    j.from_lock_ino = Some(super::swap::take_lock(&dir)?);
                }
                _ => return Err(super::swap_authorize::refused(UNAVAILABLE)),
            }
            match (
                j.outgoing_lock_ino,
                ino_at(&from_dir.join(crate::commands::profile::auto_switch::kit::REFRESH_LOCK)),
            ) {
                (Some(a), Some(b)) if a == b => {}
                (_, None) => {
                    j.outgoing_lock_ino = Some(super::swap::take_lock(&from_dir)?);
                }
                _ => return Err(super::swap_authorize::refused(UNAVAILABLE)),
            }
        }
        rs.save_journal(&j)
    })();
    if let Err(e) = locked {
        let held = j.clone();
        if let Err(save) = drop_locks(store, rs, home, &mut j) {
            super::swap::release_locks(store, home, &held);
            return Err(save);
        }
        return Ok(RecoverOutcome::Inconsistent(e.to_string()));
    }
    let source = if j.adopt {
        Source::Adopt
    } else {
        Source::Profile(from.clone())
    };
    let platform_in =
        login_kind(&l) == LoginKind::NoOAuth && kind::kind_of(store, &to, home) == Kind::Platform;
    let platform_out = login_kind(&outgoing.join(CREDS)) == LoginKind::NoOAuth
        && kind::kind_of(store, &from, home) == Kind::Platform;
    if let Some(c) = &j.outgoing {
        let fresh = reader
            .server_bound(outgoing)
            .map_err(super::swap_authorize::refused)?;
        if !c.permits(store, &fresh) {
            return Err(super::swap_authorize::refused(UNAVAILABLE));
        }
    }
    let conflict = if j.outgoing.is_some() {
        let origin = store.verify_profile_dir(&from)?;
        match fs::symlink_metadata(origin.join(CREDS)) {
            Ok(_) => Some(super::duplicate::recovery_conflict(
                store, &from, outgoing, reader,
            )?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        }
    } else {
        None
    };
    let proof = conflict.as_ref().map_or(
        super::swap_authorize::Proof::Recovery(j.identity.as_ref()),
        super::swap_authorize::Proof::Conflict,
    );
    let auth = match authorize(
        store,
        rs,
        &source,
        &to,
        outgoing,
        &home.join(".claude"),
        platform_in,
        platform_out,
        reader,
        proof,
    ) {
        Ok(auth) if auth.current(store) => auth,
        result => {
            // Uncertainty is retryable: retain the original journal phase,
            // release only operation-owned locks, and move/relabel nothing.
            drop_locks(store, rs, home, &mut j)?;
            let why = result
                .err()
                .map_or_else(|| UNAVAILABLE.to_string(), |e| e.to_string());
            return Ok(RecoverOutcome::Inconsistent(why));
        }
    };
    if j.identity
        .as_ref()
        .is_some_and(|prior| !prior.names(auth.outgoing.account(), auth.incoming.account()))
        || j.outgoing
            .as_ref()
            .is_some_and(|c| !c.permits(store, &auth.outgoing))
    {
        return Err(super::swap_authorize::refused(UNAVAILABLE));
    }
    let destination = store.root().join(
        j.outgoing
            .as_ref()
            .map_or(j.from.as_str(), |c| c.destination.as_str()),
    );
    let saved_dir = super::verified_dirs::Directory::open(
        &destination,
        j.outgoing
            .as_ref()
            .map_or(j.from_directory, |c| c.directory()),
    )
    .map_err(super::verified_dirs::incomplete)?;
    if outgoing == &to_dir
        && let Err(e) = target_dir.move_to(&saved_dir, j.live_ino)
    {
        let why = format!("the outgoing login could not be moved to its profile: {e}");
        return freeze(store, rs, home, j, &why);
    }
    // Presence is not durability. Always synchronize all movement directories,
    // even when an earlier attempt already established the final placement.
    super::verified_dirs::durable(&[&live_dir, &target_dir, &saved_dir])?;
    if let Some(c) = &j.outgoing {
        c.commit(store, rs)?;
        c.record_account(
            store,
            auth.outgoing
                .account()
                .ok_or_else(|| super::swap_authorize::refused(UNAVAILABLE))?,
        )?;
    }
    // Moving the outgoing login does not change the incoming binding.
    finish(rs, home, &mut j, &auth.incoming)?;
    // Original ownership metadata must be durable before publishing success.
    if j.outgoing.is_none() {
        if j.completion.is_some() {
            super::swap_expect::settle_durable(store, &from, auth.pending)?;
        } else {
            super::swap_expect::settle(store, &from, auth.pending);
        }
    }
    drop_locks(store, rs, home, &mut j)?;
    if j.completion.is_some() {
        super::swap_record::complete(rs, &mut j, clock())?;
    } else {
        rs.delete_journal()?;
    }
    Ok(RecoverOutcome::RolledForward { from, to })
}
