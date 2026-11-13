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
pub use super::swap_check::adoption_name;
pub(super) use super::swap_check::check_home;
pub(super) use super::swap_check::{ino_at, show};
use super::swap_check::{may_go_live, no_oauth};
#[cfg(test)]
pub use super::swap_expect::is_account_mismatch;
use super::swap_expect::{relabel, settle};
pub(super) use super::swap_finish::{finish, remove_unused_adoption};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, CREDS, MoveError, REFRESH_LOCK, abort_point, check_root_private, exists,
    movable_login, read_claude_json, release_refresh_lock_at, take_refresh_lock_at,
    write_private_atomic,
};
use crate::commands::profile::kind::{self, Kind};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
pub(super) use prepare::{abandon, drop_locks, release_locks, source_current, take_lock};
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
    swap_operation(
        store,
        rs,
        home,
        from,
        to,
        now,
        reader,
        basis,
        None,
        crate::commands::profile::auto_switch::kit::now,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn swap_operation(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: Source,
    to: &ProfileName,
    now: i64,
    reader: Reader<'_>,
    basis: Option<&super::observe::Snap>,
    completion: Option<super::swap_record::Completion>,
    clock: fn() -> i64,
) -> Result<ProfileName, ProfileError> {
    swap_operation_custody(
        store,
        rs,
        home,
        from,
        to,
        now,
        reader,
        basis,
        completion,
        clock,
        &mut super::custody::Strategy::Original,
    )
}

#[path = "swap_execute.rs"]
mod execute;
pub(super) use execute::swap_operation_custody;

/// Rewrites the journal at `phase`, then stops there in a test run.
pub(super) fn step(
    rs: &RotateState,
    j: &mut RotateJournal,
    phase: Phase,
) -> Result<(), ProfileError> {
    j.phase = phase;
    rs.save_journal(j)?;
    if j.completion.is_some() {
        abort_point(phase.as_str());
    }
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
