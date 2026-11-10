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
use std::path::PathBuf;

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
    let state = crate::commands::profile::auto_switch::kit::State::new(store.root())
        .map_err(|_| "untrusted")?;
    match (super::identity_claims::record(&state, &name), home) {
        (Ok(Some(r)), Some(h)) if r.names(h) => Ok(Some(name)),
        _ => Err("account does not match"),
    }
}

/// The one login-less profile whose account is `account`: `Known`, or
/// `Ambiguous` for several, or `Adopt` for none. A login-less profile whose
/// identity record cannot be read refuses the swap: it may be the one that
/// holds `account`, and adopting a second profile for it is not safe.
fn match_login_less(store: &ProfileStore, account: &Account) -> Result<LiveId, ProfileError> {
    let state = crate::commands::profile::auto_switch::kit::State::new(store.root())?;
    let mut matched = Vec::new();
    for n in store.list()? {
        let Ok(name) = ProfileName::parse(&n) else {
            continue;
        };
        if login_less(store, &name).is_none() {
            continue;
        }
        let record = super::identity_claims::record(&state, &name)
            .map_err(super::swap_authorize::refused)?;
        if record.is_some_and(|r| r.names(account)) {
            matched.push(name);
        }
    }
    let mut matched = matched.into_iter();
    Ok(match (matched.next(), matched.next()) {
        (Some(name), None) => LiveId::Known(name),
        (Some(_), Some(_)) => LiveId::Ambiguous,
        (None, _) => LiveId::Adopt,
    })
}

pub(super) fn find_with(
    store: &ProfileStore,
    account: Option<&Account>,
) -> Result<(LiveId, Option<&'static str>), ProfileError> {
    let ignored = match from_prototype(store, account) {
        Ok(Some(name)) => return Ok((LiveId::Known(name), None)),
        Ok(None) => None,
        Err(reason) => Some(reason),
    };
    let Some(account) = account else {
        return Ok((LiveId::NoLogin, ignored));
    };
    Ok((match_login_less(store, account)?, ignored))
}
