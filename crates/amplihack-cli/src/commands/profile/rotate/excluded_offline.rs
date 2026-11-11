//! Issue #2218: which excluded profile's login is in `~/.claude`, decided
//! from local files only, so no request is ever sent with its token. See
//! "Local proof of an excluded live login" in
//! docs/reference/profile-command.md.
//!
//! - Rule 1 (`Match::Ledger`): the rotate ledger's `live` is excluded. That
//!   is enough on its own.
//! - Rule 2 (`Match::LocalAccount`): the account in `~/.claude.json`
//!   matches an excluded profile's record.
//! - Rule 3 (`Match::SameBytes`): `~/.claude/.credentials.json` is
//!   byte-identical to an excluded profile's stored login.
//!
//! Rules 2 and 3 run whatever the ledger names, since a hand login leaves
//! the ledger stale. A stale `~/.claude.json` that names an excluded profile
//! therefore makes `~/.claude` local-only: the check fails closed, and
//! [`may_write_back`] refuses the move-out.
//!
//! A read that fails gives no match for that rule; nothing is written.

use super::super::identity_claims;
use super::super::ledger::RotateState;
use super::Account;
// Its own line: `tests_source_scan` refuses a read next to the login name.
use super::super::login_identity::Budget;
use crate::commands::profile::auto_switch::kit::CREDS;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, MAX_CLAUDE_JSON, State, euid, read_bounded_nofollow, same_private_bytes,
    writable_by_others,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd::exclude_list;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

// Declared here, not in rotate/mod.rs, for the same line budget.
#[cfg(all(test, target_os = "linux"))]
#[path = "tests_issue_2218.rs"]
mod tests_issue_2218;

/// The largest login file compared byte for byte (rule 3).
const MAX_LOGIN_BYTES: u64 = 64 * 1024;

/// Which rule named the excluded live login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    Ledger,
    LocalAccount,
    SameBytes,
}

/// The excluded profile whose login is in `<home>/.claude`, and the rule
/// that named it; `None` when no rule matches or nothing is excluded. The
/// exclude list is read quietly: each command or cycle warns once itself.
pub fn live_excluded_offline(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Option<(ProfileName, Match)> {
    let excluded = exclude_list(store.root(), |_| {});
    if excluded.is_empty() {
        return None;
    }
    // An unreadable ledger proves nothing either way; rules 2 and 3 still
    // apply, so the check fails closed.
    let live = rs
        .load_ledger()
        .ok()
        .flatten()
        .and_then(|l| l.live)
        .and_then(|s| ProfileName::parse(&s).ok());
    if let Some(name) = live
        && excluded.contains(&name)
    {
        return Some((name, Match::Ledger));
    }
    let by_account = local_account(home).and_then(|account| {
        excluded
            .iter()
            .find(|p| record_names(store, p, &account))
            .map(|p| (p.clone(), Match::LocalAccount))
    });
    by_account.or_else(|| {
        excluded
            .into_iter()
            .find(|p| same_login(&home.join(".claude"), &slot(store, p)))
            .map(|p| (p, Match::SameBytes))
    })
}

/// Whether moving the excluded live login `name` out may write it into
/// `name`'s folder (S4): the ledger named it with no hand change suspected
/// and `~/.claude.json` agrees with its record, or the login in `~/.claude`
/// is already byte-identical to the one in its folder. A match by the local
/// account alone may be a stale `~/.claude.json`, so it never may.
pub fn may_write_back(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    name: &ProfileName,
) -> bool {
    match live_excluded_offline(store, rs, home) {
        Some((n, Match::SameBytes)) => &n == name,
        Some((n, Match::Ledger)) if &n == name => {
            let calm = rs
                .load_ledger()
                .ok()
                .flatten()
                .is_some_and(|l| l.hand_suspect.is_none());
            let agrees =
                || local_account(home).is_some_and(|account| record_names(store, name, &account));
            let same = || same_login(&home.join(".claude"), &slot(store, name));
            (calm && agrees()) || same()
        }
        _ => false,
    }
}

/// The account in `<dir>/.claude.json` when it may count as evidence: a
/// regular file of the current user that group and others cannot write.
/// For `~/.claude` the file is `<home>/.claude.json`.
pub(in crate::commands::profile::rotate) fn local_account(dir: &Path) -> Option<Account> {
    let (raw, meta) = read_bounded_nofollow(&dir.join(CLAUDE_JSON), MAX_CLAUDE_JSON).ok()??;
    // `writable_by_others` accepts group write for the user's private
    // group; evidence for trusting a login refuses any group write.
    if meta.uid() != euid() || writable_by_others(&meta) || meta.mode() & 0o020 != 0 {
        return None;
    }
    Account::from_value(&serde_json::from_slice(&raw).ok()?)
}

/// Whether `name`'s stored identity record names `account`.
fn record_names(store: &ProfileStore, name: &ProfileName, account: &Account) -> bool {
    State::new(store.root()).is_ok_and(|state| {
        identity_claims::record(&state, name)
            .ok()
            .flatten()
            .is_some_and(|rec| rec.names(account))
    })
}

/// Whether the login files of two folders are byte-identical (rule 3). The
/// comparison is in `auto_switch`, so no rotate file reads a login.
fn same_login(a: &Path, b: &Path) -> bool {
    same_private_bytes(&a.join(CREDS), &b.join(CREDS), MAX_LOGIN_BYTES)
}

fn slot(store: &ProfileStore, name: &ProfileName) -> PathBuf {
    store.root().join(name.as_str())
}

/// The folders whose identity is taken from local files only this cycle:
/// every excluded profile's, and `~/.claude` when a rule names its login.
pub(in crate::commands::profile::rotate) fn local_only_dirs(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = exclude_list(store.root(), |_| {})
        .iter()
        .map(|p| slot(store, p))
        .collect();
    if live_excluded_offline(store, rs, home).is_some() {
        dirs.push(home.join(".claude"));
    }
    dirs
}

/// A budget whose local-only folders are [`local_only_dirs`], for the
/// readers outside a cycle: `profile seed-record` and start-up recovery.
pub(in crate::commands::profile::rotate) fn gated_budget(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Budget {
    let budget = Budget::new();
    budget.set_local_only(home, local_only_dirs(store, rs, home));
    budget
}

/// Whether `name` is in the exclude list, read quietly.
pub(in crate::commands::profile::rotate) fn is_excluded(
    store: &ProfileStore,
    name: &ProfileName,
) -> bool {
    exclude_list(store.root(), |_| {}).contains(name)
}
