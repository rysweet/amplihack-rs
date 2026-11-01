//! The journalled swap (D9, D12) and its recovery (D10). `L` is
//! `~/.claude/.credentials.json`, `T` is `<root>/<to>/.credentials.json` and
//! `S` is `<root>/<from>/.credentials.json`. Logins are only checked with
//! `lstat`, exchanged, and renamed without replacing: each login is in
//! exactly one place at every step and `~/.claude` always holds one.

use super::identify::{Account, LoginKind, login_kind};
use super::ledger::{LIVE_KIND_PLATFORM, Phase, RotateJournal, RotateState};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, CREDS, MoveError, REFRESH_LOCK, abort_point, check_root_private, exchange_logins,
    exists, movable_login, move_login, read_claude_json, release_refresh_lock_at,
    take_refresh_lock_at, write_atomic_keep_mode, write_private_atomic,
};
use crate::commands::profile::kind::{self, Kind};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub use super::recover::{RecoverOutcome, recover};
pub(super) use super::swap_check::check_home;
pub use super::swap_check::is_no_oauth;
use super::swap_check::{account_block, may_go_live, no_oauth};
pub(super) use super::swap_check::{ino_at, show};
pub use super::swap_expect::is_account_mismatch;
use super::swap_expect::{guard, settle};

const ADOPT_NAME: &str = "default-original";

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
const HOME_JSON_UNREADABLE: &str = "~/.claude.json is not a JSON object";
/// `profile switch NAME` (issue #1691) when NAME has no login to move in.
pub const NO_STORED_LOGIN_HINT: &str = "has no stored login; run `amplihack profile login";

#[cfg(test)]
pub const DOCUMENTED: [&str; 12] = [
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

/// The first free `default-original`, `default-original-2`, ... name.
pub fn adoption_name(store: &ProfileStore) -> Result<ProfileName, ProfileError> {
    for n in 1..=1000 {
        let raw = if n == 1 {
            ADOPT_NAME.to_string()
        } else {
            format!("{ADOPT_NAME}-{n}")
        };
        if !exists(&store.root().join(&raw)) {
            return ProfileName::parse(&raw);
        }
    }
    Err(refused("no free default-original name"))
}

/// Runs the swap; returns the profile the outgoing login went to. Every
/// check runs before anything is written; a failure there is
/// `RotateRefused` and nothing changes.
pub fn swap(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: Source,
    to: &ProfileName,
    now: i64,
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
    // An API-key profile need not have an `oauthAccount`.
    if !platform && account_block(&to_dir).is_none() {
        return Err(refused(format!(
            "profile {to} has no account in .claude.json"
        )));
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
    let pending = guard(store, rs, home, &from)?; // Issue #1845: reads only.
    let home_str = home.to_str().ok_or_else(|| refused("HOME is not UTF-8"))?;
    let (Some(live_ino), Some(target_ino)) = (ino_at(&l), ino_at(&t)) else {
        return Err(refused(LOGIN_NOT_MOVABLE));
    };
    let (from, adopt) = match from {
        Source::Profile(name) => (name, false),
        Source::Adopt => (adopt(store, home)?, true),
    };
    let mut j = RotateJournal {
        version: 1,
        from: from.to_string(),
        to: to.to_string(),
        adopt,
        phase: Phase::J0,
        home: home_str.to_string(),
        live_ino,
        target_ino,
        home_lock_ino: None,
        to_lock_ino: None,
        started_at: now,
        pid: std::process::id(),
    };
    // Each lock's inode is recorded as soon as it is taken.
    let locked = rs
        .save_journal(&j)
        .and_then(|()| take_lock(&dot))
        .and_then(|ino| {
            j.home_lock_ino = Some(ino);
            rs.save_journal(&j)?;
            j.to_lock_ino = Some(take_lock(&to_dir)?);
            rs.save_journal(&j)
        });
    if let Err(e) = locked {
        return Err(abandon(store, rs, home, &mut j, e));
    }
    // The file the journal names is still the one checked, and still may
    // go live: a file replaced in between is caught here.
    if ino_at(&t) != Some(target_ino) {
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
    drop_locks(store, rs, home, &mut j)?;
    step(rs, &mut j, Phase::S2)?;
    finish(store, rs, home, &j)?;
    settle(store, &from, pending);
    Ok(from)
}

/// Takes Claude's refresh lock in `dir`. A lock already there is never
/// broken: the swap is refused, because a refresh during the exchange would
/// write the outgoing token over the incoming login.
fn take_lock(dir: &Path) -> Result<u64, ProfileError> {
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

/// Removes a profile the swap adopted that still holds no login: nothing
/// but the copied `oauthAccount` block is in it.
pub(super) fn remove_unused_adoption(store: &ProfileStore, j: &RotateJournal) {
    let dir = store.root().join(&j.from);
    if j.adopt
        && !exists(&dir.join(CREDS))
        && let Err(e) = fs::remove_dir_all(&dir)
    {
        eprintln!("warning: unused profile {} was not removed: {e}", j.from);
    }
}

/// Rewrites the journal at `phase`, then stops there in a test run.
fn step(rs: &RotateState, j: &mut RotateJournal, phase: Phase) -> Result<(), ProfileError> {
    j.phase = phase;
    rs.save_journal(j)?;
    abort_point(&format!("rotate-{}", phase.as_str().to_uppercase()));
    Ok(())
}

/// Creates the adoption profile and writes the `oauthAccount` block of
/// `~/.claude.json` into it, and nothing else. `add` copies no login.
fn adopt(store: &ProfileStore, home: &Path) -> Result<ProfileName, ProfileError> {
    let name = adoption_name(store)?;
    let dir = store.add(&name)?;
    if exists(&dir.join(CREDS)) {
        return Err(refused(format!("new profile {name} {ALREADY_LOGGED_IN}")));
    }
    if let Some(block) = account_block(home) {
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

/// `s3` and `s4`, then the journal is deleted. Safe to repeat.
pub(super) fn finish(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    j: &RotateJournal,
) -> Result<(), ProfileError> {
    let mut j = j.clone();
    let to_dir = store.root().join(&j.to);
    let block = account_block(&to_dir);
    patch_home_account(home, block.as_ref())?;
    step(rs, &mut j, Phase::S3)?;
    let mut l = rs.load_ledger()?.unwrap_or_default();
    l.live = Some(j.to.clone());
    l.previous = Some(j.from.clone());
    l.account = block
        .as_ref()
        .and_then(Account::from_block)
        .map(|a| a.key().to_string());
    // Issue #1700: the incoming login is now in `~/.claude`; one without
    // OAuth is only ever an API-key profile's (`swap` checks that).
    l.live_kind = (login_kind(&home.join(".claude").join(CREDS)) != LoginKind::OAuth)
        .then(|| LIVE_KIND_PLATFORM.to_string());
    l.since = Some(j.started_at);
    l.swapped_at = Some(j.started_at);
    l.alert = None;
    rs.save_ledger(&l)?;
    step(rs, &mut j, Phase::S4)?;
    rs.delete_journal()
}

/// `s3`: replaces only `oauthAccount` in `$HOME/.claude.json`, keeping
/// every other key and the file's mode. A missing file is created. With no
/// block (an API-key profile without `oauthAccount`) the file is left
/// byte for byte as it was.
fn patch_home_account(home: &Path, block: Option<&serde_json::Value>) -> Result<(), ProfileError> {
    let Some(block) = block else {
        return Ok(());
    };
    let mut v = match read_claude_json(home) {
        Some((v, _)) => v,
        None if !exists(&home.join(CLAUDE_JSON)) => serde_json::json!({}),
        // `swap` refuses this before J0; only a file damaged since then,
        // during recovery, gets here.
        None => {
            eprintln!("warning: {HOME_JSON_UNREADABLE}; its account was not updated");
            return Ok(());
        }
    };
    if v.get("oauthAccount") == Some(block) {
        return Ok(());
    }
    v["oauthAccount"] = block.clone();
    let bytes = serde_json::to_vec_pretty(&v).map_err(|e| refused(e.to_string()))?;
    write_atomic_keep_mode(home, CLAUDE_JSON, &bytes).map_err(ProfileError::Io)
}
