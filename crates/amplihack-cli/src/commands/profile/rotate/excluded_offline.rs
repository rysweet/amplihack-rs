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
//! [`may_write_back`] refuses the move-out. The one exception: rule 2 is
//! skipped when the ledger names a live profile that is not excluded, no
//! hand change is suspected, and `~/.claude/.credentials.json` has not
//! changed since the ledger's `swapped_at`. After a move off an excluded
//! login, its running sessions keep writing its account into
//! `~/.claude.json`, and without this exception rotation would stall.
//!
//! A read that fails gives no match for that rule; nothing is written.

use super::super::identity_claims;
use super::super::ledger::RotateState;
use super::Account;
// Its own line: `tests_source_scan` refuses a read next to the login name.
use super::super::handfix_plan::ino_ctime;
use super::super::login_identity::Budget;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, MAX_CLAUDE_JSON, State, euid, read_bounded_nofollow, same_private_bytes,
    writable_by_others,
};
use crate::commands::profile::auto_switch::kit::{CREDS, now};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd::{exclude_list, read_exclude};
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
/// An error when the list falls back to every profile and the store cannot
/// be listed: no answer is better than "nothing is excluded".
pub fn live_excluded_offline(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Result<Option<(ProfileName, Match)>, ProfileError> {
    let excluded = exclude_list(store.root(), |_| {})?;
    Ok(find_live(store, rs, home, excluded))
}

fn find_live(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    excluded: Vec<ProfileName>,
) -> Option<(ProfileName, Match)> {
    if excluded.is_empty() {
        return None;
    }
    // An unreadable ledger proves nothing either way; rules 2 and 3 still
    // apply, so the check fails closed.
    let ledger = rs.load_ledger().ok().flatten();
    let live = ledger
        .as_ref()
        .and_then(|l| l.live.as_deref())
        .and_then(|s| ProfileName::parse(s).ok());
    if let Some(name) = &live
        && excluded.contains(name)
    {
        return Some((name.clone(), Match::Ledger));
    }
    // Rule 2 is skipped while the login a swap installed is untouched: a
    // session still running on the login moved off keeps writing its
    // account into `~/.claude.json`, which then says nothing about
    // `~/.claude`.
    let installed = live.is_some()
        && ledger
            .as_ref()
            .is_some_and(|l| l.hand_suspect.is_none() && untouched_since(home, l.swapped_at));
    let by_account = (!installed)
        .then(|| local_account(home))
        .flatten()
        .and_then(|account| {
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

/// The `live:` value of `rotate status`: the ledger's `live`, else (issue
/// #2218) the excluded profile a local rule names, with no request, else `-`.
/// When that rule cannot be applied (the exclude list falls back to every
/// profile and the store cannot be listed) the value says so rather than
/// `-`, which would read as "nothing is live".
pub fn status_live(
    store: &ProfileStore,
    rs: &RotateState,
    home: Option<&Path>,
    ledger_live: Option<String>,
) -> String {
    if let Some(live) = ledger_live {
        return live;
    }
    let Some(home) = home else { return "-".into() };
    match live_excluded_offline(store, rs, home) {
        Ok(Some((n, _))) => n.as_str().to_string(),
        Ok(None) => "-".into(),
        Err(e) => crate::commands::profile::rotate::handfix_msg::status_live_exclude_unknown(
            &e.to_string(),
        ),
    }
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
    // An error proves nothing, so nothing is written back.
    match live_excluded_offline(store, rs, home).ok().flatten() {
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

/// Whether `~/.claude/.credentials.json` has not changed since the swap
/// that finished at `swapped_at` (epoch seconds): its change time, read by
/// `lstat`, is no later. Any write to it, a session's token refresh or a
/// hand login, sets the change time, so the answer turns false and rule 2
/// applies again. A swap time in the future (a clock set back) proves
/// nothing.
fn untouched_since(home: &Path, swapped_at: Option<i64>) -> bool {
    let Some(at) = swapped_at.filter(|at| *at <= now()) else {
        return false;
    };
    ino_ctime(&home.join(".claude").join(CREDS)).is_some_and(|(_, _, ctime, _)| ctime <= at)
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
) -> Result<Vec<PathBuf>, ProfileError> {
    // An error, not a shorter list, when the exclude list is unknown.
    let excluded = exclude_list(store.root(), |_| {})?;
    let mut dirs: Vec<PathBuf> = excluded.iter().map(|p| slot(store, p)).collect();
    if find_live(store, rs, home, excluded).is_some() {
        dirs.push(home.join(".claude"));
    }
    Ok(dirs)
}

/// A budget whose local-only folders are [`local_only_dirs`], for the
/// readers outside a cycle: `profile seed-record` and start-up recovery.
pub(in crate::commands::profile::rotate) fn gated_budget(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Result<Budget, ProfileError> {
    let budget = Budget::new();
    budget.set_local_only(home, local_only_dirs(store, rs, home)?);
    Ok(budget)
}

/// Whether `name` is in the exclude list, read quietly.
pub(in crate::commands::profile::rotate) fn is_excluded(
    store: &ProfileStore,
    name: &ProfileName,
) -> bool {
    // An unknown list counts as excluding `name` (fails closed).
    exclude_list(store.root(), |_| {}).map_or(true, |l| l.contains(name))
}

/// Issue #2218: the `ALERT:` text that pauses a rotate cycle, and a dry
/// run's `would alert:`, when `.auto-switch` exists but cannot be read or
/// parsed: every profile counts as excluded, so there is nothing to
/// measure or rotate to. `warn` gets the reader's warning. `None` when the
/// list can be read.
pub(in crate::commands::profile::rotate) fn exclude_paused(
    store: &ProfileStore,
    warn: impl FnOnce(&str),
) -> Option<&'static str> {
    read_exclude(store.root(), warn)
        .is_none()
        .then_some(super::super::log::msg::EXCLUDE_PAUSED)
}
