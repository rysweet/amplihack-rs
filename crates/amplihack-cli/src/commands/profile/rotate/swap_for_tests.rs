//! The swap as lib tests call it: `swap_with` with the default identity
//! reader, no snapshot and no organisation rule. Kept out of `swap.rs`,
//! which holds no test code.

use super::{LoginExchange, Source, swap_with};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::HostExchange;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::ledger::RotateState;
use crate::commands::profile::rotate::login_identity::{Budget, Reader, default_read};
use crate::commands::profile::store::ProfileStore;
use std::path::Path;

/// The swap through the host's exchange, which only Linux has.
pub fn swap(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: Source,
    to: &ProfileName,
    now: i64,
) -> Result<ProfileName, ProfileError> {
    swap_by(&HostExchange, store, rs, home, from, to, now)
}

/// The swap through the exchange the test holds (issue #2935).
pub fn swap_by(
    exchange: &dyn LoginExchange,
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: Source,
    to: &ProfileName,
    now: i64,
) -> Result<ProfileName, ProfileError> {
    let budget = Budget::new();
    let read = default_read();
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
        None,
        exchange,
    )
}
