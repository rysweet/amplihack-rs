//! The ledger's `lost_login` list (issue #1693): profiles whose only login
//! a manual `/login` in `~/.claude` replaced. Each gets a line every cycle
//! until it has a login again or is removed. Only metadata is read.

use super::ledger::{MAX_LOST_LOGIN, RotateLedger};
use super::log::msg;
use crate::commands::profile::auto_switch::kit::{CREDS, exists};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;

/// The profile directory is a real directory (not a symlink) with nothing
/// at `.credentials.json`.
pub fn lost(store: &ProfileStore, name: &str) -> bool {
    let dir = store.root().join(name);
    std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) && !exists(&dir.join(CREDS))
}

/// Drops each profile that has a login again, or is gone.
pub fn prune(l: &mut RotateLedger, store: &ProfileStore) {
    l.lost_login.retain(|n| lost(store, n));
}

/// Adds `name` to `lost_login`. When the list is full it is not added, and
/// the one-off line that says so is returned.
pub fn add(l: &mut RotateLedger, name: &ProfileName) -> Option<String> {
    if l.lost_login.iter().any(|n| n == name.as_str()) {
        return None;
    }
    if l.lost_login.len() >= MAX_LOST_LOGIN {
        return Some(msg::lost_login_cap(name));
    }
    l.lost_login.push(name.to_string());
    None
}

/// The repeating line for each profile in `lost_login`, except `skip`: a
/// profile whose guidance this cycle already logs it in.
pub fn lines(l: &RotateLedger, skip: Option<&ProfileName>) -> Vec<String> {
    l.lost_login
        .iter()
        // Every name was checked when the ledger loaded, so none is dropped.
        .filter_map(|n| ProfileName::parse(n).ok())
        .filter(|n| Some(n) != skip)
        .map(|n| msg::lost_login(&n))
        .collect()
}
