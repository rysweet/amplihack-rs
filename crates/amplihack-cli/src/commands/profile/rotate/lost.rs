//! The ledger's `lost_login` list (issue #1693): profiles whose only login
//! a manual `/login` in `~/.claude` replaced. Each gets a line every cycle
//! until it has a login again or is removed. Only metadata is read. Since
//! issue #1886 recovery adds no profile (the only-login rule refuses any
//! move that would leave one with no login); entries written by an earlier
//! release are still reported and pruned.

use super::ledger::RotateLedger;
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

/// The repeating line for each profile in `lost_login`.
pub fn lines(l: &RotateLedger) -> Vec<String> {
    l.lost_login
        .iter()
        // Every name was checked when the ledger loaded, so none is dropped.
        .filter_map(|n| ProfileName::parse(n).ok())
        .map(|n| msg::lost_login(&n))
        .collect()
}
