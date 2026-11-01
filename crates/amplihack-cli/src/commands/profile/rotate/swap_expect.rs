//! Issue #1845 (D2, D4): the swap and the recorded accounts. [`guard`]
//! runs before J0 and only reads: it refuses to save the login in
//! `~/.claude` into a profile recorded as another account, and decides what
//! the swap may record. [`settle`] writes that record once the swap has
//! finished; a failure there is a warning, never an error.

use super::identify::Account;
use super::ledger::RotateState;
use super::swap::Source;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::State;
use crate::commands::profile::expected;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::path::Path;

/// The refusal reason, followed by the outgoing profile's name.
pub const ACCOUNT_MISMATCH: &str = "the login in ~/.claude is not the account recorded for profile";

/// What a completed swap records for the outgoing profile.
pub(super) enum Pending {
    /// Nothing.
    Nothing,
    /// The adopted login, for the new `default-original[-N]`; it replaces
    /// a record left behind by a removed profile of that name.
    Adopted(Account),
    /// The outgoing profile's own login, for a profile with no record yet.
    Backfill(Account),
}

/// Whether `e` is the refusal [`guard`] returns: the cycle writes its ALERT
/// once per profile, keyed `account_mismatch:<from>`.
pub fn is_account_mismatch(e: &ProfileError) -> bool {
    matches!(e, ProfileError::RotateRefused(m) if m.starts_with(ACCOUNT_MISMATCH))
}

/// The state the records are kept in; `None` reads as no record.
fn state(store: &ProfileStore) -> Option<State> {
    State::new(store.root()).ok()
}

/// Refuses a swap from a profile whose record names another account than
/// the one in `$HOME/.claude.json`. Otherwise says what [`settle`] records:
/// the adopted account, or, for a profile without a record, the outgoing
/// account when the ledger shows it is the one that came out of `from`.
pub(super) fn guard(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: &Source,
) -> Result<Pending, ProfileError> {
    let Some(live) = Account::of_dir(home) else {
        return Ok(Pending::Nothing);
    };
    let name = match from {
        Source::Adopt => return Ok(Pending::Adopted(live)),
        Source::Profile(name) => name,
    };
    let Some(state) = state(store) else {
        return Ok(Pending::Nothing);
    };
    match expected::recorded(&state, name) {
        Some(key) if live.matches_key(&key) => Ok(Pending::Nothing),
        Some(_) => Err(ProfileError::RotateRefused(format!(
            "{ACCOUNT_MISMATCH} {name}"
        ))),
        None => {
            // A ledger that cannot be read only rules out the backfill.
            let came_from = rs.load_ledger().ok().flatten().is_some_and(|l| {
                l.live.as_deref() == Some(name.as_str())
                    && l.account.as_deref().is_some_and(|k| live.matches_key(k))
            });
            Ok(if came_from {
                Pending::Backfill(live)
            } else {
                Pending::Nothing
            })
        }
    }
}

/// Writes what [`guard`] decided for `from`, after the swap finished. A
/// backfill is skipped when a record appeared in between.
pub(super) fn settle(store: &ProfileStore, from: &ProfileName, pending: Pending) {
    let (account, backfill) = match pending {
        Pending::Nothing => return,
        Pending::Adopted(a) => (a, false),
        Pending::Backfill(a) => (a, true),
    };
    let Some(state) = state(store) else {
        eprintln!(
            "warning: the recorded account for profile '{from}' was not saved: the profile root cannot be read"
        );
        return;
    };
    if backfill && expected::recorded(&state, from).is_some() {
        return;
    }
    if let Err(e) = expected::record(&state, from, &account) {
        eprintln!("warning: the recorded account for profile '{from}' was not saved: {e}");
    }
}
