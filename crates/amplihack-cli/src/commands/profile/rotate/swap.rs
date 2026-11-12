//! The journalled swap (D9, D12) and its recovery (D10). `L` is
//! `~/.claude/.credentials.json`, `T` is `<root>/<to>/.credentials.json` and
//! `S` is `<root>/<from>/.credentials.json`. Each login stays in exactly
//! one place and `~/.claude` always holds one.

use super::identify::{Account, LoginKind, login_kind};
use super::ledger::{Phase, RotateJournal, RotateState};
#[cfg(test)]
use super::login_identity::Budget;
use super::login_identity::{Reader, UNAVAILABLE};
pub use super::recover::{RecoverOutcome, recover};
use super::swap_authorize::{Proof, authorize};
#[path = "swap_prepare.rs"]
mod prepare;
pub(super) use super::swap_check::check_home;
pub use super::swap_check::{adoption_name, is_no_oauth};
pub(super) use super::swap_check::{ino_at, show};
use super::swap_check::{may_go_live, no_oauth};
pub use super::swap_expect::is_account_mismatch;
use super::swap_expect::{relabel, settle};
pub(super) use super::swap_finish::{finish, remove_unused_adoption};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, CREDS, MoveError, REFRESH_LOCK, abort_point, check_root_private, exchange_logins,
    exists, movable_login, move_login, read_claude_json, release_refresh_lock_at,
    take_refresh_lock_at, write_private_atomic,
};
use crate::commands::profile::kind::{self, Kind};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
// The reasons the how-to quotes word for word, held there by `tests_docs_sync`.
pub(super) const HOME_NOT_OWNED: &str = "HOME is not a directory owned by you";
pub(super) const DOT_CLAUDE_NOT_PRIVATE: &str = "~/.claude is not a private directory owned by you";
pub(super) const LOGIN_NOT_MOVABLE: &str =
    "a login file is not a private regular file with one link";
pub const DIFFERENT_DEVICES: &str = "~/.claude and the profile root are on different devices";
const NO_EXCHANGE: &str = "atomic exchange is not supported here";
const ALREADY_LOGGED_IN: &str = "already has a login";
pub(super) const LOGINS_MISPLACED: &str = "the login files are not where the journal expects";
const REFRESHING: &str = "Claude holds its refresh lock; the swap waits for the next cycle";
pub(super) const HOME_JSON_UNREADABLE: &str = "~/.claude.json is not a JSON object";
/// `profile switch NAME` (issue #1691) when NAME has no login to move in.
pub const NO_STORED_LOGIN_HINT: &str = "has no stored login; run `amplihack profile login";
#[cfg(test)]
pub const DOCUMENTED: [&str; 13] = [
    HOME_NOT_OWNED,
    DOT_CLAUDE_NOT_PRIVATE,
    LOGIN_NOT_MOVABLE,
    DIFFERENT_DEVICES,
    NO_EXCHANGE,
    ALREADY_LOGGED_IN,
    LOGINS_MISPLACED,
    REFRESHING,
    HOME_JSON_UNREADABLE,
    NO_STORED_LOGIN_HINT,
    super::FOREIGN_IGNORES_HOLD,
    super::swap_expect::ACCOUNT_MISMATCH,
    super::swap_expect::NOT_RELABELLED,
];
/// Where the outgoing login goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Profile(ProfileName),
    /// A new `default-original[-N]` profile (D12).
    Adopt,
}
fn refused(msg: impl Into<String>) -> ProfileError {
    ProfileError::RotateRefused(msg.into())
}

pub(super) fn describe(e: MoveError) -> String {
    match e {
        MoveError::Unsupported => NO_EXCHANGE.to_string(),
        MoveError::Exists => "the exchange found an unexpected file".to_string(),
        MoveError::Other(m) => m,
    }
}

/// Runs the swap after structural and trusted-identity authorization.
/// Returns the outgoing profile; refused authorization moves no login.
#[cfg(test)]
pub fn swap(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: Source,
    to: &ProfileName,
    now: i64,
) -> Result<ProfileName, ProfileError> {
    let budget = Budget::new();
    let read = super::login_identity::default_read();
    swap_with(
        store,
        rs,
        home,
        from,
        to,
        now,
        Reader {
            read: &read,
            budget: &budget,
        },
        None,
    )
}

/// Shared swap checked against the accepted snapshot (None in legacy unit primitives).
#[allow(clippy::too_many_arguments)]
pub(super) fn swap_with(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: Source,
    to: &ProfileName,
    now: i64,
    reader: Reader<'_>,
    basis: Option<&super::observe::Snap>,
) -> Result<ProfileName, ProfileError> {
    check_root_private(store.root()).map_err(|e| refused(format!("profile root {e}")))?;
    let dot = check_home(home)?;
    if exists(&rs.journal) {
        return Err(refused("a swap journal is pending; run recovery first"));
    }
    let to_dir = store
        .verify_profile_dir(to)
        .map_err(|_| refused(format!("profile {to} failed the directory check")))?;
    let (l, t) = (dot.join(CREDS), to_dir.join(CREDS));
    if !movable_login(&l) || !movable_login(&t) {
        return Err(refused(LOGIN_NOT_MOVABLE));
    }
    // Issue #1700: `~/.claude` is never left without an OAuth login unless
    // the incoming profile is an API-key profile; checked before J0, so a
    // refusal writes no journal, and again once J0 records `T`'s inode.
    let platform = kind::kind_of(store, to, home) == Kind::Platform;
    if !may_go_live(&t, platform) {
        return Err(no_oauth(to));
    }
    // `s3` could not record the new account; nothing has moved yet.
    if exists(&home.join(CLAUDE_JSON)) && read_claude_json(home).is_none() {
        return Err(refused(HOME_JSON_UNREADABLE));
    }
    let dev = |p: &Path| fs::metadata(p).map(|m| m.dev()).ok();
    if dev(&dot).is_none() || dev(&dot) != dev(&to_dir) {
        return Err(refused(DIFFERENT_DEVICES));
    }
    if !cfg!(target_os = "linux") {
        return Err(refused(NO_EXCHANGE));
    }
    // Refuses before anything is written; the locks are taken below.
    if exists(&dot.join(REFRESH_LOCK)) || exists(&to_dir.join(REFRESH_LOCK)) {
        return Err(refused(REFRESHING));
    }
    if let Source::Profile(name) = &from {
        let dir = store
            .verify_profile_dir(name)
            .map_err(|_| refused(format!("profile {name} failed the directory check")))?;
        if name == to || exists(&dir.join(CREDS)) {
            return Err(refused(format!("profile {name} {ALREADY_LOGGED_IN}")));
        }
    }
    let platform_out = login_kind(&l) == LoginKind::NoOAuth
        && matches!(&from, Source::Profile(n) if kind::kind_of(store, n, home) == Kind::Platform);
    let first = authorize(
        store,
        rs,
        &from,
        to,
        &dot,
        &to_dir,
        platform,
        platform_out,
        reader,
        Proof::Live,
    )?;
    super::swap_authorize::check_snapshot(&first, basis, to)?;
    // Persist owned locks before final identity requests can block or terminate.
    let mut j = prepare::begin(store, rs, home, &from, to, now, &l, &t)?;
    let checked = authorize(
        store,
        rs,
        &from,
        to,
        &dot,
        &to_dir,
        platform,
        platform_out,
        reader,
        Proof::Live,
    );
    let mut auth = match checked {
        Ok(auth) if first.same(&auth) && auth.current(store) => auth,
        other => {
            let error = other.err().unwrap_or_else(|| refused(UNAVAILABLE));
            return Err(abandon(store, rs, home, &mut j, error));
        }
    };
    let pending = std::mem::replace(&mut auth.pending, super::swap_expect::Pending::Nothing);
    let prepared = (|| {
        let home_str = home.to_str().ok_or_else(|| refused("HOME is not UTF-8"))?;
        let (Some(live_ino), Some(target_ino)) = (ino_at(&l), ino_at(&t)) else {
            return Err(refused(LOGIN_NOT_MOVABLE));
        };
        // The one write before J0, after the last check that refuses with
        // nothing written. `Profile` and `Adopt` exclude each other, so the
        // adoption below never follows a relabel.
        relabel(store, &from, &pending)?;
        let (from, adopt) = match from {
            Source::Profile(name) => (name, false),
            Source::Adopt => (adopt(store, auth.outgoing.account())?, true),
        };
        let j = RotateJournal {
            from: from.to_string(),
            to: to.to_string(),
            adopt,
            phase: Phase::J0,
            home: home_str.to_string(),
            live_ino,
            target_ino,
            identity: Some(super::login_identity::SwapIdentity::new(
                auth.outgoing.account(),
                auth.incoming.account(),
            )),
            ..j.clone()
        };
        Ok((from, j))
    })();
    let (from, mut j) = match prepared {
        Ok(v) => v,
        Err(e) => {
            return Err(abandon(store, rs, home, &mut j, e));
        }
    };
    if let Err(e) = rs.save_journal(&j) {
        release_locks(store, home, &j);
        remove_unused_adoption(store, &j);
        return Err(e);
    }
    let (live_ino, target_ino) = (j.live_ino, j.target_ino);
    // Recheck every material ownership dependency immediately before movement.
    if ino_at(&t) != Some(target_ino)
        || ino_at(&l) != Some(live_ino)
        || !auth.current_ignoring(store, j.adopt.then_some(&from))
        || store.verify_profile_dir(&from).is_err()
        || exists(&store.root().join(from.as_str()).join(CREDS))
    {
        return Err(abandon(store, rs, home, &mut j, refused(LOGIN_NOT_MOVABLE)));
    }
    if !may_go_live(&t, platform) {
        return Err(abandon(store, rs, home, &mut j, no_oauth(to)));
    }
    abort_point("rotate-J0");

    if let Err(e) = exchange_logins(&t, &l) {
        // Only a target that still holds its own login was not exchanged.
        // A failed sync after the exchange still exchanged the logins: go
        // on as journalled, or they would be filed under the wrong profiles
        // with no journal to say so.
        match ino_at(&t) {
            Some(ino) if ino == target_ino => {
                return Err(abandon(store, rs, home, &mut j, refused(describe(e))));
            }
            // Cannot tell: the journal stays at J0 for recovery to settle.
            None => {
                return Err(ProfileError::RotateInconsistent(format!(
                    "the target login could not be checked after the exchange failed: {}",
                    describe(e)
                )));
            }
            Some(_) => {}
        }
        eprintln!("warning: {}; the swap goes on", describe(e));
    }
    step(rs, &mut j, Phase::S1)?;
    let s = store.root().join(from.as_str()).join(CREDS);
    if let Err(e) = move_login(&t, &s, &show) {
        // `~/.claude` already holds the incoming login, as after a finished
        // swap, so Claude may refresh it again. The journal stays at S1 and
        // the next recovery retries the move.
        drop_locks(store, rs, home, &mut j)?;
        return Err(ProfileError::RotateInconsistent(format!(
            "the outgoing login could not be moved from {to} to {from}: {}",
            describe(e)
        )));
    }
    step(rs, &mut j, Phase::S2)?;
    let incoming = auth
        .incoming
        .moved_to(&dot)
        .map_err(|_| ProfileError::RotateInconsistent(UNAVAILABLE.into()))?;
    super::swap_authorize::check_record(store, to, incoming.account())
        .map_err(|_| ProfileError::RotateInconsistent(UNAVAILABLE.into()))?;
    finish(rs, home, &j, &incoming)?;
    drop_locks(store, rs, home, &mut j)?;
    rs.delete_journal()?;
    settle(store, &from, pending);
    Ok(from)
}

/// Takes the refresh lock; an existing lock always refuses.
pub(super) fn take_lock(dir: &Path) -> Result<u64, ProfileError> {
    take_refresh_lock_at(dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => refused(REFRESHING),
        _ => refused(format!("Claude's refresh lock could not be taken: {e}")),
    })
}

/// Undoes a swap that stopped before any login moved: releases the locks
/// it took, deletes the journal and removes a profile it adopted, which
/// still holds no login. Returns `err`, or the error that stopped the undo.
fn abandon(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    j: &mut RotateJournal,
    err: ProfileError,
) -> ProfileError {
    // The journal forgets the locks before they are released, so a journal
    // left behind cannot later remove a lock Claude made with a reused inode.
    // If it cannot be saved (a full disk), the locks are released anyway:
    // the journal may not record them, and nothing else would.
    let held = j.clone();
    if let Err(e) = drop_locks(store, rs, home, j) {
        eprintln!("warning: the abandoned swap's journal was not updated: {e}");
        release_locks(store, home, &held);
    }
    if let Err(e) = rs.delete_journal() {
        eprintln!("warning: the abandoned swap's journal was not removed: {e}");
        return err;
    }
    remove_unused_adoption(store, j);
    err
}

/// Rewrites the journal at `phase`, then stops there in a test run.
pub(super) fn step(
    rs: &RotateState,
    j: &mut RotateJournal,
    phase: Phase,
) -> Result<(), ProfileError> {
    j.phase = phase;
    rs.save_journal(j)?;
    abort_point(&format!("rotate-{}", phase.as_str().to_uppercase()));
    Ok(())
}

/// Creates the adoption profile and writes the `oauthAccount` block of
/// `~/.claude.json` into it, and nothing else. `add` copies no login.
fn adopt(store: &ProfileStore, account: Option<&Account>) -> Result<ProfileName, ProfileError> {
    let name = adoption_name(store)?;
    let dir = store.add(&name)?;
    if exists(&dir.join(CREDS)) {
        return Err(refused(format!("new profile {name} {ALREADY_LOGGED_IN}")));
    }
    if let Some(block) = account.map(Account::block) {
        let body = serde_json::json!({ "oauthAccount": block });
        let bytes = serde_json::to_vec_pretty(&body).map_err(|e| refused(e.to_string()))?;
        write_private_atomic(&dir, CLAUDE_JSON, &bytes)?;
    }
    Ok(name)
}

/// Clears the lock inodes from the journal, saves it, then releases the
/// locks. A lock is released at most once, so a later recovery cannot remove
/// a lock Claude made since, even if it was given the same inode.
pub(super) fn drop_locks(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    j: &mut RotateJournal,
) -> Result<(), ProfileError> {
    let held = j.clone();
    if held.home_lock_ino.is_none() && held.to_lock_ino.is_none() {
        return Ok(());
    }
    (j.home_lock_ino, j.to_lock_ino) = (None, None);
    rs.save_journal(j)?;
    release_locks(store, home, &held);
    Ok(())
}

pub(super) fn release_locks(store: &ProfileStore, home: &Path, j: &RotateJournal) {
    if let Some(ino) = j.home_lock_ino {
        release_refresh_lock_at(&home.join(".claude"), ino);
    }
    if let Some(ino) = j.to_lock_ino {
        release_refresh_lock_at(&store.root().join(&j.to), ino);
    }
}
