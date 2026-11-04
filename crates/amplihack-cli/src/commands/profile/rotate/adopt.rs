//! Who is live when the rotate ledger names nobody (moved from `identify`,
//! issue #1886): the prototype ledger's profile, else the one login-less
//! profile whose `.claude.json` names the account in `~/.claude.json`, else
//! adoption. These rules only choose which profile the next swap saves the
//! outgoing login under; they never move a login, and hand-change recovery
//! never uses them. Account values are compared, never logged.

use super::account::Account;
use super::identify::LiveId;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{CREDS, exists};
use crate::commands::profile::default_live::{self, Prototype};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::path::{Path, PathBuf};

/// A profile directory without anything at `.credentials.json` (a dangling
/// symlink counts as something) that passes the spawn-time check.
fn login_less(store: &ProfileStore, name: &ProfileName) -> Option<PathBuf> {
    let dir = store.verify_profile_dir(name).ok()?;
    (!exists(&dir.join(CREDS))).then_some(dir)
}

/// The prototype ledger's profile if it may be trusted, else the reason it
/// is ignored. `Ok(None)`: no ledger, or the idle state.
fn from_prototype(
    store: &ProfileStore,
    home: Option<&Account>,
) -> Result<Option<ProfileName>, &'static str> {
    let value = match default_live::prototype(store.root()) {
        Prototype::Absent | Prototype::Live(None | Some(serde_json::Value::Null)) => {
            return Ok(None);
        }
        Prototype::Untrusted => return Err("untrusted"),
        Prototype::Live(Some(v)) => v,
    };
    let name = value
        .as_str()
        .and_then(|s| ProfileName::parse(s).ok())
        .ok_or("invalid name")?;
    let Ok(dir) = store.verify_profile_dir(&name) else {
        return Err("untrusted");
    };
    if exists(&dir.join(CREDS)) {
        return Err("profile has a login");
    }
    match (Account::of_dir(&dir), home) {
        (Some(a), Some(h)) if !a.same(h) => Err("account does not match"),
        (Some(_), None) => Err("account does not match"),
        _ => Ok(Some(name)),
    }
}

/// The one login-less profile whose account is `account`: `Known`, or
/// `Ambiguous` for several, or `Adopt` for none.
fn match_login_less(store: &ProfileStore, account: &Account) -> Result<LiveId, ProfileError> {
    let mut matched = store.list()?.into_iter().filter_map(|n| {
        let name = ProfileName::parse(&n).ok()?;
        let dir = login_less(store, &name)?;
        Account::of_dir(&dir)
            .is_some_and(|a| a.same(account))
            .then_some(name)
    });
    Ok(match (matched.next(), matched.next()) {
        (Some(name), None) => LiveId::Known(name),
        (Some(_), Some(_)) => LiveId::Ambiguous,
        (None, _) => LiveId::Adopt,
    })
}

/// The live profile with no rotate ledger naming one, and the reason the
/// prototype ledger was ignored, if it was.
pub fn find(
    store: &ProfileStore,
    home: &Path,
) -> Result<(LiveId, Option<&'static str>), ProfileError> {
    let account = Account::of_dir(home);
    let ignored = match from_prototype(store, account.as_ref()) {
        Ok(Some(name)) => return Ok((LiveId::Known(name), None)),
        Ok(None) => None,
        Err(reason) => Some(reason),
    };
    let Some(account) = account else {
        return Ok((LiveId::Adopt, ignored));
    };
    Ok((match_login_less(store, &account)?, ignored))
}
