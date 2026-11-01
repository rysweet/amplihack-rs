//! Issue #1845 (D1): the account each profile is meant to hold, recorded at
//! `<root>/.auto-switch.d/expected/<name>.json` as `{"account": <key>}`.
//!
//! The record is written only after an accepted `profile login`, an adopted
//! login, or a swap that moved a profile's own login back (see
//! `rotate/swap_expect.rs`). Every `.claude.json` is compared against it,
//! never the other way round. The file is read and written through the
//! state helpers, so it is atomic, private and never followed through a
//! symlink. Anything doubtful about it reads as no record; only the swap
//! guard says so, in one line naming the profile.

use super::auto_switch::kit::State;
use super::name::ProfileName;
use super::rotate::identify::Account;
use super::rotate::ledger::safe_account;
use crate::commands::profile::ProfileError;
use std::path::Path;

/// The state subdirectory that holds the records.
const SUB: &str = "expected";

/// Records `account` as the one `name` is meant to hold, replacing any
/// earlier record. Only the account key is stored.
pub fn record(state: &State, name: &ProfileName, account: &Account) -> Result<(), ProfileError> {
    let key = account.key();
    if !safe_account(key) {
        return Err(ProfileError::AutoSwitchInvariant(format!(
            "profile {name} has no account that can be recorded"
        )));
    }
    state.write_sub_json(SUB, name.as_str(), &serde_json::json!({ "account": key }))
}

/// The key recorded for `name`, or `None` when there is no usable record.
pub fn recorded(state: &State, name: &ProfileName) -> Option<String> {
    lookup(state, name).ok().flatten()
}

/// `Ok(None)` without a record, `Ok(Some(key))` for a usable one, and
/// `Err(())` for a record that exists but cannot be used.
pub(super) fn lookup(state: &State, name: &ProfileName) -> Result<Option<String>, ()> {
    let Some(v) = state.read_sub_json(SUB, name.as_str()).map_err(drop)? else {
        return Ok(None);
    };
    v.get("account")
        .and_then(serde_json::Value::as_str)
        .filter(|k| safe_account(k))
        .map(|k| Some(k.to_string()))
        .ok_or(())
}

/// `None` without a record; otherwise whether `account` is not the one
/// recorded for `name`.
pub fn mismatched(state: &State, name: &ProfileName, account: &Account) -> Option<bool> {
    recorded(state, name).map(|key| !account.matches_key(&key))
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
