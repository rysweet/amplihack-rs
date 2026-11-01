//! Issue #1845 (D1): the account each profile is meant to hold, recorded at
//! `<root>/.auto-switch.d/expected/<name>.json` as
//! `{"account": <key>, "org": <organization uuid or null>}`.
//!
//! The record is written only after an accepted `profile login`, an adopted
//! login, or a swap that moved a profile's own login back (see
//! `rotate/swap_expect.rs`). Every `.claude.json` is compared against it,
//! never the other way round, with [`Account::matches_record`]: one user in
//! two organizations is two accounts. The file is read and written through
//! the state helpers, so it is atomic, private and never followed through a
//! symlink. Anything doubtful about it, including a record without an
//! `org` entry, reads as no record; only the swap guard says so, in one
//! line naming the profile.

use super::auto_switch::kit::State;
use super::name::ProfileName;
use super::rotate::identify::Account;
use super::rotate::ledger::safe_account;
use crate::commands::profile::ProfileError;
use std::path::Path;

/// The state subdirectory that holds the records.
const SUB: &str = "expected";

/// One profile's record: the account key and its organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub account: String,
    pub org: Option<String>,
}

impl Record {
    /// Whether `account` is the recorded account.
    pub fn names(&self, account: &Account) -> bool {
        account.matches_record(&self.account, self.org.as_deref())
    }
}

/// Records `account` as the one `name` is meant to hold, replacing any
/// earlier record. Only the account key and the organization are stored.
pub fn record(state: &State, name: &ProfileName, account: &Account) -> Result<(), ProfileError> {
    let key = account.key();
    if !safe_account(key) || !account.org().is_none_or(safe_account) {
        return Err(ProfileError::AutoSwitchInvariant(format!(
            "profile {name} has no account that can be recorded"
        )));
    }
    let body = serde_json::json!({ "account": key, "org": account.org() });
    state.write_sub_json(SUB, name.as_str(), &body)
}

/// The account key recorded for `name`, or `None` when there is no usable
/// record.
pub fn recorded(state: &State, name: &ProfileName) -> Option<String> {
    lookup(state, name).ok().flatten().map(|r| r.account)
}

/// `Ok(None)` without a record, `Ok(Some(record))` for a usable one, and
/// `Err(())` for a record that exists but cannot be used.
pub(super) fn lookup(state: &State, name: &ProfileName) -> Result<Option<Record>, ()> {
    let Some(v) = state.read_sub_json(SUB, name.as_str()).map_err(drop)? else {
        return Ok(None);
    };
    let account = v
        .get("account")
        .and_then(serde_json::Value::as_str)
        .filter(|k| safe_account(k))
        .ok_or(())?;
    let org = match v.get("org").ok_or(())? {
        serde_json::Value::Null => None,
        serde_json::Value::String(o) if safe_account(o) => Some(o.clone()),
        _ => return Err(()),
    };
    Ok(Some(Record {
        account: account.to_string(),
        org,
    }))
}

/// `None` without a record; otherwise whether `account` is not the one
/// recorded for `name`.
pub fn mismatched(state: &State, name: &ProfileName, account: &Account) -> Option<bool> {
    let record = lookup(state, name).ok().flatten()?;
    Some(!record.names(account))
}

/// Records `account` for `name` in the state under `root`, printing a
/// warning when it cannot be saved. With `keep`, an existing record is left
/// in place.
pub fn save(root: &Path, name: &ProfileName, account: &Account, keep: bool) {
    let saved = State::new(root).and_then(|state| {
        if keep && recorded(&state, name).is_some() {
            return Ok(());
        }
        record(&state, name, account)
    });
    if let Err(e) = saved {
        eprintln!("warning: the recorded account for profile '{name}' was not saved: {e}");
    }
}
