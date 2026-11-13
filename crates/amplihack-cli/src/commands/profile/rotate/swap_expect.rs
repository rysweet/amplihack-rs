//! Issue #1845 (D2, D4): the swap and the recorded accounts. [`guard`]
//! runs before J0 and only reads: it refuses to save the login in
//! `~/.claude` into a profile recorded as another account, and decides what
//! the swap may record. [`settle`] writes that record once the swap has
//! finished; a failure there is a warning, never an error. [`relabel`]
//! runs before J0: when the outgoing profile's record names the live login
//! but its `.claude.json` does not, it writes the live `oauthAccount` block
//! there, so the next swap back into that profile names the right account.

use super::identify::Account;
#[cfg(all(test, target_os = "linux"))]
use super::ledger::RotateState;
use super::swap::Source;
use crate::commands::profile::ProfileError;
#[cfg(all(test, target_os = "linux"))]
use crate::commands::profile::auto_switch::kit::State;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, exists, read_claude_json, write_atomic_keep_mode,
};
use crate::commands::profile::expected;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
#[cfg(all(test, target_os = "linux"))]
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
    /// Existing adoption compatibility when another known holder claims it.
    AdoptedUnrecorded,
    /// An adopted login that another profile holds or has recorded: the new
    /// profile gets no record, and a record left behind by a removed
    /// profile of that name is removed.
    /// The outgoing profile's own login, for a profile with no record yet.
    Backfill(Account),
}

/// Whether `e` is the refusal [`guard`] returns: the cycle writes its ALERT
/// once per profile, keyed `account_mismatch:<from>`.
pub fn is_account_mismatch(e: &ProfileError) -> bool {
    matches!(e, ProfileError::RotateRefused(m) if m.starts_with(ACCOUNT_MISMATCH))
}

/// [`guard`] without the printing: `note` gets the warning, if any.
#[cfg(all(test, target_os = "linux"))]
pub(super) fn decide(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: &Source,
    note: &mut Option<String>,
) -> Result<Pending, ProfileError> {
    let budget = super::login_identity::Budget::new();
    let read = super::login_identity::default_read();
    let reader = super::login_identity::Reader {
        read: &read,
        budget: &budget,
    };
    let live = reader
        .oauth(&home.join(".claude"))
        .map_err(super::swap_authorize::refused)?;
    *note = None;
    let state = State::new(store.root())?;
    match from {
        Source::Profile(name) => match super::identity_claims::record(&state, name)
            .map_err(super::swap_authorize::refused)?
        {
            Some(rec) if rec.names(&live) => Ok(relabel_for(store, name, &live)),
            Some(_) => Err(ProfileError::RotateRefused(format!(
                "{ACCOUNT_MISMATCH} {name}"
            ))),
            None => {
                let came_from = rs
                    .load_ledger()?
                    .is_some_and(|l| super::swap_authorize::ledger_owns(&l, name, &live));
                let scan = super::identity_claims::scan(store, &state, Some(name), &live, reader)
                    .map_err(super::swap_authorize::refused)?;
                if came_from && matches!(scan.claim, super::identity_claims::Claim::Unclaimed) {
                    Ok(Pending::Backfill(live))
                } else {
                    Err(super::swap_authorize::refused(
                        super::login_identity::UNAVAILABLE,
                    ))
                }
            }
        },
        Source::Adopt => {
            let scan = super::identity_claims::scan(store, &state, None, &live, reader)
                .map_err(super::swap_authorize::refused)?;
            if matches!(scan.claim, super::identity_claims::Claim::Unclaimed) {
                Ok(Pending::Adopted(live))
            } else {
                Err(super::swap_authorize::refused(
                    super::login_identity::UNAVAILABLE,
                ))
            }
        }
    }
}

/// Repairs display state from trusted evidence, never from a session block.
pub(super) fn relabel_for(store: &ProfileStore, name: &ProfileName, live: &Account) -> Pending {
    let dir = store.root().join(name.as_str());
    if Account::of_dir(&dir).is_some_and(|a| a.same(live)) {
        return Pending::Nothing;
    }
    Pending::Relabel(live.block())
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
