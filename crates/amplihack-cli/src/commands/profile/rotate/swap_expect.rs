//! Issue #1845 (D2, D4): the swap and the recorded accounts. [`guard`]
//! runs before J0 and only reads: it refuses to save the login in
//! `~/.claude` into a profile recorded as another account, and decides what
//! the swap may record. [`settle`] writes that record once the swap has
//! finished; a failure there is a warning, never an error. [`relabel`]
//! runs before J0: when the outgoing profile's record names the live login
//! but its `.claude.json` does not, it writes the live `oauthAccount` block
//! there, so the next swap back into that profile names the right account.

use super::identify::Account;
use super::ledger::RotateState;
use super::swap::Source;
use super::swap_check::account_block;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, State, exists, read_claude_json, write_atomic_keep_mode,
};
use crate::commands::profile::duplicates::{self, Holder};
use crate::commands::profile::expected;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::path::Path;

/// The refusal reason, followed by the outgoing profile's name.
pub const ACCOUNT_MISMATCH: &str = "the login in ~/.claude is not the account recorded for profile";

/// The refusal when [`relabel`] cannot write, followed by the profile's name.
pub const NOT_RELABELLED: &str =
    "the recorded account could not be written to the .claude.json of profile";

/// What a completed swap records for the outgoing profile.
pub(super) enum Pending {
    /// Nothing.
    Nothing,
    /// The outgoing profile's record names the live login, but its
    /// `.claude.json` does not: [`relabel`] writes this `oauthAccount`
    /// block there before J0. Nothing is recorded.
    Relabel(serde_json::Value),
    /// The adopted login, for the new `default-original[-N]`; it replaces
    /// a record left behind by a removed profile of that name.
    Adopted(Account),
    /// An adopted login that another profile holds or has recorded: the new
    /// profile gets no record, and a record left behind by a removed
    /// profile of that name is removed.
    AdoptedUnrecorded,
    /// The outgoing profile's own login, for a profile with no record yet.
    Backfill(Account),
}

/// Whether `e` is the refusal [`guard`] returns: the cycle writes its ALERT
/// once per profile, keyed `account_mismatch:<from>`.
pub fn is_account_mismatch(e: &ProfileError) -> bool {
    matches!(e, ProfileError::RotateRefused(m) if m.starts_with(ACCOUNT_MISMATCH))
}

/// The warning printed when `name`'s record exists but cannot be read, or
/// the state holding it cannot be opened. Names the profile, never an
/// account.
pub(super) fn unchecked(name: &ProfileName) -> String {
    format!(
        "warning: the recorded account for profile '{name}' could not be read; \
         the swap was not checked against it"
    )
}

/// Refuses a swap from a profile whose record names another account than
/// the one in `$HOME/.claude.json`. Otherwise says what [`settle`] records:
/// the adopted account when no profile holds it or has it recorded, or, for
/// a profile without a record, the outgoing account when the ledger shows
/// it is the one that came out of `from` and no other profile holds it or
/// has it recorded.
/// A record that cannot be read counts as none, with one warning.
pub(super) fn guard(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: &Source,
) -> Result<Pending, ProfileError> {
    let mut note = None;
    let decided = decide(store, rs, home, from, &mut note);
    if let Some(note) = note {
        eprintln!("{note}");
    }
    decided
}

/// [`guard`] without the printing: `note` gets the warning, if any.
pub(super) fn decide(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: &Source,
    note: &mut Option<String>,
) -> Result<Pending, ProfileError> {
    let Some(live) = Account::of_dir(home) else {
        return Ok(Pending::Nothing);
    };
    let name = match from {
        // Adoption is never refused. It is recorded under the same rule as
        // a backfill: an existing profile holding the same login may be its
        // owner (#1845), and a record here would make the duplicate warning
        // blame that profile. `default-original[-N]` does not exist yet, so
        // no profile is excluded.
        Source::Adopt => {
            let claimed = State::new(store.root()).map_or(true, |state| {
                claimed_elsewhere(store, &state, home, None, &live)
            });
            return Ok(if claimed {
                Pending::AdoptedUnrecorded
            } else {
                Pending::Adopted(live)
            });
        }
        Source::Profile(name) => name,
    };
    // A state that cannot be opened reads as no record.
    let Ok(state) = State::new(store.root()) else {
        *note = Some(unchecked(name));
        return Ok(Pending::Nothing);
    };
    let key = expected::lookup(&state, name).unwrap_or_else(|()| {
        *note = Some(unchecked(name));
        None
    });
    match key {
        Some(rec) if rec.names(&live) => Ok(relabel_for(store, home, name, &live)),
        Some(_) => Err(ProfileError::RotateRefused(format!(
            "{ACCOUNT_MISMATCH} {name}"
        ))),
        None => {
            // A ledger that cannot be read only rules out the backfill.
            let came_from = rs.load_ledger().ok().flatten().is_some_and(|l| {
                l.live.as_deref() == Some(name.as_str())
                    && l.account.as_deref().is_some_and(|k| live.matches_key(k))
            });
            Ok(
                if came_from && !claimed_elsewhere(store, &state, home, Some(name), &live) {
                    Pending::Backfill(live)
                } else {
                    Pending::Nothing
                },
            )
        }
    }
}

/// [`Pending::Relabel`] with the block in `~/.claude.json` when `name`'s
/// `.claude.json` does not name `live`, the account its record names; after
/// a `/login` in `~/.claude` to that account it still names the one before.
fn relabel_for(store: &ProfileStore, home: &Path, name: &ProfileName, live: &Account) -> Pending {
    let dir = store.root().join(name.as_str());
    if Account::of_dir(&dir).is_some_and(|a| a.same(live)) {
        return Pending::Nothing;
    }
    account_block(home).map_or(Pending::Nothing, Pending::Relabel)
}

/// Writes the block of [`Pending::Relabel`] into the outgoing profile's
/// `.claude.json`, keeping every other key and the file's mode; a missing
/// file is created. A failure is a refusal, before anything has moved: the
/// login would otherwise go into a folder that names another account.
pub(super) fn relabel(
    store: &ProfileStore,
    from: &Source,
    pending: &Pending,
) -> Result<(), ProfileError> {
    let (Source::Profile(name), Pending::Relabel(block)) = (from, pending) else {
        return Ok(());
    };
    let dir = store.root().join(name.as_str());
    let fail = |why: String| ProfileError::RotateRefused(format!("{NOT_RELABELLED} {name}: {why}"));
    let mut v = match read_claude_json(&dir) {
        Some((v, _)) => v,
        None if !exists(&dir.join(CLAUDE_JSON)) => serde_json::json!({}),
        None => return Err(fail("it is not a JSON object".into())),
    };
    v["oauthAccount"] = block.clone();
    let bytes = serde_json::to_vec_pretty(&v).map_err(|e| fail(e.to_string()))?;
    write_atomic_keep_mode(&dir, CLAUDE_JSON, &bytes).map_err(|e| fail(e.to_string()))
}

/// Whether `live` may belong to a profile other than `name` (any profile,
/// when `name` is `None`): another profile holds the same account, or has
/// it recorded. A folder can already hold another account's login (#1845:
/// one was written by a session, not a swap), and a backfill would make
/// that wrong account the one the guard protects; an adoption would make
/// the duplicate warning blame the profile that may own it. When the other
/// profiles cannot be read, this answers `true`, so nothing is recorded.
fn claimed_elsewhere(
    store: &ProfileStore,
    state: &State,
    home: &Path,
    name: Option<&ProfileName>,
    live: &Account,
) -> bool {
    let Ok(list) = duplicates::try_accounts(store, Some(home)) else {
        return true;
    };
    let held = list
        .iter()
        .any(|(h, a)| matches!(h, Holder::Profile(n) if Some(n) != name) && a.same(live));
    let Ok(names) = store.list() else {
        return true;
    };
    held || names
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .filter(|n| Some(n) != name)
        .any(|n| matches!(expected::lookup(state, &n), Ok(Some(rec)) if rec.names(live)))
}

/// Writes what [`guard`] decided for `from`, after the swap finished. A
/// backfill is skipped when a record appeared in between.
pub(super) fn settle(store: &ProfileStore, from: &ProfileName, pending: Pending) {
    match pending {
        Pending::Nothing | Pending::Relabel(_) => {}
        Pending::Adopted(a) => expected::save(store.root(), from, &a, false),
        Pending::AdoptedUnrecorded => expected::forget(store.root(), from),
        Pending::Backfill(a) => expected::save(store.root(), from, &a, true),
    }
}
